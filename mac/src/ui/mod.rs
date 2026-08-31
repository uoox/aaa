//! UI 根视图：侧栏 + tab 条 + 页面区 + 状态栏 + 模态框。

mod kit;
mod mini_input;
mod modals;
mod overview;
mod projects;
mod settings;
mod terminal_view;

use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use gpui::{
    AppContext as _, Context, Entity, SharedString, Task, Window, div, prelude::*, px,
};

use crate::model::*;
use crate::net::{ConnState, Net, UiEvent};
use crate::theme;
use kit::*;
use mini_input::MiniInput;
use terminal_view::TerminalView;

#[derive(Clone, PartialEq)]
pub enum Page {
    Overview,
    Session(String),
    Projects,
    Settings,
}

pub enum Modal {
    None,
    NewProject {
        agent_idx: usize,
        busy: bool,
    },
    AgentPick {
        path: String,
    },
    DeleteConfirm {
        paths: Vec<String>,
        report: Option<DeleteResponse>,
        busy: bool,
    },
    RenameSession {
        id: String,
    },
    ConfirmKill {
        id: String,
    },
    ConfirmDeleteSession {
        id: String,
    },
}

pub struct RootView {
    pub net: Net,
    pub page: Page,
    pub modal: Modal,

    // daemon 状态
    pub conn: ConnState,
    pub health: Option<Health>,
    pub ssd_mounted: bool,
    pub sessions: Vec<Session>,
    pub projects: Vec<Project>,
    pub agents: Vec<AgentInfo>,
    pub permissions: Vec<Permission>,
    /// 配对二维码模块（(宽, 黑白位图)；fetch 时编码一次，渲染帧只读）
    pub qr_modules: Option<(usize, Vec<bool>)>,
    pub endpoint_from_config: bool,

    // 终端
    terminals: HashMap<String, Entity<TerminalView>>,
    open_order: Vec<String>,
    pending_focus: Option<String>,

    // v1.1 watchdog：id → quiet_s
    pub stalled: HashMap<String, u64>,
    // 会话监听端口缓存（Web 预览）
    pub ports_cache: HashMap<String, Vec<PortEntry>>,
    // 通知去重：同会话同 question 只通知一次
    last_notified_question: HashMap<String, String>,

    // 输入框
    pub name_input: Entity<MiniInput>,
    pub search_input: Entity<MiniInput>,
    pub host_input: Entity<MiniInput>,
    pub port_input: Entity<MiniInput>,
    pub token_input: Entity<MiniInput>,

    // 项目页选择
    pub selected_paths: HashSet<String>,

    pub error: Option<String>,
    _pump: Task<()>,
}

