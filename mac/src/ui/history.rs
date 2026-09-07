//! 「看板」页（2026-09-07 用户拍板，取代原来的历史两 tab）：一眼看出**最近做了什么**、
//! **还有什么没做**。数据是 daemon 一次算好的 `GET /history/dashboard`。
//!
//! 顶上三块数字（今天 / 近 7 天 / 此刻）+ 近 8 周的活动热力条；下面两栏——左边把所有
//! 会话里没勾的清单项按项目归并成一张待办表，右边按天倒序的流水（haiku 写的「这一天
//! 做了什么」+ 当天会话）。搜索框两栏共用。会话还在就能点开，已删除的只能看。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::{DayCard, Dashboard, OpenItem, day_card_matches, group_open_by_project, open_item_matches};
use crate::theme;

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

    /// 数字块：大字 + 小标题；副行是「做完 / 没做」
    fn stat_tile(&self, label: &str, big: String, sub: String) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(12.))
            .py(px(8.))
            .min_w(px(120.))
            .rounded(px(10.))
            .bg(c(theme::surface()))
            .border_1()
            .border_color(c(theme::edge()))
            .child(div().text_size(px(10.5)).text_color(c(theme::faint())).child(SharedString::from(label.to_string())))
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(20.))
                    .text_color(c(theme::ink()))
                    .child(SharedString::from(big)),
            )
            .child(div().font_family("Menlo").text_size(px(10.5)).text_color(c(theme::dim())).child(SharedString::from(sub)))
    }

    /// 近 8 周的活动热力条：一格一天，最旧在左，颜色深浅按当天会话数
    fn spark_strip(&self) -> gpui::Div {
        let spark = &self.dashboard.spark;
        let max = spark.iter().copied().max().unwrap_or(0).max(1);
        let mut row = div().flex().gap(px(2.)).items_end();
        for n in spark {
            let alpha = if *n == 0 { 0.10 } else { 0.25 + 0.75 * (*n as f32 / max as f32) };
            row = row.child(div().w(px(6.)).h(px(14.)).rounded(px(2.)).bg(ca(theme::accent(), alpha)));
        }
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().font_family("Menlo").text_size(px(10.)).text_color(c(theme::faint())).child("8 周"))
            .child(row)
    }

    /// 左栏一条待办
    fn open_row(&self, it: &OpenItem, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let now = chrono::Local::now();
        let when = super::detail_panel::fmt_artifact_time(&it.created_at, &now, &chrono::Local).unwrap_or_default();
        let sid = it.session_id.clone();
        let alive = it.alive;
        let running = it.running;
        div()
            .id(SharedString::from(format!("todo:{}:{}", it.session_id, it.text)))
            .flex()
            .items_start()
            .gap(px(8.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .when(alive, |el| {
                el.cursor_pointer().hover(|st| st.bg(c(theme::surface_raised()))).on_click(cx.listener(move |this, _, _, cx| {
                    this.open_session(sid.clone(), cx);
                }))
            })
            .child(div().flex_none().text_size(px(12.)).text_color(c(theme::amber())).child("☐"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(12.5)).text_color(c(theme::ink())).whitespace_normal().child(SharedString::from(it.text.clone())))
                    .child(
                        div()
                            .truncate()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .child(SharedString::from(format!("{} · {}", it.title, when))),
                    ),
            )
            .when(running, |el| {
                el.child(div().flex_none().text_size(px(10.)).text_color(c(theme::green())).child("● 在跑"))
            })
    }

    /// 右栏一天
    fn day_card(&self, d: &DayCard, today: &str, cx: &mut Context<Self>) -> gpui::Div {
        let expanded = self.history_expanded.contains(&d.date);
        let date_key = d.date.clone();
        let head = format!("{}{}", d.date, if d.date == today { "（今天）" } else { "" });
        let counts = format!("{} 会话 · 做完 {} · 没做 {}", d.sessions, d.done, d.open);
        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(c(theme::surface()))
            .border_1()
            .border_color(c(theme::edge()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(12.))
                            .text_color(c(if d.date == today { theme::accent() } else { theme::ink() }))
                            .child(SharedString::from(head)),
                    )
                    .child(div().flex_1())
                    .child(div().font_family("Menlo").text_size(px(10.)).text_color(c(theme::faint())).child(SharedString::from(counts))),
            );
        if d.text.is_empty() {
            card = card.child(
                div()
                    .text_size(px(11.5))
                    .text_color(c(theme::faint()))
                    .child("haiku 还没写这一天的摘要（daemon 每 5 分钟补一次）"),
            );
        } else {
            for line in d.text.lines() {
                let body = line.trim_start_matches(['-', '*', '•']).trim().to_string();
                if body.is_empty() {
                    continue;
                }
                card = card.child(
                    div()
                        .flex()
                        .gap(px(6.))
                        .child(div().flex_none().text_size(px(11.)).text_color(c(theme::green())).child("▪"))
                        .child(div().text_size(px(12.5)).text_color(c(theme::ink())).whitespace_normal().child(SharedString::from(body))),
                );
            }
        }
        card = card.child(
            div()
                .id(SharedString::from(format!("day-toggle:{}", d.date)))
                .cursor_pointer()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(c(theme::dim()))
                .hover(|st| st.text_color(c(theme::ink())))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.history_expanded.remove(&date_key) {
                        this.history_expanded.insert(date_key.clone());
                    }
                    cx.notify();
                }))
                .child(SharedString::from(format!("{} 这天的会话 {}", if expanded { "▾" } else { "▸" }, d.sessions))),
        );
        if expanded {
            for e in &d.entries {
                let sid = e.id.clone();
                let alive = e.alive;
                let running = e.running;
                let tail = if e.deleted {
                    "已删除".to_string()
                } else if e.done + e.open == 0 {
                    String::new()
                } else {
                    format!("{}/{}", e.done, e.done + e.open)
                };
                card = card.child(
                    div()
                        .id(SharedString::from(format!("day-sess:{}", e.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .when(alive, |el| {
                            el.cursor_pointer().hover(|st| st.bg(c(theme::surface_raised()))).on_click(cx.listener(move |this, _, _, cx| {
                                this.open_session(sid.clone(), cx);
                            }))
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.))
                                .text_color(c(if e.deleted {
                                    theme::faint()
                                } else if running {
                                    theme::green()
                                } else if e.open > 0 {
                                    theme::amber()
                                } else {
                                    theme::dim()
                                }))
                                .child(if e.deleted {
                                    "✕"
                                } else if running {
                                    "◐"
                                } else if e.open > 0 {
                                    "☐"
                                } else {
                                    "☑"
                                }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .truncate()
                                .text_size(px(12.))
                                .text_color(c(if e.deleted { theme::dim() } else { theme::ink() }))
                                .child(SharedString::from(e.title.clone())),
                        )
                        .child(
                            div()
                                .flex_none()
                                .font_family("Menlo")
                                .text_size(px(10.))
                                .text_color(c(theme::faint()))
                                .child(SharedString::from(format!("{} {}", e.project_name, tail))),
                        ),
                );
            }
        }
        card
    }

    pub(super) fn render_history_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.history_input.read(cx).text.clone();
        let d = &self.dashboard;
        // 「（今天）」按 daemon 的日期画：客户端与 daemon 不在一个时区时才不会标错天
        let today = d.date.clone();

        let open_items: Vec<&OpenItem> = d.open.iter().filter(|i| open_item_matches(i, &query)).collect();
        let owned: Vec<OpenItem> = open_items.iter().map(|i| (*i).clone()).collect();
        let groups = group_open_by_project(&owned);

        // ── 左栏：还没做 ──
        let mut left = div().flex().flex_col().gap(px(10.));
        left = left.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("还没做"))
                .child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(10.5))
                        .text_color(c(theme::amber()))
                        .child(SharedString::from(open_items.len().to_string())),
                ),
        );
        if groups.is_empty() {
            left = left.child(
                div()
                    .text_size(px(12.))
                    .text_color(c(theme::faint()))
                    .child(if d.open.is_empty() { "没有待办——每个会话的进度清单都勾完了" } else { "没有匹配的待办" }),
            );
        }
        for (project, items) in &groups {
            let mut sec = div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .p(px(8.))
                .rounded(px(10.))
                .bg(c(theme::surface()))
                .border_1()
                .border_color(c(theme::edge()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(10.))
                        .pb(px(4.))
                        .child(div().text_size(px(12.)).text_color(c(theme::dim())).child(SharedString::from(project.clone())))
                        .child(div().flex_1())
                        .child(
                            div()
                                .font_family("Menlo")
                                .text_size(px(10.))
                                .text_color(c(theme::faint()))
                                .child(SharedString::from(items.len().to_string())),
                        ),
                );
            for it in items {
                sec = sec.child(self.open_row(it, cx));
            }
            left = left.child(sec);
        }

        // ── 右栏：最近做了什么 ──
        let days: Vec<&DayCard> = d.days.iter().filter(|x| day_card_matches(x, &query)).collect();
        let mut right = div().flex().flex_col().gap(px(10.));
        right = right.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().text_size(px(13.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("最近做了什么"))
                .child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(10.5))
                        .text_color(c(theme::faint()))
                        .child(SharedString::from(format!("{} 天", days.len()))),
                ),
        );
        if days.is_empty() {
            right = right.child(
                div()
                    .text_size(px(12.))
                    .text_color(c(theme::faint()))
                    .child(if d.days.is_empty() { "还没有记录（daemon 每秒把会话同步进日志）" } else { "没有匹配的日子" }),
            );
        }
        for day in days {
            right = right.child(self.day_card(day, &today, cx));
        }

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
                    .child(self.spark_strip())
                    .child(div().flex_1())
                    .child(div().w(px(240.)).child(self.history_input.clone())),
            )
            .child(
                div()
                    .flex()
                    .gap(px(10.))
                    .child(self.stat_tile("今天", format!("{} 会话", d.today.sessions), format!("做完 {} · 没做 {}", d.today.done, d.today.open)))
                    .child(self.stat_tile("近 7 天", format!("{} 会话", d.week.sessions), format!("做完 {} · 没做 {}", d.week.done, d.week.open)))
                    .child(self.stat_tile("此刻", format!("{} 在跑", d.active), format!("待办共 {} 条", d.open.len()))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .gap(px(12.))
                    .items_start()
                    .child(div().id("dash-todo").w(px(340.)).flex_none().h_full().overflow_y_scroll().child(left))
                    .child(div().id("dash-days").flex_1().min_w(px(0.)).h_full().overflow_y_scroll().child(right)),
            )
    }
}
