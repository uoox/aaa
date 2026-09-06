//! 「历史」页：daemon 会话日志（GET /history）+ 日历（GET /history/days）。
//! 上面是搜索框和月历（有会话的日子带数字，点一天只看那天，再点取消），
//! 中间是选中那天 haiku 写的「这一天做了什么」，下面是会话列表。
//! 点一行：会话还在（活着或有回放）就打开；已删除的只能看。

use chrono::Datelike as _;
use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::{DayDigest, HistoryEntry, history_matches, local_day_of, parse_checklist};
use crate::theme;

impl RootView {
    pub(super) fn open_history(&mut self, cx: &mut Context<Self>) {
        self.page = super::Page::History;
        self.fetch_history(cx);
        cx.notify();
    }

    pub(super) fn fetch_history(&mut self, cx: &mut Context<Self>) {
        let fut = self.net.history(500);
        self.spawn_fetch(
            fut,
            |r, entries: Vec<HistoryEntry>, cx| {
                r.history = entries;
                cx.notify();
            },
            true,
            cx,
        );
        let fut = self.net.history_days();
        self.spawn_fetch(
            fut,
            |r, days: Vec<DayDigest>, cx| {
                r.history_days = days;
                cx.notify();
            },
            false,
            cx,
        );
    }

    fn shift_month(&mut self, delta: i32, cx: &mut Context<Self>) {
        let (y, m) = parse_ym(&self.history_month);
        let idx = y * 12 + (m - 1) + delta;
        self.history_month = format!("{:04}-{:02}", idx.div_euclid(12), idx.rem_euclid(12) + 1);
        cx.notify();
    }

    pub(super) fn render_history_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let now = chrono::Local::now();
        let query = self.history_input.read(cx).text.clone();
        let alive_ids: std::collections::HashSet<&str> = self.sessions.iter().map(|s| s.id.as_str()).collect();
        let by_day: std::collections::HashMap<&str, &DayDigest> =
            self.history_days.iter().map(|d| (d.date.as_str(), d)).collect();

