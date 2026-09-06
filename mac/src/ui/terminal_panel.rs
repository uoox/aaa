//! 终端面板：侧栏底部的常驻入口 + 多标签页面（PROTOCOL「终端」，2026-09-02）。
//!
//! 终端 = `agent:"shell"` 的会话，但不是项目的会话：不进侧栏三态分组、不算
//! 项目激活、不参与 ⌃Tab，也不在新建项目的 agent 选择里。标签 = 存活的 shell
//! 会话按 created_at 排序；「+」在项目根 fresh 开一个；关标签 = kill 再 DELETE，
//! 不弹确认；shell 一 exited 就 DELETE（没有回放价值）。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::kit::*;
use super::{
    Modal, Page, RootView, next_terminal_after_close, resolve_active_terminal, sorted_terminals,
    terminal_tabs,
};
use crate::model::{Session, SessionState};
use crate::theme;

/// health 还没到时的项目根兜底（与新建项目弹窗的提示一致）
const DEFAULT_ROOT: &str = "~/project";

impl RootView {
    /// daemon 的项目根；health 未到时兜底
    pub(super) fn project_root(&self) -> String {
        self.health
            .as_ref()
            .map(|h| h.project_root.clone())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| DEFAULT_ROOT.into())
    }

    /// 面板里的标签。已发过 DELETE 的不算：刚关掉的标签不该在 exited 帧到来前
    /// 因为一帧 preview 更新又闪回来。
    pub(super) fn live_terminal_tabs(&self) -> Vec<(String, String)> {
        let root = self.project_root();
        terminal_tabs(
            self.sessions
                .iter()
                .filter(|s| !self.deleted_terminals.contains(&s.id)),
            &root,
        )
    }

    pub(super) fn live_terminal_ids(&self) -> Vec<String> {
        self.live_terminal_tabs()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// 会话表变了（快照 / 单帧 / 删除 / 拉取）之后的面板维护：回收 exited 的
    /// shell；面板开着时把激活标签对齐到存活的那个。
    pub(super) fn sessions_changed(&mut self, cx: &mut Context<Self>) {
        self.reap_exited_terminals(cx);
        if self.page == Page::Terminal {
            self.sync_active_terminal(cx);
        }
    }

    /// 终端没有回放价值：shell 会话一 exited 就 DELETE（每个 id 只发一次），
    /// 丢掉它的视图；正是当前标签的话挪到邻居。
    fn reap_exited_terminals(&mut self, cx: &mut Context<Self>) {
        let dead: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| {
                s.is_terminal()
                    && s.state == SessionState::Exited
                    && !self.deleted_terminals.contains(&s.id)
            })
            .map(|s| s.id.clone())
            .collect();
        for id in dead {
            if self.active_terminal.as_deref() == Some(id.as_str()) {
                // 邻居要按「它还在时」的顺序挑：把它自己也算进存活列表
                let order: Vec<String> = sorted_terminals(self.sessions.iter().filter(|s| {
                    !self.deleted_terminals.contains(&s.id)
                        && (s.state != SessionState::Exited || s.id == id)
                }))
                .iter()
                .map(|s| s.id.clone())
                .collect();
                self.active_terminal = next_terminal_after_close(&order, &id);
            }
            self.deleted_terminals.insert(id.clone());
            self.terminals.remove(&id);
            let fut = self.net.delete_session(&id);
            // 删失败只记日志：daemon 侧 200 条上限兜底，不值得弹错
            self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, false, cx);
        }
    }

    /// 激活标签对齐到存活的那个（None / 已死 → 最新），并确保它有视图。
    /// 只在新建了视图时请求焦点：每帧都抢会把弹窗里输入框的焦点抢走。
    fn sync_active_terminal(&mut self, cx: &mut Context<Self>) {
        let ids = self.live_terminal_ids();
        let resolved = resolve_active_terminal(self.active_terminal.as_deref(), &ids);
        self.active_terminal = resolved.clone();
        if let Some(id) = resolved
            && self.ensure_terminal_view(&id, cx)
            && matches!(self.modal, Modal::None)
        {
            self.pending_focus = Some(id);
        }
    }

    /// 切到终端面板（侧栏入口行）：焦点交给当前标签
    pub(super) fn open_terminal_panel(&mut self, cx: &mut Context<Self>) {
        self.page = Page::Terminal;
        self.sync_active_terminal(cx);
        if let Some(id) = self.active_terminal.clone() {
            self.pending_focus = Some(id);
        }
        cx.notify();
    }

    /// 激活某个标签（点标签 / 新开 / 旧 shell 项目双击 / 别处开的 shell）
    pub(super) fn focus_terminal(&mut self, id: String, cx: &mut Context<Self>) {
        self.ensure_terminal_view(&id, cx);
        self.active_terminal = Some(id.clone());
        self.page = Page::Terminal;
        self.pending_focus = Some(id);
        cx.notify();
    }

    /// 「+」：在项目根开一个新 shell（fresh，第二个标签必须真是第二个 shell）
    pub(super) fn new_terminal(&mut self, cx: &mut Context<Self>) {
        let root = self.project_root();
        self.create_terminal_in(root, true, cx);
    }

    /// 在 `dir` 开终端并切过去。`fresh`：true 一定新开（面板「+」）；false 沿用
    /// daemon 的幂等（目录里已有存活 shell 就回它，旧 shell 项目双击用）。
    pub(super) fn create_terminal_in(&mut self, dir: String, fresh: bool, cx: &mut Context<Self>) {
        let fut = self.net.create_terminal(dir, fresh);
        self.spawn_fetch(
            fut,
            |r, s: Session, cx| {
                let id = s.id.clone();
                r.upsert_session(s, cx);
                r.focus_terminal(id, cx);
            },
            true,
            cx,
        );
    }

    /// 关标签 = kill 再 DELETE，不弹确认（终端标签便宜）。本地立刻收起：
    /// 视图丢掉、标签从列表消失、当前标签挪到邻居。
    pub(super) fn close_terminal(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.deleted_terminals.contains(id) {
            return;
        }
        if self.active_terminal.as_deref() == Some(id) {
            let order = self.live_terminal_ids();
            self.active_terminal = next_terminal_after_close(&order, id);
            if self.page == Page::Terminal
                && let Some(next) = self.active_terminal.clone()
            {
                self.ensure_terminal_view(&next, cx);
                self.pending_focus = Some(next);
            }
        }
        self.deleted_terminals.insert(id.to_string());
        self.terminals.remove(id);
        let net = self.net.clone();
        let sid = id.to_string();
        cx.spawn(async move |this, cx| {
            // 先 kill 再 DELETE：DELETE 本身也会终止存活会话，但分两步走
            // kill 的 TERM→KILL 时序更稳；kill 失败（已经退了）不影响删除
            if let Err(e) = net.kill_session(&sid).await {
                log::debug!("终端 {sid} kill: {e}");
            }
            if let Err(e) = net.delete_session(&sid).await {
                let _ = this.update(cx, |r, cx| r.set_error(format!("关闭终端失败: {e}"), cx));
            }
        })
        .detach();
        cx.notify();
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    /// 侧栏底部的入口行（daemon 状态行上方）：`>_ 终端   n  ＋`
    /// 侧栏底部「历史」入口：所有出现过的会话，含已退出、已删除
    pub(super) fn render_history_entry(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let active = self.page == Page::History;
        div()
            .id("sb-history")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(7.))
            .border_t_1()
            .border_color(c(theme::edge()))
            .cursor_pointer()
            .when(active, |el| el.bg(c(theme::surface_raised())))
            .hover(|st| st.bg(c(theme::surface_raised())))
            .on_click(cx.listener(|this, _, _, cx| this.open_history(cx)))
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(c(if active { theme::accent() } else { theme::dim() }))
                    .child("⏱"),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(c(if active { theme::accent() } else { theme::ink() }))
                    .child("历史"),
            )
    }

    pub(super) fn render_terminal_entry(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let n = self.live_terminal_tabs().len();
        let active = self.page == Page::Terminal;
        let accent = c(if active { theme::accent() } else { theme::dim() });
        div()
            .id("sb-terminal")
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(16.))
            .py(px(7.))
            .border_t_1()
            .border_color(c(theme::edge()))
            .cursor_pointer()
            .when(active, |el| el.bg(c(theme::surface_raised())))
            .hover(|st| st.bg(c(theme::surface_raised())))
            .on_click(cx.listener(|this, _, _, cx| this.open_terminal_panel(cx)))
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(accent)
                    .child(">_"),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(c(if active { theme::accent() } else { theme::ink() }))
                    .child("终端"),
            )
            .when(n > 0, |el| {
                el.child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(10.))
                        .text_color(c(theme::faint()))
                        .child(SharedString::from(n.to_string())),
                )
            })
            .child(
                div()
                    .id("sb-terminal-new")
                    .flex_none()
                    .px(px(4.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .text_color(c(theme::dim()))
                    .hover(|st| st.text_color(c(theme::accent())).bg(c(theme::edge_light())))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.new_terminal(cx);
                    }))
                    .child("＋"),
            )
    }

    /// 面板正文：标签条 + 当前标签的终端视图；没有标签时给个开一个的入口
    pub(super) fn render_terminal_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tabs = self.live_terminal_tabs();
        let active = self.active_terminal.clone();

        let mut strip = div()
            .flex_none()
            .flex()
            .items_center()
            .h(px(30.))
            .overflow_hidden()
            .bg(c(theme::surface()))
            .border_b_1()
            .border_color(c(theme::edge()));
        for (ix, (id, label)) in tabs.iter().enumerate() {
            let is_active = active.as_deref() == Some(id.as_str());
            let id_click = id.clone();
            let id_close = id.clone();
            strip = strip.child(
                div()
                    .id(("term-tab", ix))
                    .group("term-tab")
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h_full()
                    .px(px(12.))
                    .border_r_1()
                    .border_color(c(theme::edge()))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(is_active, |el| {
                        el.bg(c(theme::surface_raised())).text_color(c(theme::ink()))
                    })
                    .when(!is_active, |el| {
                        el.text_color(c(theme::dim()))
                            .hover(|st| st.bg(c(theme::surface_raised())))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.focus_terminal(id_click.clone(), cx);
                    }))
                    .child(SharedString::from(label.clone()))
                    .child(
                        // × 关标签：当前标签常显，其余悬停才现身（与侧栏 × 一致）
                        div()
                            .id(("term-tab-close", ix))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .hover(|st| st.text_color(c(theme::red())).bg(c(theme::edge_light())))
                            .when(!is_active, |el| {
                                el.invisible().group_hover("term-tab", |st| st.visible())
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_terminal(&id_close, cx);
                            }))
                            .child("✕"),
                    ),
            );
        }
        strip = strip.child(
            div()
                .id("term-tab-new")
                .flex_none()
                .flex()
                .items_center()
                .h_full()
                .px(px(12.))
                .cursor_pointer()
                .text_size(px(14.))
                .text_color(c(theme::dim()))
                .hover(|st| st.text_color(c(theme::accent())).bg(c(theme::surface_raised())))
                .on_click(cx.listener(|this, _, _, cx| this.new_terminal(cx)))
                .child("＋"),
        );

        let view = active
            .as_deref()
            .and_then(|id| self.terminals.get(id))
            .cloned();
        let body = match view {
            Some(t) => div().flex_1().min_h(px(0.)).child(t),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(c(theme::faint()))
                .child(div().text_size(px(13.)).child("还没有终端 · 点 + 开一个"))
                .child(
                    tbtn("term-empty-new", "＋ 新终端")
                        .on_click(cx.listener(|this, _, _, cx| this.new_terminal(cx))),
                ),
        };

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(strip)
            .child(body)
    }

    /// 状态栏（终端面板）：当前标签所在目录 · 终端 · 标签数 / ⌘W 提示
    pub(super) fn render_terminal_statusbar(&self, bar: gpui::Div) -> gpui::Div {
        let path = self
            .active_terminal
            .as_deref()
            .and_then(|id| self.session(id))
            .map(|s| s.project_path.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| self.project_root());
        let n = self.live_terminal_tabs().len();
        let hint = if n > 0 {
            format!("{n} 个标签 · ⌘W 关闭当前")
        } else {
            String::new()
        };
        bar.child(
            div()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(SharedString::from(path)),
        )
        .child("终端")
        .child(
            div()
                .ml_auto()
                .text_color(c(theme::faint()))
                .child(SharedString::from(hint)),
        )
    }
}
