//! 「看板」页（2026-09-07 用户拍板，第二版）：**所有会话的进度**，没有时间维度。
//! 数据是 daemon 一次算好的 `GET /history/dashboard`。
//!
//! 顶上是未完成条目数 + 搜索（2026-09-08 用户拍板：五态计数条和状态字跟着侧栏一起去掉——
//! 看板和项目列表要说同一套话）。主体是**瀑布流、全展开**（2026-09-07 第三版，用户要一眼
//! 看全）：一会话一张卡——在跑就一根蓝竖线、否则什么都没有 + 标题 + 项目，一根进度条 `done/total`，全部
//! 清单项直接列出（没勾的在前、做完的灰掉），不折叠；卡片按估算高度塞进最短的一
//! 列，列数随窗口宽度变。已删除的默认不显示，一个开关切出来。会话还在就能点开。
//!
//! 2026-09-08 用户报「看板里面东西太多了，要做一下分割」：分**两节**——「在 AAA 里」
//! （`alive`，还在池子里、点一下就进得去）铺开，「不在 AAA 里」（只剩记录）默认折起来
//! 只留一行表头，点开才铺；搜索框里有字时两节都展开，不然搜到的东西藏在折叠节里等于
//! 没搜到。同一天用户还要一根滑动条：全展开之后一屏根本装不下（见 [`super::scrollbar`]）。

use gpui::{Context, SharedString, div, prelude::*, px};

use super::RootView;
use super::kit::*;
use crate::model::{Dashboard, SessionCard, card_running, dashboard_split};
use crate::theme;

impl RootView {
    pub(super) fn open_history(&mut self, cx: &mut Context<Self>) {
        self.page = super::Page::History;
        self.fetch_history(cx);
        cx.notify();
    }

    /// 看板上点清单项 = 勾 / 取消勾：POST /sessions/:id/checklist，然后重拉看板
    pub(super) fn toggle_checklist(&mut self, id: String, index: usize, text: String, done: bool, cx: &mut Context<Self>) {
        let fut = self.net.session_checklist(&id, index, &text, done);
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

    /// 一张卡（全展开：所有清单项都列出来）
    fn card(&self, card: &SessionCard, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let total = card.done + card.open;
        let frac = if total == 0 { 0. } else { card.done as f32 / total as f32 };
        let id_open = card.id.clone();
        let alive = card.alive;
        let dimmed = card.deleted;
        let mut el = super::kit::card()
            .id(SharedString::from(format!("card:{}", card.id)))
            .flex()
            .flex_col()
            .gap(px(5.))
            .p(px(12.))
            .when(dimmed, |el| el.opacity(0.6))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    // 蓝竖线 = 还在跑；已删除的写一个字；其余什么都不画（和项目列表同一套话）
                    .when(card_running(card), |el| el.child(mark_bar(theme::blue()).mt(px(1.))))
                    .when(card.deleted, |el| {
                        el.child(
                            meta().flex_none()
                                .child("已删除"),
                        )
                    })
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
                meta()
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
        // 全部清单项：没勾的在前，做完的灰掉；会话还在池子里就能点着勾 / 取消勾。
        // 带着**原下标**一起走（`enumerate` 在重排之前）：POST 回去要说清是第几条，
        // 光给文字的话，清单里有两条一样的就会一起被翻过去。
        for (i, it) in card
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| !i.done)
            .chain(card.items.iter().enumerate().filter(|(_, i)| i.done))
        {
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
                                this.toggle_checklist(sid.clone(), i, text.clone(), !done, cx);
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

        // 两节：在 AAA 里（还在池子里）/ 不在 AAA 里（只剩记录）。搜索 + 已删除开关先筛过
        let (here, gone) = dashboard_split(&d.sessions, &query, self.dash_show_deleted);
        // 搜索时第二节强制展开：搜到的东西藏在折叠节里等于没搜到
        let gone_open = self.dash_show_gone || !query.trim().is_empty();

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
        // 节表头：一行小字 + 条数。颜色和 hover 都长在这一层——套一个外壳再在外壳上写
        // hover 是没用的，子元素自己的 text_color 会把父层的 hover 盖掉。
        let head = |label: String, clickable: bool| {
            div()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(c(theme::dim()))
                .when(clickable, |el| el.cursor_pointer().hover(|st| st.text_color(c(theme::ink()))))
                .child(SharedString::from(label))
        };

        let mut body = div().flex().flex_col().gap(px(12.)).pb(px(16.));
        if !here.is_empty() {
            // 只有一节时不写表头：「在 AAA 里 7」单独挂在那儿是句废话，它只在
            // 「和下面那节相对」时才有意义
            body = body
                .when(!gone.is_empty(), |el| el.child(head(format!("在 AAA 里 {}", here.len()), false)))
                .child(masonry(&here, cx));
        }
        if !gone.is_empty() {
            let arrow = if gone_open { "▾" } else { "▸" };
            body = body
                .child(
                    div()
                        .id("dash-gone")
                        .child(head(format!("{arrow} 不在 AAA 里 {}", gone.len()), true))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dash_show_gone = !this.dash_show_gone;
                            cx.notify();
                        })),
                )
                .when(gone_open, |el| el.child(masonry(&gone, cx)));
        }
        if here.is_empty() && gone.is_empty() {
            body = body.child(div().text_size(px(12.)).text_color(c(theme::faint())).child(if d.sessions.is_empty() {
                "还没有记录（daemon 每秒把会话同步进日志）"
            } else {
                "没有匹配的会话"
            }));
        }
        let deleted_n = d.sessions.iter().filter(|c| c.deleted).count();
        let open_items = div()
            .font_family("Menlo")
            .text_size(px(10.5))
            .text_color(c(theme::amber()))
            .child(SharedString::from(format!("未完成 {} 条", d.counts.open_items)));

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .child(div().text_size(px(14.)).font_weight(gpui::FontWeight::BOLD).text_color(c(theme::ink())).child("看板"))
                    .child(open_items)
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
            // 页边距留在滚动区里：滚动条那条命中带压着的就是这 16px，底下没有能点的东西
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(super::scrollbar::scroll_area("dash-scroll", &self.dash_scroll, div().px(px(16.)).child(body))),
            )
    }
}
