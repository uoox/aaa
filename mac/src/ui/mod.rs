//! UI 根视图：侧栏（唯一的会话切换入口）+ 页面区 + 状态栏 + 模态框。

mod kit;
mod mini_input;
mod modals;
mod settings;
mod terminal_view;

use std::collections::HashMap;

use futures::StreamExt;
use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, SharedString, Task, Window, div, prelude::*, px,
};

use crate::model::*;
use crate::net::{ConnState, Net, UiEvent};
use crate::theme;
use kit::*;
use mini_input::MiniInput;
use terminal_view::TerminalView;

#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    Home,
    Session(String),
    Settings,
}

/// 关掉 `closed` 之后停在哪一页：只有关的正是当前页才换页，换到剩下的最近一个
/// 会话，一个都不剩就回主页。`open_order` 传入时已剔除 `closed`。
fn page_after_close(current: &Page, closed: &str, open_order: &[String]) -> Page {
    if current != &Page::Session(closed.to_string()) {
        return current.clone();
    }
    open_order
        .last()
        .map(|x| Page::Session(x.clone()))
        .unwrap_or(Page::Home)
}

/// Ctrl-Tab：在存活会话里循环。`alive` 按侧栏顺序；当前不在列表里（主页 /
/// 设置 / 已退出）就回到第一个。
fn next_session_id(alive: &[String], current: Option<&str>) -> Option<String> {
    if alive.is_empty() {
        return None;
    }
    let ix = current
        .and_then(|c| alive.iter().position(|x| x == c))
        .map(|i| (i + 1) % alive.len())
        .unwrap_or(0);
    Some(alive[ix].clone())
}

