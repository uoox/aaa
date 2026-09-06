//! 「历史」页：daemon 会话日志（GET /history）——所有出现过的会话，含已退出、已删除。
//! 点一行：会话还在（活着或有回放）就打开；已删除的只能看。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use crate::model::HistoryEntry;
use crate::theme;
use super::kit::*;

impl RootView {
    pub(super) fn open_history(&mut self, cx: &mut Context<Self>) {
        self.page = super::Page::History;
        self.fetch_history(cx);
        cx.notify();
    }

    pub(super) fn fetch_history(&mut self, cx: &mut Context<Self>) {
        let fut = self.net.history(300);
        self.spawn_fetch(
            fut,
            |r, entries: Vec<HistoryEntry>, cx| {
                r.history = entries;
                cx.notify();
            },
            true,
            cx,
        );
    }

    pub(super) fn render_history_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = chrono::Local::now();
        let alive_ids: std::collections::HashSet<&str> = self.sessions.iter().map(|s| s.id.as_str()).collect();
        let mut list = div().flex().flex_col().gap(px(2.)).w_full();
        if self.history.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.))
                    .text_color(c(theme::faint()))
                    .child("还没有记录（daemon 每秒把会话同步进日志）"),
            );
        }
        for e in &self.history {
            let openable = alive_ids.contains(e.id.as_str());
            let (status, color) = if e.deleted_at.is_some() {
                ("已删除", theme::faint())
            } else {
                match e.last_state.as_str() {
                    "running" => ("执行中", theme::green()),
                    "waiting" => ("已激活", theme::accent()),
                    _ => ("已退出", theme::dim()),
                }
            };
            let when = super::detail_panel::fmt_artifact_time(&e.created_at, &now, &chrono::Local).unwrap_or_default();
            let ended = e
                .ended_at
                .as_deref()
                .and_then(|t| super::detail_panel::fmt_artifact_time(t, &now, &chrono::Local));
            let checklist = crate::model::parse_checklist(&e.summary);
            let progress = if checklist.is_empty() {
                String::new()
            } else {
                format!(" · {}/{} 完成", checklist.iter().filter(|i| i.done).count(), checklist.len())
            };
            let meta = format!(
                "{}{} · {}{}{}",
                e.project_name,
                if e.agent == "shell" { " · 终端" } else { "" },
                when,
                ended.map(|t| format!(" → {t}")).unwrap_or_default(),
                progress
            );
            let id = e.id.clone();
            let title = if e.title.is_empty() { e.project_name.clone() } else { e.title.clone() };
            list = list.child(
                div()
                    .id(SharedString::from(format!("hist:{}", e.id)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .when(openable, |el| {
                        el.cursor_pointer()
                            .hover(|st| st.bg(c(theme::surface_raised())))
                            .on_click(cx.listener(move |this, _, _, cx| this.open_session(id.clone(), cx)))
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(44.))
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(color))
                            .child(status),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(c(if e.deleted_at.is_some() { theme::dim() } else { theme::ink() }))
                                    .child(SharedString::from(title)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .font_family("Menlo")
                                    .text_size(px(10.))
                                    .text_color(c(theme::faint()))
                                    .child(SharedString::from(meta)),
                            ),
                    ),
            );
        }
        div()
            .id("history-scroll")
            .size_full()
            .overflow_y_scroll()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("历史"))
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .child(SharedString::from(format!("{} 条 · 含已退出、已删除", self.history.len()))),
                    ),
            )
            .child(list)
    }
}
