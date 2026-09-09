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
            self.spawn_fetch_ignore(fut, false, cx);
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
        // 不走 spawn_fetch：这里是两个请求串起来，前一个失败要吞掉、后一个失败
        // 才报错，spawn_fetch 一个 future 一个结果的形状装不下
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

    /// 侧栏底部「看板」入口：最近做了什么 + 还有什么没做
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
            .border_color(c(theme::EDGE))
            .cursor_pointer()
            .when(active, |el| el.bg(c(theme::SURFACE_RAISED)))
            .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
            .on_click(cx.listener(|this, _, _, cx| this.open_history(cx)))
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(c(if active { theme::ACCENT } else { theme::DIM }))
                    .child("▦"),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(c(if active { theme::ACCENT } else { theme::INK }))
                    .child("看板"),
            )
    }

    /// 侧栏里的终端小节：一条分隔线 + 「终端」小标题 + 一终端一行 + 「＋ 新增终端」。
    /// 2026-09-08 用户拍板：终端与对话同级——和项目行排在同一列里、同一套行样式，
    /// 点一行就是那一个终端。以前它是底部一个入口行，后面还藏着一条标签条。
    /// 同日用户「终端列表前面不需要三道杠」：行首那个记号（连同项目行的指示位）一起
    /// 拿掉了——终端没有状态可言，一个记号只是占着行首；标题现在顶格起，和项目行对齐。
    pub(super) fn render_terminal_rows(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(1.))
            .mt(px(10.))
            .child(
                meta()
                    .mx(px(6.))
                    .px(px(10.))
                    .pt(px(8.))
                    .pb(px(4.))
                    .border_t_1()
                    .border_color(c(theme::EDGE))
                    .child("终端"),
            );
        for (ix, (id, label)) in self.live_terminal_tabs().into_iter().enumerate() {
            let active =
                self.page == Page::Terminal && self.active_terminal.as_deref() == Some(id.as_str());
            let id_click = id.clone();
            let id_close = id;
            col = col.child(
                sidebar_row(("sb-term", ix).into())
                    // 当前终端与当前项目同一种说法：整行一圈强调色边框（2026-09-10）
                    .when(active, |el| el.border_color(c(theme::ACCENT)))
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.focus_terminal(id_click.clone(), cx);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(c(if active { theme::ACCENT } else { theme::DIM }))
                            .child(SharedString::from(label)),
                    )
                    // × 关终端（与项目行一致：一直画着）
                    .child(
                        row_btn(("sb-term-close", ix))
                            .hover(|st| st.text_color(c(theme::RED)).bg(c(theme::EDGE_LIGHT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_terminal(&id_close, cx);
                            }))
                            .child("✕"),
                    ),
            );
        }
        col.child(
            sidebar_row("sb-term-new".into())
                .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                .on_click(cx.listener(|this, _, _, cx| this.new_terminal(cx)))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .text_color(c(theme::ACCENT))
                        .child("＋ 新增终端"),
                ),
        )
    }

    /// 面板正文：当前终端的视图。标签条 2026-09-08 拆了——侧栏就是标签条，
    /// 两处并排列同一批终端只会互相打架。
    pub(super) fn render_terminal_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let view = self
            .active_terminal
            .as_deref()
            .and_then(|id| self.terminals.get(id))
            .cloned();
        match view {
            Some(t) => div().flex_1().min_h(px(0.)).child(t),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .text_color(c(theme::FAINT))
                .child(div().text_size(px(13.)).child("还没有终端"))
                .child(
                    tbtn("term-empty-new", "＋ 新终端")
                        .on_click(cx.listener(|this, _, _, cx| this.new_terminal(cx))),
                ),
        }
    }

    /// 状态栏（终端面板）：当前终端所在目录 · 终端 · ⌘W 提示
    pub(super) fn render_terminal_statusbar(&self, bar: gpui::Div) -> gpui::Div {
        let path = self
            .active_terminal
            .as_deref()
            .and_then(|id| self.session(id))
            .map(|s| s.project_path.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| self.project_root());
        let hint = if self.active_terminal.is_some() {
            "⌘W 关闭当前"
        } else {
            ""
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
                .text_color(c(theme::FAINT))
                .child(hint),
        )
    }
}