pub enum Modal {
    None,
    NewProject {
        agent_idx: usize,
        busy: bool,
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
    /// 项目目录变更确认：迁移 or 仅指向（daemon 会重启）
    ConfirmConfig {
        port: u16,
        token: String,
        old_root: String,
        new_root: String,
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

    // 侧栏宽度（拖右边缘调整，松手落盘）与拖动中的 (按下时鼠标 x, 按下时宽度)
    pub sidebar_w: f32,
    sidebar_drag: Option<(f32, f32)>,

    // 输入框
    pub name_input: Entity<MiniInput>,
    pub host_input: Entity<MiniInput>,
    pub port_input: Entity<MiniInput>,
    pub token_input: Entity<MiniInput>,
    pub root_input: Entity<MiniInput>,
    /// 项目目录输入框只在首次拿到 health 时填一次，之后不覆盖用户输入
    root_input_seeded: bool,

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
        let host_input = cx.new(|cx| MiniInput::new(cx, "127.0.0.1"));
        let port_input = cx.new(|cx| MiniInput::new(cx, "2730"));
        let token_input = cx.new(|cx| MiniInput::new(cx, "aaa_tk_…"));
        let root_input = cx.new(|cx| MiniInput::new(cx, "/Volumes/SSD/project"));
        if let Some(ep) = &endpoint {
            host_input.update(cx, |i, cx| i.set_text(ep.host.clone(), cx));
            port_input.update(cx, |i, cx| i.set_text(ep.port.to_string(), cx));
            token_input.update(cx, |i, cx| i.set_text(ep.token.clone(), cx));
        }

        RootView {
            net,
            page: Page::Home,
            modal: Modal::None,
            conn: ConnState::Connecting,
            health: None,
            ssd_mounted: true,
            sessions: Vec::new(),
            projects: Vec::new(),
            agents: builtin_agents(),
            qr_modules: None,
            endpoint_from_config,
            terminals: HashMap::new(),
            open_order: Vec::new(),
            pending_focus: None,
            stalled: HashMap::new(),
            ports_cache: HashMap::new(),
            last_notified_question: HashMap::new(),
            sidebar_w: UiState::load().sidebar_w,
            sidebar_drag: None,
            name_input,
            host_input,
            port_input,
            token_input,
            root_input,
            root_input_seeded: false,
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
                self.page = page_after_close(&self.page, &id, &self.open_order);
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
                if !r.root_input_seeded && !h.project_root.is_empty() {
                    r.root_input_seeded = true;
                    let root = h.project_root.clone();
                    r.root_input.update(cx, |i, cx| i.set_text(root, cx));
                }
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

    /// 收起本地 tab（不碰 daemon）。侧栏 × 的完整语义在 confirm_kill 里：
    /// 先 kill 会话再调这里收 tab；删除会话、session_removed 也走这条清理。
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.terminals.remove(id);
        self.open_order.retain(|x| x != id);
        self.page = page_after_close(&self.page, id, &self.open_order);
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

    /// Ctrl-Tab / 双击下分区都会走到的「激活会话」帮手
    fn cycle_session(&mut self, cx: &mut Context<Self>) {
        let alive: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.state != SessionState::Exited)
            .map(|s| s.id.clone())
            .collect();
        let cur = match &self.page {
            Page::Session(id) => Some(id.as_str()),
            _ => None,
        };
        if let Some(next) = next_session_id(&alive, cur) {
            self.open_session(next, cx);
        }
    }

    /// App 级快捷键：⌘N/Ctrl-N 新建项目，Ctrl-Tab 切换激活会话。
    /// 挂在根节点上吃冒泡：终端把 Ctrl-Tab 放行、⌘ 组合本来就不吞。
    fn on_root_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if ks.key == "n" && (m.platform || m.control) {
            if matches!(self.modal, Modal::None) {
                self.open_new_project_modal(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if ks.key == "tab" && m.control {
            if matches!(self.modal, Modal::None) {
                self.cycle_session(cx);
            }
            cx.stop_propagation();
            return;
        }
        // 新建项目弹窗里回车 = 「创建并进入」。MiniInput 不消费 enter，
        // 这里在根上接住（IME 组字中的确认回车走 input handler，到不了这）。
        if ks.key == "enter"
            && let Modal::NewProject { agent_idx, .. } = self.modal
        {
            self.confirm_create_project(agent_idx, cx);
            cx.stop_propagation();
        }
    }

    // ── 侧栏 ───────────────────────────────────────────────────────────
    //
    // 结构（自上而下）：新建按钮 → 激活的会话（TUI/Shell 开着的） → 分隔线 →
    // 未激活的项目（双击开启会话）。没有大标题、没有总览页——侧栏本身就是
    // 全部导航。

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let row_base = |id: gpui::ElementId| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(5.))
                .mx(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
        };

        // ── 上分区：存活会话 ────────────────────────────────────────────
        let alive: Vec<Session> = self
            .sessions
            .iter()
            .filter(|s| s.state != SessionState::Exited)
            .cloned()
            .collect();
        let alive_paths: Vec<&str> = alive.iter().map(|s| s.project_path.as_str()).collect();

        let mut active_col = div().flex().flex_col().gap(px(1.));
        for (ix, s) in alive.iter().enumerate() {
            let id = s.id.clone();
            let id_close = s.id.clone();
            let active = self.page == Page::Session(id.clone());
            let is_stalled = s.state == SessionState::Running && self.stalled.contains_key(&s.id);
            let agent_label: SharedString = if s.agent == "shell" {
                "term".into()
            } else {
                s.agent.clone().into()
            };
            active_col = active_col.child(
                row_base(("sb-sess", ix).into())
                    .group("sb-row")
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
                            .text_color(c(theme::INK))
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
                    )
                    .child(
                        // × = 关闭这个 TUI/Shell：终止进程、项目回到下分区。
                        // 一律先弹确认——还在跑的 agent 被顺手点掉最伤。
                        div()
                            .id(("sb-close", ix))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(c(theme::FAINT))
                            .hover(|st| st.text_color(c(theme::RED)).bg(c(theme::EDGE_LIGHT)))
                            // 非当前行悬停才现身；invisible 连命中盒一起去掉
                            .when(!active, |el| {
                                el.invisible().group_hover("sb-row", |st| st.visible())
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.modal = Modal::ConfirmKill {
                                    id: id_close.clone(),
                                };
                                cx.notify();
                            }))
                            .child("✕"),
                    ),
            );
        }

        // ── 下分区：未激活的项目（双击开启会话并移入上分区） ─────────────
        let mut idle_col = div().flex().flex_col().gap(px(1.));
        for (ix, p) in self
            .projects
            .iter()
            .filter(|p| !alive_paths.contains(&p.path.as_str()))
            .enumerate()
        {
            let proj = p.clone();
            let del_path = p.path.clone();
            let title = p
                .session_title
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| p.name.clone());
            let agent_label: SharedString = match p.agent.as_deref() {
                Some("shell") => "term".into(),
                Some(a) => a.to_string().into(),
                None => "".into(),
            };
            idle_col = idle_col.child(
                row_base(("sb-proj", ix).into())
                    .group("sb-idle")
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                        if ev.click_count() >= 2 {
                            this.open_project(&proj, cx);
                        }
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.))
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(title)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(c(theme::FAINT))
                            .child(agent_label),
                    )
                    .child(
                        div()
                            .id(("sb-del", ix))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(c(theme::FAINT))
                            .hover(|st| st.text_color(c(theme::RED)).bg(c(theme::EDGE_LIGHT)))
                            .invisible()
                            .group_hover("sb-idle", |st| st.visible())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.modal = Modal::DeleteConfirm {
                                    paths: vec![del_path.clone()],
                                    report: None,
                                    busy: false,
                                };
                                cx.notify();
                            }))
                            .child("删"),
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

        div()
            .w(px(self.sidebar_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden() // 拖窄时标题按 ellipsis 收，不许挤出侧栏
            .bg(c(theme::SURFACE))
            .child(
                row_base("sb-new".into())
                    .mt(px(10.))
                    .mb(px(4.))
                    .border_1()
                    .border_color(c(theme::EDGE_LIGHT))
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)).border_color(c(theme::CYAN)))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_new_project_modal(window, cx);
                    }))
                    .child(div().text_size(px(13.)).text_color(c(theme::CYAN)).child("＋"))
                    .child(div().flex_1().text_size(px(12.5)).child("新建项目"))
                    .child(
                        div()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(c(theme::FAINT))
                            .child("⌘N"),
                    ),
            )
            .child(
                div()
                    .id("sb-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .child(active_col)
                    .child(
                        div()
                            .h(px(1.))
                            .mx(px(10.))
                            .my(px(7.))
                            .bg(c(theme::EDGE)),
                    )
                    .child(idle_col),
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
                                cx.notify();
                            }))
                            .child("⚙"),
                    ),
            )
    }

    /// 侧栏右边缘的拖拽把手：兼作原来的分隔线，所以侧栏本身不再画 border_r。
    fn render_sidebar_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let dragging = self.sidebar_drag.is_some();
        div()
            .id("sidebar-resizer")
            .w(px(4.))
            .flex_none()
            .h_full()
            .cursor_col_resize()
            .bg(c(if dragging { theme::CYAN } else { theme::EDGE }))
            .hover(|st| st.bg(c(theme::CYAN)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    // 记按下时的锚点而不是逐帧累加 delta：中途丢帧也不会漂。
                    this.sidebar_drag = Some((f32::from(ev.position.x), this.sidebar_w));
                    cx.notify();
                }),
            )
    }

    fn on_sidebar_drag(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((from_x, from_w)) = self.sidebar_drag else {
            return;
        };
        let w = clamp_sidebar_width(from_w + (f32::from(ev.position.x) - from_x));
        if w != self.sidebar_w {
            self.sidebar_w = w;
            cx.notify();
        }
    }

    fn on_sidebar_drag_end(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.sidebar_drag.take().is_some() {
            // 只在松手时落盘：拖动中每帧写文件没有意义
            UiState {
                sidebar_w: self.sidebar_w,
            }
            .save();
            cx.notify();
        }
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
                Page::Home => el.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(6.))
                        .text_color(c(theme::FAINT))
                        .child(div().text_size(px(13.)).child("双击左侧项目开启会话"))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_family("Menlo")
                                .child("⌘N 新建项目 · ⌃Tab 切换会话"),
                        ),
                ),
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
        main = main.child(content).child(self.render_statusbar(cx));

        let mut root = div()
            .size_full()
            .flex()
            .bg(c(theme::BG))
            .text_color(c(theme::INK))
            .text_size(px(13.))
            .on_key_down(cx.listener(Self::on_root_key))
            .child(self.render_sidebar(cx))
            .child(self.render_sidebar_resizer(cx))
            .child(main);

        // 拖动中把 move/up 挂到根上：4px 的把手留不住指针，只有根覆盖整窗。
        // 不拖时不挂，免得每次鼠标移动都空跑一遍监听。
        if self.sidebar_drag.is_some() {
            root = root
                .on_mouse_move(cx.listener(Self::on_sidebar_drag))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::on_sidebar_drag_end))
                // 甩出窗口才松手时根命中盒不算 hover，up 走不到上面那条；
                // 不接住它侧栏就会一直粘着鼠标走
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_sidebar_drag_end));
        }

        if let Some(modal) = self.render_modal(cx) {
            root = root.child(modal);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn closing_other_tab_does_not_switch_page() {
        let cur = Page::Session("s_1".into());
        assert_eq!(
            page_after_close(&cur, "s_2", &ids(&["s_1"])),
            Page::Session("s_1".into())
        );
        // 在别的页上关 tab 也不该被拽走
        assert_eq!(
            page_after_close(&Page::Settings, "s_2", &ids(&["s_1"])),
            Page::Settings
        );
    }

    #[test]
    fn ctrl_tab_cycles_and_wraps() {
        let alive = ids(&["a", "b", "c"]);
        assert_eq!(next_session_id(&alive, Some("a")).as_deref(), Some("b"));
        assert_eq!(next_session_id(&alive, Some("c")).as_deref(), Some("a"), "回绕");
        // 当前不在列表（主页/设置/会话刚退出）→ 回到第一个
        assert_eq!(next_session_id(&alive, None).as_deref(), Some("a"));
        assert_eq!(next_session_id(&alive, Some("gone")).as_deref(), Some("a"));
        assert_eq!(next_session_id(&[], Some("a")), None, "没有存活会话就不动");
    }

    #[test]
    fn closing_current_tab_falls_back() {
        let cur = Page::Session("s_2".into());
        // 回到剩下的最近一个
        assert_eq!(
            page_after_close(&cur, "s_2", &ids(&["s_1", "s_3"])),
            Page::Session("s_3".into())
        );
        // 一个都不剩 → 总览
        assert_eq!(page_after_close(&cur, "s_2", &[]), Page::Home);
    }
}
