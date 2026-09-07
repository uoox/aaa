//! 「看板」页（2026-09-07 用户拍板，第二版）：**所有会话的进度**，没有时间维度。
//! 数据是 daemon 一次算好的 `GET /history/dashboard`。
//!
//! 顶上一条计数（待回复 / 运行 / 后台 / 激活 / 暂停，点一个 = 只看那一组；再点取消）
//! + 未完成条目数 + 搜索。主体一会话一张卡：状态字 + 标题 + 项目，一根进度条
//! `done/total`，下面直接列没勾的项，做完的折成一行「已做 N」点开看。已完成的（暂停
//! 且全勾完）默认收进「已完成 N」一组；已删除的默认不显示，一个开关切出来。
//! 会话还在就能点开，已退出的只能看。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::{Dashboard, SessionCard, card_is_finished, card_matches, status_label};
use crate::theme;

const STATUSES: [&str; 5] = ["asking", "running", "background", "active", "paused"];

fn status_color(status: &str) -> u32 {
    match status {
        "asking" => theme::amber(),
        "running" | "background" => theme::green(),
        "active" => theme::accent(),
        _ => theme::faint(),
    }
}

impl RootView {
    pub(super) fn open_history(&mut self, cx: &mut Context<Self>) {
        self.page = super::Page::History;
        self.fetch_history(cx);
        cx.notify();
    }

    pub(super) fn fetch_history(&mut self, cx: &mut Context<Self>) {
        let fut = self.net.history_dashboard();
        self.spawn_fetch(
            fut,
            |r, d: Dashboard, cx| {
                r.dashboard = d;
                cx.notify();
            },
            true,
            cx,
        );
    }

