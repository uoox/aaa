//! 项目表格页：列与 aaa CLI 项目表一致（# / 会话命名 / 上下文 / 目录 / 名称 / agent / 修改时间）。
//! 行双击 resume 打开；工具栏 新建 / 打开 / 换 agent / 删除；搜索过滤。

use gpui::{Context, MouseButton, SharedString, div, prelude::*, px};

use super::kit::*;
use super::{Modal, RootView};
use crate::theme;

fn fmt_mtime(iso: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => iso.chars().take(16).collect::<String>().replace('T', " "),
    }
}

impl RootView {
    pub(super) fn render_projects(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.search_input.read(cx).text.trim().to_lowercase();
        let filtered: Vec<(usize, crate::model::Project)> = self
            .projects
            .iter()
            .filter(|p| {
                query.is_empty()
                    || p.name.to_lowercase().contains(&query)
                    || p.session_title
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
            })
            .cloned()
            .enumerate()
            .collect();

        // ── 工具栏 ──────────────────────────────────────────────────────
        let sel_first = self
            .selected_paths
            .iter()
            .next()
            .and_then(|p| self.projects.iter().find(|x| &x.path == p))
            .cloned();
        let sel_paths: Vec<String> = self.selected_paths.iter().cloned().collect();
        let has_sel = !sel_paths.is_empty();

        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .flex_none()
            .border_b_1()
            .border_color(c(theme::EDGE))
            .child(
                btn_primary("pj-new", "＋ 新建").on_click(cx.listener(|this, _, window, cx| {
                    this.open_new_project_modal(window, cx);
                })),
            )
            .child(tbtn("pj-open", "▶ 打开").map(|el| {
                let sel = sel_first.clone();
                el.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = &sel {
                        this.open_project(p, cx);
                    } else {
                        this.set_error("先选中一行".into(), cx);
                    }
                }))
            }))
            .child(tbtn("pj-agent", "⚙ 换 agent").map(|el| {
                let sel = sel_first.clone();
                el.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(p) = &sel {
                        this.modal = Modal::AgentPick {
                            path: p.path.clone(),
                        };
                        cx.notify();
                    } else {
                        this.set_error("先选中一行".into(), cx);
                    }
                }))
            }))
            .child(
                tbtn("pj-del", "🗑 删除").map(|el| {
                    let paths = sel_paths.clone();
                    el.text_color(c(theme::RED))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if paths.is_empty() {
                                this.set_error("先选中要删除的项目".into(), cx);
                            } else {
                                this.modal = Modal::DeleteConfirm {
                                    paths: paths.clone(),
                                    report: None,
                                    busy: false,
                                };
                                cx.notify();
                            }
                        }))
                }),
            )
            .child(div().flex_1())
            .child(div().w(px(220.)).child(self.search_input.clone()))
            .when(has_sel, |el| {
                el.child(
                    div()
                        .text_size(px(11.))
                        .font_family("Menlo")
                        .text_color(c(theme::FAINT))
                        .child(SharedString::from(format!("已选 {}", sel_paths.len()))),
                )
            });

        // ── 表头 ────────────────────────────────────────────────────────
        let th = |w: f32, grow: bool, align_right: bool, text: &'static str| {
            let mut d = div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(10.5))
                .font_family("Menlo")
                .text_color(c(theme::FAINT))
                .whitespace_nowrap();
            if grow {
                d = d.flex_1().min_w(px(0.));
            } else {
                d = d.w(px(w)).flex_none();
            }
            if align_right {
                d = d.text_right();
            }
            d.child(text)
        };
        let header = div()
            .flex()
            .flex_none()
            .border_b_1()
            .border_color(c(theme::EDGE))
            .bg(c(theme::SURFACE))
            .child(th(34., false, true, "#"))
            .child(th(0., true, false, "会话命名"))
            .child(th(64., false, true, "上下文"))
            .child(th(64., false, true, "目录"))
            .child(th(150., false, false, "名称"))
            .child(th(84., false, false, "agent"))
            .child(th(120., false, true, "修改时间"));

        // ── 行 ──────────────────────────────────────────────────────────
        let mut body = div().flex().flex_col();
        for (ix, p) in &filtered {
            let selected = self.selected_paths.contains(&p.path);
            let path = p.path.clone();
            let path2 = p.path.clone();
            let proj = p.clone();
            let td = |w: f32, grow: bool, right: bool| {
                let mut d = div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(12.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap();
                if grow {
                    d = d.flex_1().min_w(px(0.));
                } else {
                    d = d.w(px(w)).flex_none();
                }
                if right {
                    d = d.text_right().font_family("Menlo").text_size(px(11.));
                }
                d
            };
            body = body.child(
                div()
                    .id(("pj-row", *ix))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .when(selected, |el| el.bg(ca(theme::CYAN, 0.10)))
                    .when(!selected, |el| el.hover(|st| st.bg(c(theme::SURFACE))))
                    .border_b_1()
                    .border_color(ca(theme::EDGE, 0.5))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, ev: &gpui::MouseDownEvent, _, cx| {
                            if ev.modifiers.platform {
                                if !this.selected_paths.insert(path.clone()) {
                                    this.selected_paths.remove(&path);
                                }
                            } else {
                                this.selected_paths.clear();
                                this.selected_paths.insert(path.clone());
                            }
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            this.selected_paths.clear();
                            this.selected_paths.insert(path2.clone());
                            this.open_project(&proj, cx);
                        }
                    }))
                    .child(
                        td(34., false, true)
                            .text_color(c(theme::FAINT))
                            .child(SharedString::from((*ix + 1).to_string())),
                    )
                    .child(
                        td(0., true, false)
                            .text_color(c(theme::INK))
                            .child(SharedString::from(
                                p.session_title.clone().unwrap_or_else(|| "—".into()),
                            )),
                    )
                    .child(
                        td(64., false, true)
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(if p.ctx_size == 0 {
                                "—".to_string()
                            } else {
                                theme::human_bytes(p.ctx_size)
                            })),
                    )
                    .child(
                        td(64., false, true)
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(theme::human_bytes(p.dir_size))),
                    )
                    .child(
                        td(150., false, false)
                            .text_color(c(theme::CYAN))
                            .child(SharedString::from(p.name.clone())),
                    )
                    .child(td(84., false, false).map(|el| match &p.agent {
                        Some(a) if a != "shell" => {
                            el.child(agent_chip(a, SharedString::from(a.clone())))
                        }
                        Some(_) => el.child(agent_chip("shell", "终端")),
                        None => el
                            .text_color(c(theme::FAINT))
                            .child("—"),
                    }))
                    .child(
                        td(120., false, true)
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(fmt_mtime(&p.mtime))),
                    ),
            );
        }

        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .child(toolbar)
            .child(header)
            .child(
                div()
                    .id("pj-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(body)
                    .when(filtered.is_empty(), |el| {
                        el.child(
                            div()
                                .p(px(24.))
                                .text_color(c(theme::FAINT))
                                .text_size(px(12.5))
                                .child(if self.projects.is_empty() {
                                    "暂无项目（或 daemon 未连接）"
                                } else {
                                    "无匹配结果"
                                }),
                        )
                    }),
            )
    }
}
