//! 总览页：会话卡片墙 + 最近输出预览（P1）。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::SessionState;
use crate::theme;

impl RootView {
    pub(super) fn render_overview(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let waiting = self
            .sessions
            .iter()
            .filter(|s| s.state == SessionState::Waiting)
            .count();

        let mut grid = div().flex().flex_wrap().gap(px(12.));
        for (ix, s) in self.sessions.iter().enumerate() {
            let id = s.id.clone();
            let is_waiting = s.state == SessionState::Waiting;
            let exited = s.state == SessionState::Exited;
            let sub = match s.state {
                SessionState::Waiting => format!("{} · 等待输入", s.project_name),
                SessionState::Running if self.stalled.contains_key(&s.id) => {
                    format!("{} · 运行中 · 可能空转", s.project_name)
                }
                SessionState::Running => format!("{} · 运行中", s.project_name),
                SessionState::Idle => format!("{} · 空闲", s.project_name),
                SessionState::Exited => format!(
                    "{} · 已退出{} · 回放已保留",
                    s.project_name,
                    s.exit_code.map(|c| format!(" (exit {c})")).unwrap_or_default()
                ),
            };
            let preview = if s.preview.is_empty() {
                match s.state {
                    SessionState::Exited => "进程已结束 · 点击查看回放".to_string(),
                    _ => "（暂无输出）".to_string(),
                }
            } else {
                s.preview.clone()
            };
            let agent_label: SharedString = if s.agent == "shell" {
                "终端".into()
            } else {
                s.agent.clone().into()
            };

            grid = grid.child(
                div()
                    .id(("ov-card", ix))
                    .w(px(420.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .bg(c(theme::SURFACE))
                    .border_1()
                    .border_color(if is_waiting {
                        c(theme::AMBER)
                    } else {
                        c(theme::EDGE)
                    })
                    .when(exited, |el| el.opacity(0.62))
                    .cursor_pointer()
                    .hover(|st| st.border_color(c(theme::CYAN)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id.clone(), cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(dot(theme::state_color(s.state.as_str())))
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_size(px(13.))
                                    .child(SharedString::from(s.display_title())),
                            )
                            .child(agent_chip(&s.agent, agent_label)),
                    )
                    .child(
                        div()
                            .mt(px(3.))
                            .text_size(px(11.))
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(sub)),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .p(px(8.))
                            .rounded(px(6.))
                            .bg(c(theme::TERM_BG))
                            .font_family("Menlo")
                            .text_size(px(10.5))
                            .text_color(c(theme::DIM))
                            .overflow_hidden()
                            .max_h(px(76.))
                            .child(SharedString::from(preview)),
                    ),
            );
        }

        div()
            .id("overview-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(12.))
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(16.))
                            .child("会话总览"),
                    )
                    .when(waiting > 0, |el| {
                        el.child(
                            div()
                                .font_family("Menlo")
                                .text_size(px(11.))
                                .text_color(c(theme::AMBER))
                                .child(SharedString::from(format!("· {waiting} 个等待输入"))),
                        )
                    }),
            )
            .child(grid)
            .child(
                div()
                    .mt(px(14.))
                    .font_family("Menlo")
                    .text_size(px(10.5))
                    .text_color(c(theme::FAINT))
                    .child("卡片实时刷新最近输出 · 等待输入的会话置顶并高亮"),
            )
    }
}