    /// 顶上的计数块：点一下只看这一组，再点取消
    fn count_chip(&self, status: &'static str, n: usize, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let on = self.dash_filter.as_deref() == Some(status);
        div()
            .id(SharedString::from(format!("dash-chip:{status}")))
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(8.))
            .cursor_pointer()
            .border_1()
            .border_color(c(if on { status_color(status) } else { theme::edge() }))
            .when(on, |el| el.bg(ca(status_color(status), 0.14)))
            .hover(|st| st.bg(c(theme::surface_raised())))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.dash_filter = if this.dash_filter.as_deref() == Some(status) { None } else { Some(status.to_string()) };
                cx.notify();
            }))
            .child(div().text_size(px(11.5)).text_color(c(status_color(status))).child(status_label(status)))
            .child(div().font_family("Menlo").text_size(px(13.)).text_color(c(theme::ink())).child(SharedString::from(n.to_string())))
    }

    /// 一张卡
    fn card(&self, card: &SessionCard, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let total = card.done + card.open;
        let frac = if total == 0 { 0. } else { card.done as f32 / total as f32 };
        let done_open = self.history_expanded.contains(&card.id);
        let id_toggle = card.id.clone();
        let id_open = card.id.clone();
        let alive = card.alive;
        let color = status_color(&card.status);
        let mut el = div()
            .id(SharedString::from(format!("card:{}", card.id)))
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(c(theme::surface()))
            .border_1()
            .border_color(c(theme::edge()))
            .when(card.deleted, |el| el.opacity(0.6))
            // 标题行：状态字 + 标题 + 项目 +「打开」
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(ca(color, 0.7))
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(color))
                            .child(if card.deleted { "已删除" } else { status_label(&card.status) }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(13.))
                            .text_color(c(if card.deleted { theme::dim() } else { theme::ink() }))
                            .child(SharedString::from(card.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .child(SharedString::from(card.project_name.clone())),
                    )
                    .when(alive, |el| {
                        el.child(
                            div()
                                .id(SharedString::from(format!("card-open:{}", card.id)))
                                .flex_none()
                                .px(px(6.))
                                .py(px(2.))
                                .rounded(px(4.))
                                .text_size(px(10.5))
                                .text_color(c(theme::accent()))
                                .cursor_pointer()
                                .hover(|st| st.bg(c(theme::edge_light())))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_session(id_open.clone(), cx);
                                }))
                                .child("打开"),
                        )
                    }),
            );
        // 进度条 + 数字
        if total > 0 {
            el = el.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(c(theme::edge()))
                            .child(div().h_full().w(gpui::relative(frac)).rounded(px(2.)).bg(c(if card.open == 0 { theme::green() } else { theme::accent() }))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::dim()))
                            .child(SharedString::from(format!("{}/{}", card.done, total))),
                    ),
            );
        } else {
            el = el.child(div().text_size(px(11.)).text_color(c(theme::faint())).child("没有进度清单"));
        }
        // 没勾的项
        for it in card.items.iter().filter(|i| !i.done) {
            el = el.child(
                div()
                    .flex()
                    .gap(px(6.))
                    .items_start()
                    .child(div().flex_none().text_size(px(11.5)).text_color(c(theme::amber())).child("☐"))
                    .child(div().text_size(px(12.)).text_color(c(theme::ink())).whitespace_normal().child(SharedString::from(it.text.clone()))),
            );
        }
        // 做完的折成一行
        if card.done > 0 {
            el = el.child(
                div()
                    .id(SharedString::from(format!("card-done:{}", card.id)))
                    .cursor_pointer()
                    .font_family("Menlo")
                    .text_size(px(10.5))
                    .text_color(c(theme::dim()))
                    .hover(|st| st.text_color(c(theme::ink())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.history_expanded.remove(&id_toggle) {
                            this.history_expanded.insert(id_toggle.clone());
                        }
                        cx.notify();
                    }))
                    .child(SharedString::from(format!("{} 已做 {}", if done_open { "▾" } else { "▸" }, card.done))),
            );
            if done_open {
                for it in card.items.iter().filter(|i| i.done) {
                    el = el.child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .items_start()
                            .child(div().flex_none().text_size(px(11.5)).text_color(c(theme::green())).child("☑"))
                            .child(div().text_size(px(12.)).text_color(c(theme::dim())).whitespace_normal().child(SharedString::from(it.text.clone()))),
                    );
                }
            }
        }
        el
    }

    pub(super) fn render_history_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.history_input.read(cx).text().to_string();
        let d = &self.dashboard;

        // 过滤：搜索 → 状态组 → 已删除开关
        let matched: Vec<&SessionCard> = d
            .sessions
            .iter()
            .filter(|c| card_matches(c, &query))
            .filter(|c| self.dash_filter.as_deref().is_none_or(|f| c.status == f))
            .filter(|c| self.dash_show_deleted || !c.deleted)
            .collect();
        let (finished, active): (Vec<&SessionCard>, Vec<&SessionCard>) = matched.iter().partition(|c| card_is_finished(c) && !c.deleted);

        let toggle = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(3.))
                .rounded(px(6.))
                .cursor_pointer()
                .text_size(px(11.))
                .text_color(c(if on { theme::accent() } else { theme::dim() }))
                .when(on, |el| el.bg(ca(theme::accent(), 0.14)))
                .hover(|st| st.bg(c(theme::surface_raised())))
                .child(SharedString::from(label))
        };

        let mut list = div().flex().flex_col().gap(px(8.));
        if active.is_empty() && finished.is_empty() {
            list = list.child(div().text_size(px(12.)).text_color(c(theme::faint())).child(if d.sessions.is_empty() {
                "还没有记录（daemon 每秒把会话同步进日志）"
            } else {
                "没有匹配的会话"
            }));
        }
        for card in &active {
            list = list.child(self.card(card, cx));
        }
        if !finished.is_empty() {
            list = list.child(
                toggle("dash-finished", format!("{} 已完成 {}", if self.dash_show_finished { "▾" } else { "▸" }, finished.len()), self.dash_show_finished)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dash_show_finished = !this.dash_show_finished;
                        cx.notify();
                    })),
            );
            if self.dash_show_finished {
                for card in &finished {
                    list = list.child(self.card(card, cx));
                }
            }
        }

        let deleted_n = d.sessions.iter().filter(|c| c.deleted).count();
        let mut chips = div().flex().items_center().gap(px(6.)).flex_wrap();
        for st in STATUSES {
            let n = match st {
                "asking" => d.counts.asking,
                "running" => d.counts.running,
                "background" => d.counts.background,
                "active" => d.counts.active,
                _ => d.counts.paused,
            };
            chips = chips.child(self.count_chip(st, n, cx));
        }
        chips = chips.child(
            div()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(c(theme::amber()))
                .pl(px(6.))
                .child(SharedString::from(format!("未完成 {} 条", d.counts.open_items))),
        );

        div()
            .size_full()
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("看板"))
                    .child(chips)
                    .child(div().flex_1())
                    .when(deleted_n > 0, |el| {
                        el.child(
                            toggle("dash-deleted", format!("已删除 {}", deleted_n), self.dash_show_deleted).on_click(cx.listener(|this, _, _, cx| {
                                this.dash_show_deleted = !this.dash_show_deleted;
                                cx.notify();
                            })),
                        )
                    })
                    .child(div().w(px(220.)).child(self.history_input.clone())),
            )
            .child(div().id("dash-scroll").flex_1().min_h(px(0.)).overflow_y_scroll().child(list))
    }
}