        // ── 月历 ──
        let (y, m) = parse_ym(&self.history_month);
        let first = chrono::NaiveDate::from_ymd_opt(y, m as u32, 1).unwrap_or_else(|| now.date_naive());
        let days_in_month = days_in(y, m);
        let lead = first.weekday().num_days_from_monday() as usize;
        let today = now.format("%Y-%m-%d").to_string();
        let mut grid = div().flex().flex_col().gap(px(3.));
        let mut row = div().flex().gap(px(3.));
        let cell = |content: gpui::Div| content.w(px(40.)).h(px(34.)).flex().flex_col().items_center().justify_center().rounded(px(6.));
        for w in ["一", "二", "三", "四", "五", "六", "日"] {
            row = row.child(cell(div()).child(div().text_size(px(10.)).text_color(c(theme::faint())).child(w)));
        }
        grid = grid.child(row);
        row = div().flex().gap(px(3.));
        for _ in 0..lead {
            row = row.child(cell(div()));
        }
        let mut col_ix = lead;
        for d in 1..=days_in_month {
            let date = format!("{:04}-{:02}-{:02}", y, m, d);
            let n = by_day.get(date.as_str()).map(|x| x.sessions).unwrap_or(0);
            let selected = self.history_day.as_deref() == Some(date.as_str());
            let is_today = date == today;
            let date2 = date.clone();
            let mut c_el = cell(div())
                .id(SharedString::from(format!("cal:{date}")))
                .when(n > 0, |el| {
                    el.cursor_pointer()
                        .hover(|st| st.bg(c(theme::surface_raised())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.history_day = if this.history_day.as_deref() == Some(date2.as_str()) { None } else { Some(date2.clone()) };
                            cx.notify();
                        }))
                })
                .when(selected, |el| el.bg(ca(theme::accent(), 0.18)).border_1().border_color(c(theme::accent())))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(c(if n > 0 { theme::ink() } else { theme::faint() }))
                        .when(is_today, |el| el.font_weight(gpui::FontWeight::BOLD).text_color(c(theme::accent())))
                        .child(SharedString::from(d.to_string())),
                );
            if n > 0 {
                c_el = c_el.child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(8.5))
                        .text_color(c(theme::green()))
                        .child(SharedString::from(format!("{n}"))),
                );
            }
            row = row.child(c_el);
            col_ix += 1;
            if col_ix % 7 == 0 {
                grid = grid.child(row);
                row = div().flex().gap(px(3.));
            }
        }
        if col_ix % 7 != 0 {
            grid = grid.child(row);
        }
        let nav_btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(6.))
                .py(px(2.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(12.))
                .text_color(c(theme::dim()))
                .hover(|st| st.bg(c(theme::surface_raised())).text_color(c(theme::ink())))
                .child(label)
        };
        let calendar = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(nav_btn("cal-prev", "‹").on_click(cx.listener(|this, _, _, cx| this.shift_month(-1, cx))))
                    .child(div().font_family("Menlo").text_size(px(12.)).text_color(c(theme::ink())).child(SharedString::from(self.history_month.clone())))
                    .child(nav_btn("cal-next", "›").on_click(cx.listener(|this, _, _, cx| this.shift_month(1, cx)))),
            )
            .child(grid);

        // ── 选中那天的摘要 ──
        let digest = self.history_day.as_deref().and_then(|d| by_day.get(d)).map(|d| {
            let body = if d.text.is_empty() {
                "haiku 还没写这一天的摘要（daemon 每 5 分钟补一次）".to_string()
            } else {
                d.text.clone()
            };
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(12.))
                .rounded(px(8.))
                .bg(c(theme::surface()))
                .border_1()
                .border_color(c(theme::edge()))
                .child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(10.))
                        .text_color(c(theme::faint()))
                        .child(SharedString::from(format!("{} · {} 个会话", d.date, d.sessions))),
                )
                .child(div().text_size(px(12.5)).text_color(c(theme::ink())).whitespace_normal().child(SharedString::from(body)))
        });

        // ── 会话列表（按搜索 + 选中日期过滤）──
        let mut list = div().flex().flex_col().gap(px(2.)).w_full();
        let filtered: Vec<&HistoryEntry> = self
            .history
            .iter()
            .filter(|e| history_matches(e, &query))
            .filter(|e| self.history_day.as_deref().is_none_or(|d| local_day_of(&e.created_at).as_deref() == Some(d)))
            .collect();
        if filtered.is_empty() {
            list = list.child(div().text_size(px(12.)).text_color(c(theme::faint())).child(if self.history.is_empty() {
                "还没有记录（daemon 每秒把会话同步进日志）"
            } else {
                "没有匹配的会话"
            }));
        }
        for e in filtered {
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
            let ended = e.ended_at.as_deref().and_then(|t| super::detail_panel::fmt_artifact_time(t, &now, &chrono::Local));
            let checklist = parse_checklist(&e.summary);
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
                    .child(div().flex_none().w(px(44.)).font_family("Menlo").text_size(px(10.)).text_color(c(color)).child(status))
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
                            .child(div().truncate().font_family("Menlo").text_size(px(10.)).text_color(c(theme::faint())).child(SharedString::from(meta))),
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
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("历史"))
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .child(SharedString::from(format!("{} 条 · 含已退出、已删除", self.history.len()))),
                    )
                    .child(div().flex_1())
                    .child(div().w(px(260.)).child(self.history_input.clone())),
            )
            .child(calendar)
            .when_some(digest, |el, d| el.child(d))
            .child(list)
    }
}

fn parse_ym(s: &str) -> (i32, i32) {
    let mut it = s.split('-');
    let y = it.next().and_then(|v| v.parse().ok()).unwrap_or(2026);
    let m = it.next().and_then(|v| v.parse().ok()).unwrap_or(1);
    (y, m)
}

fn days_in(y: i32, m: i32) -> u32 {
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    let next = chrono::NaiveDate::from_ymd_opt(ny, nm as u32, 1).unwrap();
    let this = chrono::NaiveDate::from_ymd_opt(y, m as u32, 1).unwrap();
    (next - this).num_days() as u32
}
