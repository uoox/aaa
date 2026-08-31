//! 消息流视图：daemon `/sessions/:id/messages` 的气泡列表，与 Android 同源同构。
//! 终端仍是权威视图；消息流只读展示（输入走终端或问题栏），⌘E 切换。
//!
//! 拉取模型：打开时全量补齐（seq 游标循环直到追平 last_seq），此后靠
//! /events 的 messages_changed 帧增量拉。`supported:false`（shell、
//! reasonix 等）或 404（v1 daemon）→ 上层自动回落终端并隐藏切换入口。

use gpui::{Context, ScrollHandle, SharedString, Window, div, prelude::*, px, relative};

use super::kit::{c, ca};
use crate::model::ChatMessage;
use crate::net::Net;
use crate::theme;

/// 内存里最多留这么多条：够回看，不至于无限膨胀
const KEEP: usize = 2000;

pub struct MessagesView {
    sid: String,
    net: Net,
    pub msgs: Vec<ChatMessage>,
    last_seq: u64,
    /// None = 首拉未回；Some(false) = 该会话不支持消息流（回落终端）
    pub supported: Option<bool>,
    loading: bool,
    /// 已展开的 thinking / tool 消息 seq（点击切换）
    expanded: std::collections::HashSet<u64>,
    scroll: ScrollHandle,
}

impl MessagesView {
    pub fn new(sid: String, net: Net, cx: &mut Context<Self>) -> Self {
        let mut v = MessagesView {
            sid,
            net,
            msgs: Vec::new(),
            last_seq: 0,
            supported: None,
            loading: false,
            expanded: std::collections::HashSet::new(),
            scroll: ScrollHandle::new(),
        };
        v.fetch(cx);
        v
    }

    /// 增量拉取；没追平 last_seq 就继续拉（打开旧会话时的补齐循环）
    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.supported == Some(false) {
            return;
        }
        self.loading = true;
        let fut = self.net.messages(&self.sid, self.last_seq);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.loading = false;
                match res {
                    Ok(r) => {
                        v.supported = Some(r.supported);
                        if !r.messages.is_empty() {
                            v.msgs.extend(r.messages);
                            v.msgs.sort_by_key(|m| m.seq);
                            v.msgs.dedup_by_key(|m| m.seq);
                            if v.msgs.len() > KEEP {
                                let cut = v.msgs.len() - KEEP;
                                v.msgs.drain(..cut);
                            }
                            v.last_seq = v.msgs.last().map(|m| m.seq).unwrap_or(0);
                            v.scroll.scroll_to_bottom();
                        }
                        if r.supported && v.last_seq < r.last_seq {
                            v.fetch(cx);
                        }
                    }
                    Err(e) => {
                        // v1 daemon 无此端点 → 永久回落终端；其余错误留待下次事件重试
                        if e.to_string().contains("404") {
                            v.supported = Some(false);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// messages_changed 帧带的 last_seq 没超过已见位置就不拉（自身回显去重）
    pub fn fetch_if_behind(&mut self, seq: u64, cx: &mut Context<Self>) {
        if seq == 0 || self.last_seq < seq {
            self.fetch(cx);
        }
    }

    fn toggle_expand(&mut self, seq: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&seq) {
            self.expanded.insert(seq);
        }
        cx.notify();
    }

    fn row(&self, m: &ChatMessage, cx: &mut Context<Self>) -> gpui::AnyElement {
        let text: SharedString = m.text.clone().into();
        match () {
            _ if m.kind == "thinking" => {
                let expanded = self.expanded.contains(&m.seq);
                let seq = m.seq;
                let first_line: SharedString = if expanded {
                    m.text.clone().into()
                } else {
                    m.text.lines().next().unwrap_or("").to_string().into()
                };
                div()
                    .id(("think", m.seq as usize))
                    .w_full()
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(8.))
                    .bg(c(theme::TERM_BG))
                    .cursor_pointer()
                    .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.toggle_expand(seq, cx)))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .italic()
                            .text_color(c(theme::FAINT))
                            .child(if expanded { "▾ 思考" } else { "▸ 思考" }),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .italic()
                            .text_color(c(theme::FAINT))
                            .when(!expanded, |el| {
                                el.overflow_hidden().text_ellipsis().whitespace_nowrap()
                            })
                            .child(first_line),
                    )
                    .into_any_element()
            }
            _ if m.kind == "tool_use" || m.kind == "tool_result" => {
                let (name, summary, status) = m
                    .tool
                    .as_ref()
                    .map(|t| (t.name.clone(), t.summary.clone(), t.status.clone()))
                    .unwrap_or_default();
                let status_color = match status.as_str() {
                    "ok" => theme::GREEN,
                    "err" => theme::RED,
                    "running" => theme::AMBER,
                    _ => theme::DIM,
                };
                let label: SharedString = if m.kind == "tool_result" {
                    "⎿ 结果".into()
                } else if name.is_empty() {
                    "tool".into()
                } else {
                    name.into()
                };
                let expanded = self.expanded.contains(&m.seq) && !m.text.is_empty();
                let seq = m.seq;
                div()
                    .id(("tool", m.seq as usize))
                    .w_full()
                    .cursor_pointer()
                    .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.toggle_expand(seq, cx)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                div()
                                    .w(px(6.))
                                    .h(px(6.))
                                    .flex_none()
                                    .rounded_full()
                                    .bg(c(status_color)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_family("Menlo")
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(c(theme::INK))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(11.5))
                                    .font_family("Menlo")
                                    .text_color(c(theme::DIM))
                                    .child(SharedString::from(summary)),
                            ),
                    )
                    .when(expanded, |el| {
                        el.child(
                            div()
                                .ml(px(13.))
                                .mt(px(3.))
                                .px(px(8.))
                                .py(px(6.))
                                .rounded(px(8.))
                                .bg(c(theme::TERM_BG))
                                .text_size(px(11.))
                                .font_family("Menlo")
                                .text_color(c(theme::DIM))
                                .child(text),
                        )
                    })
                    .into_any_element()
            }
            _ if m.role == "user" => div()
                .w_full()
                .flex()
                .justify_end()
                .child(
                    div()
                        .max_w(relative(0.78))
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(12.))
                        .bg(ca(theme::CYAN, 0.16))
                        .text_size(px(12.5))
                        .text_color(c(theme::INK))
                        .child(text),
                )
                .into_any_element(),
            _ if m.role == "system" => div()
                .w_full()
                .text_size(px(10.5))
                .text_color(c(theme::FAINT))
                .child(text)
                .into_any_element(),
            _ => div()
                .w_full()
                .flex()
                .child(
                    div()
                        .max_w(relative(0.86))
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(12.))
                        .bg(c(theme::SURFACE))
                        .text_size(px(12.5))
                        .text_color(c(theme::INK))
                        .child(text),
                )
                .into_any_element(),
        }
    }
}

impl Render for MessagesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let placeholder = |msg: &'static str| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::FAINT))
                .text_size(px(12.))
                .child(msg)
        };
        if self.msgs.is_empty() {
            let msg = match self.supported {
                None => "加载消息流…",
                Some(false) => "该会话不支持消息流（已回落终端）",
                Some(true) => "暂无消息",
            };
            return div().size_full().bg(c(theme::BG)).flex().flex_col().child(placeholder(msg));
        }
        let rows: Vec<gpui::AnyElement> =
            self.msgs.clone().iter().map(|m| self.row(m, cx)).collect();
        div().size_full().bg(c(theme::BG)).flex().flex_col().child(
            div()
                .id("msgs-scroll")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .px(px(16.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(rows),
        )
    }
}