impl RootView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = DaemonConfig::load();
        let endpoint = config.as_ref().map(Endpoint::from_config);
        let endpoint_from_config = endpoint.is_some();
        let (net, mut rx) = Net::new(endpoint.clone());

        let pump = cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                if this
                    .update(cx, |root: &mut RootView, cx| root.on_net_event(ev, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        let name_input = cx.new(|cx| MiniInput::new(cx, "留空 = 时间戳目录名"));
        let search_input = cx.new(|cx| MiniInput::new(cx, "搜索…"));
        let host_input = cx.new(|cx| MiniInput::new(cx, "127.0.0.1"));
        let port_input = cx.new(|cx| MiniInput::new(cx, "2730"));
        let token_input = cx.new(|cx| MiniInput::new(cx, "aaa_tk_…"));
        if let Some(ep) = &endpoint {
            host_input.update(cx, |i, cx| i.set_text(ep.host.clone(), cx));
            port_input.update(cx, |i, cx| i.set_text(ep.port.to_string(), cx));
            token_input.update(cx, |i, cx| i.set_text(ep.token.clone(), cx));
        }

        RootView {
            net,
            page: Page::Overview,
            modal: Modal::None,
            conn: ConnState::Connecting,
            health: None,
            ssd_mounted: true,
            sessions: Vec::new(),
            projects: Vec::new(),
            agents: builtin_agents(),
            permissions: Vec::new(),
            qr_modules: None,
            endpoint_from_config,
            terminals: HashMap::new(),
            open_order: Vec::new(),
            pending_focus: None,
            stalled: HashMap::new(),
            ports_cache: HashMap::new(),
            last_notified_question: HashMap::new(),
            name_input,
            search_input,
            host_input,
            port_input,
            token_input,
            selected_paths: HashSet::new(),
            error: None,
            _pump: pump,
        }
    }

    // ── 网络事件 ────────────────────────────────────────────────────────

    fn on_net_event(&mut self, ev: UiEvent, cx: &mut Context<Self>) {
        match ev {
            UiEvent::Conn(state) => {
                let was = self.conn;
                self.conn = state;
                if state == ConnState::Connected && was != ConnState::Connected {
                    self.fetch_all(cx);
                }
                cx.notify();
            }
            UiEvent::Daemon(ev) => self.on_daemon_event(ev, cx),
            UiEvent::TermHello {
                id,
                session,
                rows,
                cols,
            } => {
                self.upsert_session(*session, cx);
                if let Some(t) = self.terminals.get(&id) {
                    t.update(cx, |t, cx| {
                        // hello 即视为链路恢复（重连后可能长时间无输出，不能等首字节才撤横幅）
                        t.set_down(false, cx);
                        t.set_remote_size(cols, rows, cx);
                    });
                }
            }
            UiEvent::TermData { id, bytes } => {
                if let Some(t) = self.terminals.get(&id) {
                    t.update(cx, |t, cx| t.feed(&bytes, cx));
                }
            }
            UiEvent::TermDown { id } => {
                if let Some(t) = self.terminals.get(&id) {
                    t.update(cx, |t, cx| t.set_down(true, cx));
                }
            }
        }
    }

    fn on_daemon_event(&mut self, ev: DaemonEvent, cx: &mut Context<Self>) {
        match ev {
            DaemonEvent::Snapshot { sessions } => {
                self.sessions = sessions;
                self.sort_sessions();
                // 快照不带 stalled 状态：仅保留仍在 running 的标记
                self.stalled.retain(|id, _| {
                    self.sessions
                        .iter()
                        .any(|s| &s.id == id && s.state == SessionState::Running)
                });
                cx.notify();
            }
            DaemonEvent::Session { session } => {
                // 任何会话帧（状态/preview/title 变化）都视为有活动，解除空转标记
                self.stalled.remove(&session.id);
                self.maybe_notify(&session);
                self.upsert_session(session, cx);
            }
            DaemonEvent::SessionRemoved { id } => {
                self.sessions.retain(|s| s.id != id);
                self.terminals.remove(&id);
                self.open_order.retain(|x| x != &id);
                self.stalled.remove(&id);
                self.ports_cache.remove(&id);
                self.last_notified_question.remove(&id);
                if self.page == Page::Session(id.clone()) {
                    self.page = self
                        .open_order
                        .last()
                        .map(|x| Page::Session(x.clone()))
                        .unwrap_or(Page::Overview);
                }
                cx.notify();
            }
            DaemonEvent::ProjectsChanged {} => self.fetch_projects(cx),
            DaemonEvent::Health { ssd_mounted } => {
                self.ssd_mounted = ssd_mounted;
                cx.notify();
            }
            DaemonEvent::SessionStalled { id, quiet_s } => {
                if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
                    crate::notify::send(
                        &s.display_title(),
                        &format!("可能空转：已静默 {} 分钟", (quiet_s / 60).max(1)),
                    );
                }
                self.stalled.insert(id, quiet_s);
                cx.notify();
            }
            DaemonEvent::Unknown => {}
        }
    }

    /// 系统通知：进入 waiting（带 question，去重）与 running→exited
    fn maybe_notify(&mut self, new: &Session) {
        let old_state = self
            .sessions
            .iter()
            .find(|s| s.id == new.id)
            .map(|s| s.state);
        match new.state {
            SessionState::Waiting => {
                let key = new
                    .question
                    .as_ref()
                    .map(|q| q.text.clone())
                    .unwrap_or_default();
                if self.last_notified_question.get(&new.id) != Some(&key) {
                    let body = if key.is_empty() { "等待输入" } else { &key };
                    crate::notify::send(&new.display_title(), body);
                    self.last_notified_question.insert(new.id.clone(), key);
                }
            }
            SessionState::Exited if old_state == Some(SessionState::Running) => {
                let body = match new.exit_code {
                    Some(code) => format!("已退出 (exit {code})"),
                    None => "已退出".to_string(),
                };
                crate::notify::send(&new.display_title(), &body);
            }
            _ => {}
        }
    }

    fn upsert_session(&mut self, session: Session, cx: &mut Context<Self>) {
        match self.sessions.iter_mut().find(|s| s.id == session.id) {
            Some(slot) => *slot = session,
            None => self.sessions.push(session),
        }
        self.sort_sessions();
        cx.notify();
    }

    fn sort_sessions(&mut self) {
        self.sessions.sort_by(|a, b| {
            a.state
                .sort_weight()
                .cmp(&b.state.sort_weight())
                .then(b.last_output_at.cmp(&a.last_output_at))
        });
    }

    // ── 数据拉取 ────────────────────────────────────────────────────────

    fn spawn_fetch<T: 'static>(
        &self,
        fut: impl Future<Output = anyhow::Result<T>> + 'static,
        apply: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        toast_error: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            match fut.await {
                Ok(v) => {
                    let _ = this.update(cx, |r, cx| apply(r, v, cx));
                }
                Err(e) => {
                    log::warn!("请求失败: {e}");
                    if toast_error {
                        let _ = this.update(cx, |r, cx| r.set_error(e.to_string(), cx));
                    }
                }
            }
        })
        .detach();
    }

    pub fn fetch_all(&mut self, cx: &mut Context<Self>) {
        self.spawn_fetch(
            self.net.health(),
            |r, h: Health, cx| {
                r.ssd_mounted = h.ssd_mounted;
                r.health = Some(h);
                cx.notify();
            },
            false,
            cx,
        );
        self.spawn_fetch(
            self.net.agents(),
            |r, a: Vec<AgentInfo>, cx| {
                if !a.is_empty() {
                    r.agents = a;
                }
                cx.notify();
            },
            false,
            cx,
        );
        self.spawn_fetch(
            self.net.sessions(),
            |r, s: Vec<Session>, cx| {
                r.sessions = s;
                r.sort_sessions();
                cx.notify();
            },
            false,
            cx,
        );
        self.fetch_projects(cx);
        self.fetch_permissions(cx);
        self.spawn_fetch(
            self.net.pair(),
            |r, p: PairResponse, cx| {
                r.qr_modules = settings::qr_encode(&p.payload);
                cx.notify();
            },
            false,
            cx,
        );
    }

    pub fn fetch_projects(&mut self, cx: &mut Context<Self>) {
        self.spawn_fetch(
            self.net.projects(),
            |r, p: Vec<Project>, cx| {
                r.projects = p;
                cx.notify();
            },
            false,
            cx,
        );
    }

    pub fn fetch_permissions(&mut self, cx: &mut Context<Self>) {
        self.spawn_fetch(
            self.net.permissions(),
            |r, p: Vec<Permission>, cx| {
                r.permissions = p;
                cx.notify();
            },
            false,
            cx,
        );
    }

    pub fn set_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.error = Some(msg);
        cx.notify();
    }

    // ── 会话操作 ────────────────────────────────────────────────────────

    pub fn open_session(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.terminals.contains_key(&id) {
            let net = self.net.clone();
            let sid = id.clone();
            let term = cx.new(|cx| TerminalView::new(sid, &net, cx));
            self.terminals.insert(id.clone(), term);
            self.open_order.push(id.clone());
        }
        self.fetch_ports(id.clone(), cx);
        self.page = Page::Session(id.clone());
        self.pending_focus = Some(id);
        cx.notify();
    }

    /// 关 tab（仅 detach，不 kill 进程）
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.terminals.remove(id);
        self.open_order.retain(|x| x != id);
        if self.page == Page::Session(id.to_string()) {
            self.page = self
                .open_order
                .last()
                .map(|x| Page::Session(x.clone()))
                .unwrap_or(Page::Overview);
        }
        cx.notify();
    }

    pub fn fetch_ports(&mut self, id: String, cx: &mut Context<Self>) {
        let fut = self.net.ports(&id);
        self.spawn_fetch(
            fut,
            move |r, ports: Vec<PortEntry>, cx| {
                r.ports_cache.insert(id, ports);
                cx.notify();
            },
            false,
            cx,
        );
    }

    /// 项目行双击 / 打开按钮：resume 最近会话
    pub fn open_project(&mut self, project: &Project, cx: &mut Context<Self>) {
        let agent = project.agent.clone().unwrap_or_else(|| "claude".into());
        let resume = agent != "shell";
        let fut = self.net.create_session(project.path.clone(), agent, resume);
        self.spawn_fetch(
            fut,
            |r, s: Session, cx| {
                let id = s.id.clone();
                r.upsert_session(s, cx);
                r.open_session(id, cx);
            },
            true,
            cx,
        );
    }

    /// waiting 选项胶囊点击
    pub fn answer_question(&mut self, id: &str, key: String, cx: &mut Context<Self>) {
        let fut = self.net.session_input(id, key, true);
        self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, true, cx);
    }

    fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let waiting_count = self
            .sessions
            .iter()
            .filter(|s| s.state == SessionState::Waiting)
            .count();

        let mut sessions_col = div().flex().flex_col().gap(px(1.));
        for (ix, s) in self.sessions.iter().enumerate() {
            let id = s.id.clone();
            let active = self.page == Page::Session(id.clone());
            let is_stalled =
                s.state == SessionState::Running && self.stalled.contains_key(&s.id);
            let agent_label: SharedString = if s.agent == "shell" {
                "term".into()
            } else {
                s.agent.clone().into()
            };
            sessions_col = sessions_col.child(
                div()
                    .id(("sb-sess", ix))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .mx(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(active, |el| el.bg(c(theme::SURFACE_RAISED)))
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id.clone(), cx);
                    }))
                    .child(dot(theme::state_color(s.state.as_str())))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(c(if s.state == SessionState::Exited {
                                theme::DIM
                            } else {
                                theme::INK
                            }))
                            .child(SharedString::from(s.display_title())),
                    )
                    .when(is_stalled, |el| {
                        el.child(
                            div()
                                .text_size(px(9.5))
                                .font_family("Menlo")
                                .text_color(c(theme::AMBER))
                                .child("空转?"),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(c(theme::FAINT))
                            .child(agent_label),
                    ),
            );
        }

        let (conn_color, conn_text) = match self.conn {
            ConnState::Connected => (
                theme::GREEN,
                format!(
                    "daemon v{}",
                    self.health
                        .as_ref()
                        .map(|h| h.version.as_str())
                        .unwrap_or("?")
                ),
            ),
            ConnState::Connecting => (theme::AMBER, "连接中…".to_string()),
            ConnState::Disconnected => (theme::RED, "未连接".to_string()),
        };

        let label = |text: &'static str| {
            div()
                .px(px(16.))
                .pt(px(12.))
                .pb(px(4.))
                .text_size(px(10.))
                .font_family("Menlo")
                .text_color(c(theme::FAINT))
                .child(text)
        };

        div()
            .w(px(210.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(c(theme::SURFACE))
            .border_r_1()
            .border_color(c(theme::EDGE))
            .child(
                div()
                    .id("sb-overview")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .mx(px(6.))
                    .mt(px(10.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(self.page == Page::Overview, |el| {
                        el.bg(c(theme::SURFACE_RAISED))
                    })
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Overview;
                        cx.notify();
                    }))
                    .child(div().text_size(px(12.)).child("◱"))
                    .child(div().text_size(px(12.5)).child("总览"))
                    .when(waiting_count > 0, |el| {
                        el.child(
                            div()
                                .ml_auto()
                                .px(px(6.))
                                .rounded_full()
                                .bg(ca(theme::AMBER, 0.18))
                                .text_size(px(10.5))
                                .font_family("Menlo")
                                .text_color(c(theme::AMBER))
                                .child(SharedString::from(waiting_count.to_string())),
                        )
                    }),
            )
            .child(label("会话"))
            .child(
                div()
                    .id("sb-sessions-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(sessions_col),
            )
            .child(label("资料库"))
            .child(
                div()
                    .id("sb-projects")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .mx(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(self.page == Page::Projects, |el| {
                        el.bg(c(theme::SURFACE_RAISED))
                    })
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Projects;
                        this.fetch_projects(cx);
                        cx.notify();
                    }))
                    .child(div().text_size(px(12.)).child("▤"))
                    .child(div().flex_1().text_size(px(12.5)).child("项目"))
                    .child(
                        div()
                            .text_size(px(10.5))
                            .font_family("Menlo")
                            .text_color(c(theme::FAINT))
                            .child(SharedString::from(self.projects.len().to_string())),
                    ),
            )
            .child(
                div()
                    .id("sb-new")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .mx(px(6.))
                    .mb(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_project_modal(window, cx);
                    }))
                    .child(div().text_size(px(13.)).text_color(c(theme::CYAN)).child("＋"))
                    .child(div().text_size(px(12.5)).child("新建…")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(16.))
                    .py(px(9.))
                    .border_t_1()
                    .border_color(c(theme::EDGE))
                    .child(dot(conn_color))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family("Menlo")
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(conn_text)),
                    )
                    .child(
                        div()
                            .id("sb-settings")
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(if self.page == Page::Settings {
                                c(theme::CYAN)
                            } else {
                                c(theme::DIM)
                            })
                            .hover(|st| st.text_color(c(theme::CYAN)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = Page::Settings;
                                this.fetch_permissions(cx);
                                cx.notify();
                            }))
                            .child("⚙"),
                    ),
            )
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut bar = div()
            .flex()
            .items_center()
            .h(px(34.))
            .flex_none()
            .px(px(6.))
            .gap(px(2.))
            .bg(c(theme::BG))
            .border_b_1()
            .border_color(c(theme::EDGE));
        for (ix, id) in self.open_order.iter().enumerate() {
            let Some(s) = self.session(id) else {
                continue;
            };
            let active = self.page == Page::Session(id.clone());
            let id2 = id.clone();
            let id3 = id.clone();
            let name = if s.project_name.is_empty() {
                s.display_title()
            } else {
                s.project_name.clone()
            };
            bar = bar.child(
                div()
                    .id(("tab", ix))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(active, |el| {
                        el.bg(c(theme::SURFACE_RAISED)).text_color(c(theme::INK))
                    })
                    .when(!active, |el| el.text_color(c(theme::DIM)))
                    .hover(|st| st.bg(c(theme::SURFACE)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_session(id2.clone(), cx);
                    }))
                    .child(dot(theme::state_color(s.state.as_str())))
                    .child(SharedString::from(name))
                    .child(
                        div()
                            .id(("tab-close", ix))
                            .ml(px(2.))
                            .px(px(3.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(c(theme::FAINT))
                            .hover(|st| st.text_color(c(theme::INK)).bg(c(theme::EDGE_LIGHT)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(&id3, cx);
                            }))
                            .child("✕"),
                    ),
            );
        }
        bar
    }

    fn render_question_bar(&self, s: &Session, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if s.state != SessionState::Waiting {
            return None;
        }
        let q = s.question.clone()?;
        let sid = s.id.clone();
        let mut row = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .flex_none()
            .bg(ca(theme::AMBER, 0.08))
            .border_t_1()
            .border_color(ca(theme::AMBER, 0.35))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c(theme::AMBER))
                    .child(SharedString::from(format!("? {}", q.text))),
            );
        for (ix, opt) in q.options.iter().enumerate() {
            let key = opt.key.clone();
            let sid2 = sid.clone();
            row = row.child(
                div()
                    .id(("qopt", ix))
                    .px(px(10.))
                    .py(px(3.))
                    .rounded_full()
                    .border_1()
                    .border_color(c(theme::AMBER))
                    .bg(ca(theme::AMBER, 0.12))
                    .text_size(px(12.))
                    .text_color(c(theme::INK))
                    .cursor_pointer()
                    .hover(|st| st.bg(ca(theme::AMBER, 0.28)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.answer_question(&sid2.clone(), key.clone(), cx);
                    }))
                    .child(SharedString::from(format!("{}. {}", opt.key, opt.label))),
            );
        }
        Some(row)
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let bar = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .h(px(26.))
            .flex_none()
            .px(px(12.))
            .bg(c(theme::SURFACE))
            .border_t_1()
            .border_color(c(theme::EDGE))
            .text_size(px(11.))
            .font_family("Menlo")
            .text_color(c(theme::DIM));
        match &self.page {
            Page::Session(id) => {
                if let Some(s) = self.session(id) {
                    let agent_part = match &s.resume_id {
                        Some(r) if !r.is_empty() => {
                            format!("{} · resume {}", s.agent, &r[..r.len().min(6)])
                        }
                        _ => s.agent.clone(),
                    };
                    let state_color = theme::state_color(s.state.as_str());
                    let sid = s.id.clone();
                    let exited = s.state == SessionState::Exited;
                    let stalled_min = (s.state == SessionState::Running)
                        .then(|| self.stalled.get(&s.id).map(|q| (q / 60).max(1)))
                        .flatten();
                    let host = self
                        .net
                        .endpoint()
                        .map(|e| e.host)
                        .unwrap_or_else(|| "127.0.0.1".into());

                    let mut bar = bar
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(s.project_path.clone())),
                        )
                        .child(SharedString::from(agent_part));

                    // Web 预览：进程树监听端口
                    if let Some(ports) = self.ports_cache.get(&s.id) {
                        for (ix, p) in ports.iter().take(4).enumerate() {
                            let url = format!("http://{}:{}", host, p.port);
                            bar = bar.child(
                                div()
                                    .id(("port", ix))
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .cursor_pointer()
                                    .text_color(c(theme::CYAN))
                                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.open_url(&url);
                                    }))
                                    .child(SharedString::from(format!("▶ 预览 :{}", p.port))),
                            );
                        }
                    }
                    let sid_ports = sid.clone();
                    bar = bar.child(
                        div()
                            .id("ports-refresh")
                            .px(px(4.))
                            .cursor_pointer()
                            .text_color(c(theme::FAINT))
                            .hover(|st| st.text_color(c(theme::CYAN)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.fetch_ports(sid_ports.clone(), cx);
                            }))
                            .child("↻端口"),
                    );

                    // 会话操作
                    let act = |id: &'static str, label: &'static str, color: u32| {
                        div()
                            .id(id)
                            .px(px(6.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .text_color(c(color))
                            .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                            .child(label)
                    };
                    let sid_rename = sid.clone();
                    let sid_kill = sid.clone();
                    let sid_del = sid.clone();
                    bar = bar
                        .child(div().ml_auto())
                        .child(act("sess-rename", "重命名", theme::DIM).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_rename_modal(sid_rename.clone(), window, cx);
                            },
                        )));
                    if !exited {
                        bar = bar.child(act("sess-kill", "终止", theme::AMBER).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.modal = Modal::ConfirmKill {
                                    id: sid_kill.clone(),
                                };
                                cx.notify();
                            }),
                        ));
                    }
                    bar = bar.child(act("sess-del", "删除", theme::RED).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.modal = Modal::ConfirmDeleteSession {
                                id: sid_del.clone(),
                            };
                            cx.notify();
                        },
                    )));

                    if let Some(min) = stalled_min {
                        bar = bar.child(
                            div()
                                .text_color(c(theme::AMBER))
                                .child(SharedString::from(format!("可能空转 {min} 分"))),
                        );
                    }
                    bar.child(
                        div()
                            .text_color(c(state_color))
                            .child(SharedString::from(format!(
                                "● {}",
                                theme::state_label(s.state.as_str())
                            ))),
                    )
                } else {
                    bar.child("会话不存在")
                }
            }
            Page::Projects => {
                let root = self
                    .health
                    .as_ref()
                    .map(|h| h.project_root.clone())
                    .unwrap_or_default();
                bar.child(SharedString::from(format!("{} 个项目", self.projects.len())))
                    .child(SharedString::from(root))
                    .child(
                        div()
                            .ml_auto()
                            .text_color(c(if self.ssd_mounted {
                                theme::GREEN
                            } else {
                                theme::RED
                            }))
                            .child(if self.ssd_mounted {
                                "SSD 已挂载 ✓"
                            } else {
                                "SSD 未挂载 ✕"
                            }),
                    )
            }
            _ => {
                let waiting = self
                    .sessions
                    .iter()
                    .filter(|s| s.state == SessionState::Waiting)
                    .count();
                bar.child(SharedString::from(format!(
                    "{} 个会话 · {} 个等待输入",
                    self.sessions.len(),
                    waiting
                )))
            }
        }
    }

    fn render_error_toast(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let err = self.error.clone()?;
        Some(
            div()
                .id("error-toast")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(12.))
                .py(px(6.))
                .bg(ca(theme::RED, 0.14))
                .border_b_1()
                .border_color(c(theme::RED))
                .text_size(px(12.))
                .text_color(c(theme::RED))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.error = None;
                    cx.notify();
                }))
                .child(SharedString::from(err))
                .child(div().ml_auto().text_color(c(theme::DIM)).child("点击关闭")),
        )
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 挂起的焦点请求（异步流程里无 window，延到这里）
        if let Some(id) = self.pending_focus.take()
            && let Some(t) = self.terminals.get(&id)
        {
            let handle = t.read(cx).focus_handle_clone();
            handle.focus(window, cx);
        }

        let show_tabs = matches!(self.page, Page::Session(_)) && !self.open_order.is_empty();

        let content = div().flex_1().min_h(px(0.)).flex().flex_col().map(|el| {
            match self.page.clone() {
                Page::Session(id) => {
                    let term = self.terminals.get(&id).cloned();
                    let el = match term {
                        Some(t) => el.child(div().flex_1().min_h(px(0.)).child(t)),
                        None => el.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c(theme::FAINT))
                                .child("会话未打开"),
                        ),
                    };
                    if let Some(s) = self.session(&id).cloned()
                        && let Some(qbar) = self.render_question_bar(&s, cx)
                    {
                        el.child(qbar)
                    } else {
                        el
                    }
                }
                Page::Overview => el.child(self.render_overview(cx)),
                Page::Projects => el.child(self.render_projects(cx)),
                Page::Settings => el.child(self.render_settings(window, cx)),
            }
        });

        let mut main = div().flex_1().min_w(px(0.)).flex().flex_col().bg(c(theme::BG));
        if let Some(toast) = self.render_error_toast(cx) {
            main = main.child(toast);
        }
        if !self.ssd_mounted {
            main = main.child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(5.))
                    .bg(ca(theme::AMBER, 0.1))
                    .border_b_1()
                    .border_color(ca(theme::AMBER, 0.4))
                    .text_size(px(11.5))
                    .text_color(c(theme::AMBER))
                    .child("SSD 未挂载：创建 / 启动 / 删除已禁用（绝不建占位目录），挂载恢复后自动解除"),
            );
        }
        if show_tabs {
            main = main.child(self.render_tabs(cx));
        }
        main = main.child(content).child(self.render_statusbar(cx));

        let mut root = div()
            .size_full()
            .flex()
            .bg(c(theme::BG))
            .text_color(c(theme::INK))
            .text_size(px(13.))
            .child(self.render_sidebar(cx))
            .child(main);

        if let Some(modal) = self.render_modal(cx) {
            root = root.child(modal);
        }
        root
    }
}
