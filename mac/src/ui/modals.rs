//! 模态框：删除确认（purge 报告）/
//! 会话终止、删除、重命名 / 配置变更确认。

use anyhow::anyhow;
use gpui::{Context, SharedString, Window, div, prelude::*, px};

use super::kit::*;
use super::{Modal, RootView};
use crate::theme;

/// 六个模态的标题行。字号字重一样，标题与正文之间的留白（8 / 10 / 12）各挑各的，
/// 所以是参数——正文块的收尾留白不同，标题这头得跟着配平。
fn modal_title(text: impl Into<SharedString>, pb: f32) -> gpui::Div {
    div()
        .font_weight(gpui::FontWeight::BOLD)
        .text_size(px(15.))
        .pb(px(pb))
        .child(text.into())
}

/// 模态右下角那一行按钮。上留白不收进来：14 / 8 / 0 三种，取决于正文最后一块
/// 自己带了多少 pb。
fn modal_actions() -> gpui::Div {
    div().flex().justify_end().gap(px(8.))
}

/// 模态正文里的说明文字（次要，比标题小一号）。
fn modal_hint(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(12.5))
        .text_color(c(theme::DIM))
        .child(text.into())
}

impl RootView {
    /// ⌘N / 点侧栏输入框：光标进「新建项目」输入框
    pub(super) fn focus_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.new_input.read(cx).focus_handle.clone();
        handle.focus(window, cx);
        cx.notify();
    }

    /// 侧栏「新建项目」那一行按回车：行里的字就是文件夹名，留空 = 时间戳目录名。
    /// 建完清空输入框并进入新会话。
    pub(super) fn create_project(&mut self, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        self.creating = true;
        cx.notify();
        let name = {
            let t = self.new_input.read(cx).text().trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        };
        // 显式写进注册表（名册以注册表为准），不靠 daemon 端兜底
        let agent = self.new_agent.clone();
        let project_agent = Some(agent.clone());
        let fallback_path = name.as_ref().and_then(|n| {
            self.health
                .as_ref()
                .map(|h| format!("{}/{}", h.project_root, n))
        });
        let net = self.net.clone();
        // 不走 spawn_fetch：失败时也得把 `creating` 放掉（不然那一行永远卡在「…」），
        // 而 spawn_fetch 的失败分支只会弹错，碰不到 self
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
                r.creating = false;
                match res {
                    Ok(s) => {
                        r.new_input.update(cx, |i, cx| i.set_text("", cx));
                        let id = s.id.clone();
                        r.upsert_session(s, cx);
                        r.open_session(id, cx);
                        r.fetch_projects(cx);
                    }
                    Err(e) => r.set_error(e.to_string(), cx),
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
        // 同上：失败要把模态的 busy 复位，让人能再点一次「删除」
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |r, cx| {
                match res {
                    Ok(resp) => {
                        // 全部成功就直接收起：列表少了那几行就是结果，不必再弹一层
                        // 「删除完成」让人点关闭。有失败的才留下清单看哪条没删掉。
                        if resp.results.iter().all(|x| x.ok) {
                            r.modal = Modal::None;
                        } else if let Modal::DeleteConfirm { report, busy, .. } = &mut r.modal {
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
        let title = self.name_input.read(cx).text().trim().to_string();
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
        self.spawn_fetch_ignore(fut, true, cx);
        cx.notify();
    }

    /// 侧栏 × / ⌘W / 工具栏「终止」的统一入口：`confirm` 为真（还在执行）先弹确认，
    /// 否则直接终止——等你的会话关掉没损失，不打断。
    pub(super) fn request_kill(&mut self, id: String, confirm: bool, cx: &mut Context<Self>) {
        if confirm {
            self.modal = Modal::ConfirmKill { id };
            cx.notify();
        } else {
            self.kill_session(id, cx);
        }
    }

    /// 终止会话并收起本地 tab：项目从上栏回到下栏。确认弹窗的「终止」也走这里。
    pub(super) fn kill_session(&mut self, id: String, cx: &mut Context<Self>) {
        let fut = self.net.kill_session(&id);
        self.modal = Modal::None;
        // 自己终止的会话，随后的 exited 不弹通知
        self.user_killed.insert(id.clone());
        self.close_tab(&id, cx);
        self.spawn_fetch_ignore(fut, true, cx);
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
            Modal::DeleteConfirm {
                paths,
                report,
                busy,
            } => self.render_delete_confirm(paths, report, *busy, cx),
            Modal::RenameSession { id } => self.render_rename_session(id.clone(), cx),
            Modal::ConfirmKill { id } => self.render_confirm_kill(id.clone(), cx),
            Modal::ConfirmRestart { alive } => self.render_confirm_restart(*alive, cx),
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

    /// 模态外壳。宽度是参数而不是默认 480 再各自 `.w()` 覆盖：
    /// 三种宽度（480 / 440 / 400）本来就是各挑各的，覆盖只是把这件事藏起来。
    fn modal_box(&self, w: f32) -> gpui::Div {
        div()
            .w(px(w))
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

    /// 每个模态左边那个「取消」：五处只差一个元素 id，动作都是收起模态。
    fn modal_cancel(&self, id: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        btn_secondary(id, "取消").on_click(cx.listener(|this, _, _, cx| {
            this.modal = Modal::None;
            cx.notify();
        }))
    }

    /// 批量删除模态有两副面孔：删之前是「要删这几个，确认吗」，删完是一份逐项报告。
    /// 外壳（宽度、标题）共用，正文各画各的。
    fn render_delete_confirm(
        &self,
        paths: &[String],
        report: &Option<crate::model::DeleteResponse>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let body = self.modal_box(440.).child(modal_title(
            if report.is_some() {
                "删除完成".to_string()
            } else {
                format!("批量删除 {} 个项目？", paths.len())
            },
            10.,
        ));
        match report {
            None => self.fill_delete_prompt(body, paths, busy, cx),
            Some(resp) => self.fill_delete_report(body, resp, cx),
        }
    }

    /// 删之前：一项一行（名字 + 占多大），底下两行警告，右下角「取消 / 删除」
    fn fill_delete_prompt(
        &self,
        mut body: gpui::Div,
        paths: &[String],
        busy: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        for p in paths {
            let info = self.projects.iter().find(|x| &x.path == p);
            let line = match info {
                Some(pr) if pr.dir_size == 0 => format!("— {}（空目录）", pr.name),
                Some(pr) => format!("— {}（{}）", pr.name, theme::human_bytes(pr.dir_size)),
                None => format!("— {p}"),
            };
            body = body.child(modal_hint(line));
        }
        body.child(modal_hint("将同时清除所有 agent 在这些目录下的会话存储。").pt(px(8.)))
            .child(
                div()
                    .pt(px(8.))
                    .text_size(px(11.))
                    .text_color(c(theme::FAINT))
                    .child("purge 语义与 aaa CLI 完全一致，不可恢复。"),
            )
            .child(
                modal_actions()
                    .pt(px(14.))
                    .child(self.modal_cancel("del-cancel", cx))
                    .child(
                        btn_danger("del-ok", if busy { "删除中…" } else { "删除" }).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.confirm_delete(cx);
                            }),
                        ),
                    ),
            )
    }

    /// 删之后：一项一块（成败点 + 名字，下面一行「清了哪些会话存储」）
    fn fill_delete_report(
        &self,
        mut body: gpui::Div,
        resp: &crate::model::DeleteResponse,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
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
        // 只有一个「关闭」，没有 gap 可言：这行不走 modal_actions
        body.child(
            div().flex().justify_end().pt(px(14.)).child(
                btn_primary("del-close", "关闭").on_click(cx.listener(|this, _, _, cx| {
                    this.modal = Modal::None;
                    cx.notify();
                })),
            ),
        )
    }

    fn session_title_of(&self, id: &str) -> String {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.display_title())
            .unwrap_or_else(|| id.to_string())
    }

    fn render_rename_session(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box(400.)
            .child(modal_title("重命名会话", 12.))
            .child(self.name_input.clone())
            .child(
                modal_actions()
                    .pt(px(14.))
                    .child(self.modal_cancel("rn-cancel", cx))
                    .child(btn_primary("rn-ok", "保存").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.confirm_rename(id.clone(), cx);
                        },
                    ))),
            )
    }

    /// 关闭确认只在会话还在执行时弹（等你的直接关，见 `request_kill`），提示语只有这一种
    fn kill_warning(&self, _id: &str) -> String {
        "会话还在运行（可能还有后台任务）。关闭会终止整个进程树（TERM，2 秒后 KILL），项目变为暂停，点一下即可 resume。".into()
    }

    fn render_confirm_kill(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box(400.)
            .child(modal_title(
                format!("关闭「{}」？", self.session_title_of(&id)),
                8.,
            ))
            .child(modal_hint(self.kill_warning(&id)))
            .child(
                modal_actions()
                    .pt(px(14.))
                    .child(self.modal_cancel("kill-cancel", cx))
                    .child(btn_danger("kill-ok", "终止").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.kill_session(id.clone(), cx);
                        },
                    ))),
            )
    }

    fn render_confirm_delete_session(&self, id: String, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box(400.)
            .child(modal_title(
                format!("删除会话「{}」？", self.session_title_of(&id)),
                8.,
            ))
            .child(modal_hint("删除记录与回放（存活会话会先被终止），不可恢复。"))
            .child(
                modal_actions()
                    .pt(px(14.))
                    .child(self.modal_cancel("sd-cancel", cx))
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
                r.set_error("配置已保存，daemon 重启中…".into(), cx);
                // 提交确认：轮询新地址的 /health，把「正常重启回来了」和
                // 「配置写坏 / 端口 token 填错」区分开——只停在 Connecting
                // 上用户没法知道该等还是该救（评审指出的盲区）。
                let net = r.net.clone();
                // 这不是一次请求而是一轮轮询（成功/超时各有各的话要说），
                // spawn_fetch 只认「一个 future 一个结果」
                cx.spawn(async move |this, cx| {
                    for _ in 0..20u32 {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                        if let Ok(h) = net.health().await {
                            let _ = this.update(cx, |r, cx| {
                                r.set_error(
                                    format!("daemon 已带新配置回来（v{}）✓ 点击关闭", h.version),
                                    cx,
                                );
                            });
                            return;
                        }
                    }
                    let _ = this.update(cx, |r, cx| {
                        r.set_error(
                            "配置已写入，但 20 秒内没连上新地址——检查端口/token 是否填对，                             或看 daemon 日志（~/.local/state/aaa-daemon/）"
                                .into(),
                            cx,
                        );
                    });
                })
                .detach();
            },
            true,
            cx,
        );
        cx.notify();
    }

    /// 设置页「重启 daemon」：没有存活会话直接重启，有就先弹 ConfirmRestart
    pub(super) fn request_restart_daemon(&mut self, cx: &mut Context<Self>) {
        let alive = self.sessions.iter().filter(|s| super::is_active(s)).count();
        if alive > 0 {
            self.modal = Modal::ConfirmRestart { alive };
            cx.notify();
        } else {
            self.restart_daemon(false, cx);
        }
    }

    /// 重启 daemon：**跑的是 `aaa-daemon service restart` 这个命令，不是 REST**
    /// （2026-09-10 用户拍板）。REST 那条路的前提是「daemon 还活着」，它关掉之后
    /// 按钮就再也点不动了——而「它关掉了」正是最需要这个按钮的时候。命令自己会
    /// 判断：它还答话就走 REST 的 force 重启（活着的会话记下来、起来后自动 resume），
    /// 不答话就交给 launchd 拉起。`force` 在这里只用来决定要不要先弹确认框。
    pub(super) fn restart_daemon(&mut self, _force: bool, cx: &mut Context<Self>) {
        self.modal = Modal::None;
        // exec 期间连接会断一下；先置成连接中，重连逻辑自己接上
        self.conn = super::ConnState::Connecting;
        self.health = None;
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async { crate::net::run_daemon_restart() })
                .await;
            let _ = this.update(cx, |r, cx| {
                if let Err(e) = res {
                    r.set_error(format!("重启失败：{e}"), cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_confirm_restart(&self, alive: usize, cx: &mut Context<Self>) -> gpui::Div {
        self.modal_box(480.)
            .child(modal_title("重启 daemon？", 10.))
            .child(
                modal_hint(format!(
                    "有 {alive} 个会话还活着。daemon 的 PTY 都是它的子进程，重启会把它们一起终止；屏幕回放保留，之后可以从项目行 resume。"
                ))
                .pb(px(14.)),
            )
            .child(
                modal_actions()
                    .child(self.modal_cancel("rs-cancel", cx))
                    .child(
                        btn_danger("rs-ok", "终止并重启")
                            .on_click(cx.listener(|this, _, _, cx| this.restart_daemon(true, cx))),
                    ),
            )
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
        self.modal_box(480.)
            .child(modal_title("项目目录变更", 8.))
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
                modal_hint("「迁移」会整体移动目录并重写注册表（含各项目对话 id，之后 resume 不会乱）。")
                    .pb(px(4.)),
            )
            .child(
                modal_hint("「仅指向」不动旧文件，只把 daemon 指到新目录（须已存在）。两者都要求没有存活会话，daemon 会自动重启。")
                    .pb(px(10.)),
            )
            .child(
                modal_actions()
                    .pt(px(8.))
                    .child(self.modal_cancel("cfg-cancel", cx))
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
