//! 「看板」页（2026-09-07 用户拍板，第二版）：**所有会话的进度**，没有时间维度。
//! 数据是 daemon 一次算好的 `GET /history/dashboard`。
//!
//! 顶上一条计数（待回复 / 运行 / 后台 / 激活 / 暂停，点一个 = 只看那一组；再点取消）
//! + 未完成条目数 + 搜索。主体是**瀑布流、全展开**（2026-09-07 第三版，用户要一眼看全）：
//! 一会话一张卡——状态字 + 标题 + 项目，一根进度条 `done/total`，全部清单项直接列出
//! （没勾的在前、做完的灰掉），不折叠、不分组；卡片按估算高度塞进最短的一列，列数随
//! 窗口宽度变。已删除 / 已归档的默认不显示，各一个开关切出来。会话还在就能点开。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::{Dashboard, SessionCard, card_matches, status_label};
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

    /// 看板上点清单项 = 勾 / 取消勾：POST /sessions/:id/checklist，然后重拉看板
    pub(super) fn toggle_checklist(&mut self, id: String, text: String, done: bool, cx: &mut Context<Self>) {
        let fut = self.net.session_checklist(&id, &text, done);
        self.spawn_fetch(fut, |r, _: serde_json::Value, cx| r.fetch_history(cx), true, cx);
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

    /// 一张卡（全展开：所有清单项都列出来）
    fn card(&self, card: &SessionCard, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let total = card.done + card.open;
        let frac = if total == 0 { 0. } else { card.done as f32 / total as f32 };
        let id_open = card.id.clone();
        let alive = card.alive;
        let color = status_color(&card.status);
        let dimmed = card.deleted || card.archived;
        let mut el = div()
            .id(SharedString::from(format!("card:{}", card.id)))
            .flex()
            .flex_col()
            .gap(px(5.))
            .p(px(12.))
            .rounded(px(10.))
            .bg(c(theme::surface()))
            .border_1()
            .border_color(c(theme::edge()))
            .when(dimmed, |el| el.opacity(0.6))
            .child(
                div()
                    .flex()
                    .items_start()
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
                            .child(if card.deleted { "已删除" } else if card.archived { "归档" } else { status_label(&card.status) }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(13.))
                            .whitespace_normal()
                            .text_color(c(if dimmed { theme::dim() } else { theme::ink() }))
                            .child(SharedString::from(card.title.clone())),
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
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(10.))
                    .text_color(c(theme::faint()))
                    .child(SharedString::from(card.project_name.clone())),
            );
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
        // 全部清单项：没勾的在前，做完的灰掉；会话还在池子里就能点着勾 / 取消勾
        for (i, it) in card.items.iter().filter(|i| !i.done).chain(card.items.iter().filter(|i| i.done)).enumerate() {
            let (sid, text, done) = (card.id.clone(), it.text.clone(), it.done);
            el = el.child(
                div()
                    .id(SharedString::from(format!("card-item:{}:{i}", card.id)))
                    .flex()
                    .gap(px(6.))
                    .items_start()
                    .when(alive, |row| {
                        row.cursor_pointer()
                            .hover(|st| st.bg(ca(theme::accent(), 0.08)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_checklist(sid.clone(), text.clone(), !done, cx);
                            }))
                    })
                    .child(div().flex_none().text_size(px(11.5)).text_color(c(if it.done { theme::green() } else { theme::amber() })).child(if it.done { "☑" } else { "☐" }))
                    .child(div().text_size(px(12.)).text_color(c(if it.done { theme::dim() } else { theme::ink() })).whitespace_normal().child(SharedString::from(it.text.clone()))),
            );
        }
        el
    }

    pub(super) fn render_history_page(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.history_input.read(cx).text().to_string();
        let d = &self.dashboard;

        // 过滤：搜索 → 状态组 → 已删除开关；归档的单独一节（2026-09-07 用户：和还在列表里的分开）
        let (archived_cards, matched): (Vec<&SessionCard>, Vec<&SessionCard>) = d
            .sessions
            .iter()
            .filter(|c| card_matches(c, &query))
            .filter(|c| self.dash_filter.as_deref().is_none_or(|f| c.status == f))
            .filter(|c| self.dash_show_deleted || !c.deleted)
            .partition(|c| c.archived);

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

        // 瀑布流：列数随窗口宽度（侧栏约 260px，卡片最窄 300px），卡片按估算高度
        // （标题 + 进度条 + 每项一行）塞进当前最短的一列，高矮不一才不浪费竖向空间
        let cols_n = (((self.win_w - 260.) / 320.).floor() as usize).clamp(1, 6);
        let masonry = |cards: &[&SessionCard], cx: &mut Context<Self>| -> gpui::Div {
            let mut cols: Vec<(f32, Vec<&SessionCard>)> = (0..cols_n).map(|_| (0., Vec::new())).collect();
            for card in cards {
                let h = 70. + 18. * card.items.len() as f32;
                let (i, _) = cols.iter().enumerate().min_by(|a, b| a.1.0.partial_cmp(&b.1.0).unwrap()).unwrap();
                cols[i].0 += h;
                cols[i].1.push(card);
            }
            let mut grid = div().flex().items_start().gap(px(8.));
            for (_, col) in &cols {
                let mut col_el = div().flex_1().min_w(px(0.)).flex().flex_col().gap(px(8.));
                for card in col {
                    col_el = col_el.child(self.card(card, cx));
                }
                grid = grid.child(col_el);
            }
            grid
        };
        let mut grid = div().flex().flex_col().gap(px(10.)).child(masonry(&matched, cx));
        if matched.is_empty() {
            grid = grid.child(div().text_size(px(12.)).text_color(c(theme::faint())).child(if d.sessions.is_empty() {
                "还没有记录（daemon 每秒把会话同步进日志）"
            } else {
                "没有匹配的会话"
            }));
        }
        // 归档的：单独一节，默认收着，点开是自己的一片瀑布流
        if !archived_cards.is_empty() {
            grid = grid.child(
                div()
                    .id("dash-archived-section")
                    .mt(px(6.))
                    .py(px(6.))
                    .border_t_1()
                    .border_color(c(theme::edge()))
                    .cursor_pointer()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(c(if self.dash_show_archived { theme::accent() } else { theme::dim() }))
                    .hover(|st| st.text_color(c(theme::ink())))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dash_show_archived = !this.dash_show_archived;
                        cx.notify();
                    }))
                    .child(SharedString::from(format!("{} 归档 {}（不在项目列表里的）", if self.dash_show_archived { "▾" } else { "▸" }, archived_cards.len()))),
            );
            if self.dash_show_archived {
                grid = grid.child(masonry(&archived_cards, cx));
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
            .child(div().id("dash-scroll").flex_1().min_h(px(0.)).overflow_y_scroll().child(grid))
    }
}
