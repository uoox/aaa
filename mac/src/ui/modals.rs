//! 模态框：新建项目（含 6 agent 选择）/ 换 agent / 删除确认（purge 报告）。

use anyhow::anyhow;
use gpui::{Context, SharedString, Window, div, prelude::*, px};

use super::kit::*;
use super::{Modal, RootView};
use crate::theme;

impl RootView {
    pub(super) fn open_new_project_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = Modal::NewProject {
            agent_idx: 0,
            busy: false,
        };
        self.name_input.update(cx, |i, cx| i.set_text("", cx));
        let handle = self.name_input.read(cx).focus_handle.clone();
        handle.focus(window, cx);
        cx.notify();
    }

    pub(super) fn confirm_create_project(&mut self, agent_idx: usize, cx: &mut Context<Self>) {
        if let Modal::NewProject { busy, .. } = &mut self.modal {
            if *busy {
                return;
            }
            *busy = true;
        } else {
            return;
        }
        cx.notify();
        let name = {
            let t = self.name_input.read(cx).text.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        let agent = self
            .agents
            .get(agent_idx)
            .map(|a| a.id.clone())
            .unwrap_or_else(|| "claude".into());
        // shell 也写注册表：名册以注册表为准，且不写的话 daemon 端「没登记」
        // 会回落成 claude，从终端页建的文件夹一转头就变 claude 项目了
        let project_agent = Some(agent.clone());
        let fallback_path = name.as_ref().and_then(|n| {
            self.health
                .as_ref()
                .map(|h| format!("{}/{}", h.project_root, n))
        });
        let net = self.net.clone();
        cx.spawn(async move |this, cx| {
            let res: anyhow::Result<crate::model::Session> = async {
                let v = net.create_project(name.clone(), project_agent.clone()).await?;
                let path = v
                    .get("path")
                    .and_then(|p| p.as_str())
                    .map(str::to_string)
                    .or_else(|| {
                        v.get("project")
                            .and_then(|p| p.get("path"))
                            .and_then(|p| p.as_str())
                            .map(str::to_string)
                    })
                    .or(fallback_path)
                    .ok_or_else(|| anyhow!("项目已创建，但响应缺少 path 字段"))?;
                let s = net.create_session(path, agent.clone(), false).await?;
                Ok(s)
            }
            .await;
            let _ = this.update(cx, |r, cx| {
                match res {
                    Ok(s) => {
                        r.modal = Modal::None;
                        let id = s.id.clone();
                        r.upsert_session(s, cx);
                        r.open_session(id, cx);
                        r.fetch_projects(cx);
                    }
                    Err(e) => {
                        r.set_error(e.to_string(), cx);
                        if let Modal::NewProject { busy, .. } = &mut r.modal {
                            *busy = false;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let paths = if let Modal::DeleteConfirm { paths, busy, report } = &mut self.modal {
            if *busy || report.is_some() {
                return;
            }
            *busy = true;
            paths.clone()
        } else {
            return;
        };
        cx.notify();
        let fut = self.net.delete_projects(paths);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |r, cx| {
                match res {
                    Ok(resp) => {
                        if let Modal::DeleteConfirm { report, busy, .. } = &mut r.modal {
                            *report = Some(resp);
                            *busy = false;
                        }
                        r.fetch_projects(cx);
                    }
                    Err(e) => {
                        r.set_error(e.to_string(), cx);
                        if let Modal::DeleteConfirm { busy, .. } = &mut r.modal {
                            *busy = false;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── 会话操作（重命名 / 终止 / 删除记录） ────────────────────────────

    pub(super) fn open_rename_modal(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.name_input.update(cx, |i, cx| i.set_text(current, cx));
        let handle = self.name_input.read(cx).focus_handle.clone();
        handle.focus(window, cx);
        self.modal = Modal::RenameSession { id };
        cx.notify();
    }

    fn confirm_rename(&mut self, id: String, cx: &mut Context<Self>) {
        let title = self.name_input.read(cx).text.trim().to_string();
        if title.is_empty() {
            self.set_error("名称不能为空".into(), cx);
            return;
        }
        // 乐观更新，session 帧会再对齐
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.title = title.clone();
        }
        let fut = self.net.rename_session(&id, title);
        self.modal = Modal::None;
        self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, true, cx);
        cx.notify();
    }

    fn confirm_kill(&mut self, id: String, cx: &mut Context<Self>) {
        let fut = self.net.kill_session(&id);
        self.modal = Modal::None;
        // 关 TUI = 终止 + 收起 tab：项目从上分区回到下分区
        self.close_tab(&id, cx);
        self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, true, cx);
        cx.notify();
    }

    fn confirm_delete_session(&mut self, id: String, cx: &mut Context<Self>) {
        let fut = self.net.delete_session(&id);
        self.modal = Modal::None;
        let id2 = id.clone();
        self.spawn_fetch(
            fut,
            move |r, _: serde_json::Value, cx| {
                // session_removed 事件也会来；本地先行清理
                r.sessions.retain(|s| s.id != id2);
                r.close_tab(&id2, cx);
                cx.notify();
            },
            true,
            cx,
        );
        cx.notify();
    }

    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let content = match &self.modal {
            Modal::None => return None,
            Modal::NewProject { agent_idx, busy } => {
                self.render_new_project(*agent_idx, *busy, cx)
            }
            Modal::DeleteConfirm {
                paths,
                report,
                busy,
            } => self.render_delete_confirm(paths, report, *busy, cx),
            Modal::RenameSession { id } => self.render_rename_session(id.clone(), cx),
            Modal::ConfirmKill { id } => self.render_confirm_kill(id.clone(), cx),
            Modal::ConfirmDeleteSession { id } => {
                self.render_confirm_delete_session(id.clone(), cx)
            }
            Modal::ConfirmConfig {
                port,
                token,
                old_root,
                new_root,
            } => self.render_confirm_config(*port, token.clone(), old_root.clone(), new_root.clone(), cx),
        };
        Some(
            div()
                .id("modal-scrim")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgba(0x00000073))
                .occlude()
                .child(content),
        )
    }

    fn modal_box(&self) -> gpui::Div {
        div()
            .w(px(480.))
            .max_h(px(600.))
            .p(px(18.))
            .rounded(px(12.))
            .bg(c(theme::SURFACE_RAISED))
            .border_1()
            .border_color(c(theme::EDGE_LIGHT))
            .shadow_lg()
            .flex()
            .flex_col()
    }

    fn agent_rows(
        &self,
        selected: Option<usize>,
        on_pick_id: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut list = div().flex().flex_col().gap(px(4.));
        for (ix, a) in self.agents.iter().enumerate() {
            let is_sel = selected == Some(ix);
            let is_shell = a.id == "shell";
            let chip_label: SharedString = if is_shell {
                "zsh".into()
            } else {
                a.id.clone().into()
            };
            let label: SharedString = if is_shell {
                "普通终端".into()
            } else {
                a.label.clone().into()
            };
            let cmd: SharedString = a.cmd.clone().unwrap_or_default().into();
            list = list.child(
                div()
                    .id((on_pick_id, ix))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(8.))
                    .border_1()
                    .when(is_shell, |el| el.border_dashed())
                    .border_color(if is_sel {
                        c(theme::CYAN)
                    } else {
                        c(theme::EDGE_LIGHT)
                    })
                    .when(is_sel, |el| el.bg(ca(theme::CYAN, 0.07)))
                    .cursor_pointer()
                    .hover(|st| st.border_color(c(theme::CYAN)))
                    .when(!a.available, |el| el.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.on_agent_row_click(ix, cx);
                    }))
                    .child(agent_chip(&a.id, chip_label))
                    .child(div().text_size(px(12.5)).child(label))
                    .when(!a.available, |el| {
                        el.child(
                            div()
                                .text_size(px(10.))
                                .text_color(c(theme::RED))
                                .child("未安装"),
                        )
                    })
                    .child(
                        div()
                            .ml_auto()
                            .max_w(px(220.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::FAINT))
                            .child(cmd),
                    ),
            );
        }
        list
    }

    /// agent 行点击：目前只有新建项目模态用得到（换 agent 入口已随项目页移除）
    fn on_agent_row_click(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Modal::NewProject { busy, .. } = &self.modal {
            let b = *busy;
            self.modal = Modal::NewProject {
                agent_idx: ix,
                busy: b,
            };
            cx.notify();
        }
    }

    fn render_new_project(
        &self,
        agent_idx: usize,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let ts_hint = chrono::Local::now().format("%Y-%m-%d-%H%M").to_string();
        self.modal_box()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(15.))
                    .pb(px(12.))
                    .child("新项目"),
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(10.))
                    .text_color(c(theme::FAINT))
                    .pb(px(4.))
                    .child(SharedString::from(format!(
                        "名称 · {}/<名称>",
                        self.health
                            .as_ref()
                            .map(|h| h.project_root.clone())
                            .unwrap_or_else(|| "/Volumes/SSD/project".into())
                    ))),
            )
            .child(self.name_input.clone())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(c(theme::FAINT))
                    .pt(px(4.))
                    .pb(px(12.))
                    .child(SharedString::from(format!("留空 = {ts_hint}"))),
            )
            .child(
                div()
                    .id("np-agents-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(self.agent_rows(Some(agent_idx), "np-agent", cx)),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(14.))
                    .child(
                        btn_secondary("np-cancel", "取消").on_click(cx.listener(|this, _, _, cx| {
                            this.modal = Modal::None;
                            cx.notify();
                        })),
                    )
                    .child(
                        btn_primary("np-ok", if busy { "创建中…" } else { "创建并进入" }).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.confirm_create_project(agent_idx, cx);
                            }),
                        ),
                    ),
            )
    }

    fn render_delete_confirm(
        &self,
        paths: &[String],
        report: &Option<crate::model::DeleteResponse>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut body = self.modal_box().w(px(440.)).child(
            div()
                .font_weight(gpui::FontWeight::BOLD)
                .text_size(px(15.))
                .pb(px(10.))
                .child(SharedString::from(if report.is_some() {
                    "删除完成".to_string()
                } else {
                    format!("批量删除 {} 个项目？", paths.len())
                })),
        );

        match report {
            None => {
                for p in paths {
                    let info = self.projects.iter().find(|x| &x.path == p);
                    let line = match info {
                        Some(pr) if pr.dir_size == 0 => format!("— {}（空目录）", pr.name),
                        Some(pr) => {
                            format!("— {}（{}）", pr.name, theme::human_bytes(pr.dir_size))
                        }
                        None => format!("— {p}"),
                    };
                    body = body.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(line)),
                    );
                }
                body = body
                    .child(
                        div()
                            .pt(px(8.))
                            .text_size(px(12.5))
                            .text_color(c(theme::DIM))
                            .child("将同时清除各 agent 会话存储（含历史遗留）。"),
                    )
                    .child(
                        div()
                            .pt(px(8.))
                            .text_size(px(11.))
                            .text_color(c(theme::FAINT))
                            .child("purge 语义与 aaa CLI 完全一致，不可恢复。"),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .pt(px(14.))
                            .child(btn_secondary("del-cancel", "取消").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.modal = Modal::None;
                                    cx.notify();
                                },
                            )))
                            .child(
                                btn_danger("del-ok", if busy { "删除中…" } else { "删除" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_delete(cx);
                                    })),
                            ),
                    );
            }
            Some(resp) => {
                for r in &resp.results {
                    let name = r.path.rsplit('/').next().unwrap_or(&r.path);
                    let purged = if r.purged.is_empty() {
                        "无会话存储".to_string()
                    } else {
                        r.purged
                            .iter()
                            .map(|p| format!("{} {} 条", p.agent_label, p.count))
                            .collect::<Vec<_>>()
                            .join(" · ")
                    };
                    body = body.child(
                        div()
                            .py(px(4.))
                            .border_b_1()
                            .border_color(ca(theme::EDGE, 0.5))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(dot(if r.ok { theme::GREEN } else { theme::RED }))
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .child(SharedString::from(name.to_string())),
                                    ),
                            )
                            .child(
                                div()
                                    .pl(px(15.))
                                    .font_family("Menlo")
                                    .text_size(px(11.))
                                    .text_color(c(theme::INK))
                                    .child(SharedString::from(purged)),
                            ),
                    );
                }
                body = body.child(
                    div().flex().justify_end().pt(px(14.)).child(
                        btn_primary("del-close", "关闭").on_click(cx.listener(|this, _, _, cx| {
                            this.modal = Modal::None;
                            cx.notify();
                        })),
                    ),
                );
            }
        }
        body
    }

    fn session_title_of(&self, id: &str) -> String {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.display_title())
            .unwrap_or_else(|| id.to_string())
    }

    fn render_rename_session(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box()
            .w(px(400.))
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(15.))
                    .pb(px(12.))
                    .child("重命名会话"),
            )
            .child(self.name_input.clone())
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(14.))
                    .child(
                        btn_secondary("rn-cancel", "取消").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.modal = Modal::None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(btn_primary("rn-ok", "保存").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.confirm_rename(id.clone(), cx);
                        },
                    ))),
            )
    }

    /// 关闭 TUI 的提示语：还在跑的进程要说得更重
    fn kill_warning(&self, id: &str) -> String {
        let running = self
            .sessions
            .iter()
            .any(|s| s.id == id && s.state == crate::model::SessionState::Running);
        if running {
            "会话仍在执行中（可能还有后台任务）。关闭会终止整个进程树（TERM，2 秒后 KILL），项目回到下方未激活区，下次双击可 resume。".into()
        } else {
            "进程将被终止（TERM，2 秒后 KILL），项目回到下方未激活区，下次双击可 resume。".into()
        }
    }

    fn render_confirm_kill(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box()
            .w(px(400.))
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(15.))
                    .pb(px(8.))
                    .child(SharedString::from(format!(
                        "关闭「{}」？",
                        self.session_title_of(&id)
                    ))),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(c(theme::DIM))
                    .child(SharedString::from(self.kill_warning(&id))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(14.))
                    .child(
                        btn_secondary("kill-cancel", "取消").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.modal = Modal::None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(btn_danger("kill-ok", "终止").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.confirm_kill(id.clone(), cx);
                        },
                    ))),
            )
    }

    fn render_confirm_delete_session(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box()
            .w(px(400.))
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(15.))
                    .pb(px(8.))
                    .child(SharedString::from(format!(
                        "删除会话「{}」？",
                        self.session_title_of(&id)
                    ))),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(c(theme::DIM))
                    .child("删除记录与回放（存活会话会先被终止），不可恢复。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(14.))
                    .child(
                        btn_secondary("sd-cancel", "取消").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.modal = Modal::None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(btn_danger("sd-ok", "删除").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.confirm_delete_session(id.clone(), cx);
                        },
                    ))),
            )
    }
    // ── 配置变更（含项目目录迁移） ──────────────────────────────────────

    /// 发送配置到 daemon。root=None 表示目录没变。成功后 daemon 自我重启，
    /// 本地立刻切到新端口/token 等它回来。
    pub(super) fn apply_daemon_config(
        &mut self,
        port: u16,
        token: String,
        root: Option<String>,
        migrate: bool,
        cx: &mut Context<Self>,
    ) {
        let host = self
            .net
            .endpoint()
            .map(|e| e.host)
            .unwrap_or_else(|| "127.0.0.1".into());
        let fut = self
            .net
            .put_config(Some(port), Some(token.clone()), root, migrate);
        self.modal = Modal::None;
        self.spawn_fetch(
            fut,
            move |r, _: serde_json::Value, cx| {
                r.net.set_endpoint(crate::model::Endpoint { host, port, token });
                r.conn = crate::net::ConnState::Connecting;
                r.set_error("配置已保存，daemon 重启中…（会自动重连，点击关闭本条）".into(), cx);
            },
            true,
            cx,
        );
        cx.notify();
    }

    fn render_confirm_config(
        &self,
        port: u16,
        token: String,
        old_root: String,
        new_root: String,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (t1, t2) = (token.clone(), token);
        let (r1, r2) = (new_root.clone(), new_root.clone());
        self.modal_box()
            .child(
                div()
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_size(px(15.))
                    .pb(px(8.))
                    .child("项目目录变更"),
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(11.5))
                    .text_color(c(theme::DIM))
                    .child(SharedString::from(old_root)),
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(11.5))
                    .text_color(c(theme::INK))
                    .pb(px(10.))
                    .child(SharedString::from(format!("→ {new_root}"))),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(c(theme::DIM))
                    .pb(px(4.))
                    .child("「迁移」会整体移动目录并重写注册表（含各项目对话 id，之后 resume 不会乱）。"),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(c(theme::DIM))
                    .pb(px(10.))
                    .child("「仅指向」不动旧文件，只把 daemon 指到新目录（须已存在）。两者都要求没有存活会话，daemon 会自动重启。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .pt(px(8.))
                    .child(
                        btn_secondary("cfg-cancel", "取消").on_click(cx.listener(
                            |this, _, _, cx| {
                                this.modal = Modal::None;
                                cx.notify();
                            },
                        )),
                    )
                    .child(btn_secondary("cfg-point", "仅指向").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.apply_daemon_config(port, t1.clone(), Some(r1.clone()), false, cx);
                        },
                    )))
                    .child(btn_primary("cfg-migrate", "迁移并切换").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.apply_daemon_config(port, t2.clone(), Some(r2.clone()), true, cx);
                        },
                    ))),
            )
    }

}
