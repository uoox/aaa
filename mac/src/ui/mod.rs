//! UI 根视图：侧栏（唯一的会话切换入口）+ 页面区 + 状态栏 + 模态框。

mod detail_panel;
mod history;
mod kit;
mod messages_view;
mod mini_input;
mod modals;
mod settings;
mod stream_fold;
mod terminal_panel;
mod terminal_view;

use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, SharedString, Task, Window, div, prelude::*, px,
};

use crate::model::*;
use crate::net::{ConnState, Net, UiEvent};
use crate::theme::{self, ThemeKind};
use kit::*;
use messages_view::MessagesView;
use mini_input::MiniInput;
use terminal_view::TerminalView;

#[derive(Debug, Clone, PartialEq)]
pub enum Page {
    Home,
    Session(String),
    Settings,
    /// 常驻多标签终端面板（侧栏底部入口），标签 = 存活的 shell 会话
    Terminal,
    /// 会话日志（侧栏底部入口）：所有出现过的会话，含已退出、已删除
    History,
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

/// 存活的项目会话（running / waiting，含 asking）。**exited 会话不进侧栏**——进程没了
/// 它就只是历史，项目行标「未激活」，点一下即 resume。终端（shell）不是项目会话，
/// 归终端面板管（PROTOCOL「终端」）。
fn is_active(s: &Session) -> bool {
    !s.is_terminal() && s.state != SessionState::Exited
}

/// 关闭前要不要确认：只有还在执行（running 且不在问）的会话被顺手点掉最伤。
/// waiting / asking 都是「等你」，exited 没进程可杀——这些关掉没损失，不打断。
fn kill_needs_confirm(s: &Session) -> bool {
    s.state == SessionState::Running && !s.asking
}

/// 项目行的状态字（2026-09-06 用户拍板，三端一致；不再用色点）：
/// 执行中 = 会话在跑；待回复 = 弹着选项等你选，不选就卡住（`asking`，哪怕屏幕还在变）；
/// 已激活 = 会话活着、停在输入框轮到你（waiting）；未激活 = 没有存活会话
/// （退出了 / 只有旧对话 / 从没跑过）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowStatus {
    Running,
    Asking,
    Active,
    Inactive,
}

impl RowStatus {
    fn of(session: Option<&Session>) -> RowStatus {
        match session {
            Some(s) if is_active(s) && s.asking => RowStatus::Asking,
            Some(s) if is_active(s) && s.state == SessionState::Running => RowStatus::Running,
            Some(s) if is_active(s) => RowStatus::Active,
            _ => RowStatus::Inactive,
        }
    }

    fn label(self) -> &'static str {
        match self {
            RowStatus::Running => "执行中",
            RowStatus::Asking => "待回复",
            RowStatus::Active => "已激活",
            RowStatus::Inactive => "未激活",
        }
    }

    fn color(self) -> u32 {
        match self {
            RowStatus::Running => theme::green(),
            RowStatus::Asking => theme::amber(),
            RowStatus::Active => theme::accent(),
            RowStatus::Inactive => theme::faint(),
        }
    }
}

/// 侧栏一行：一个项目（2026-09-06 起单列，不再分「激活 / 未激活」两栏）。
#[derive(Debug, Clone)]
struct ProjectRow {
    path: String,
    title: String,
    status: RowStatus,
    /// 存活的项目会话（点行即打开）；None = 未激活，点行 resume
    session: Option<Session>,
    /// 注册表里的项目；只有会话、没登记的目录为 None（只能看，不能 resume / 删）
    project: Option<Project>,
    /// 排序键：该项目最新一条会话的 `updated_at`（老 daemon 退到 created_at），
    /// 没有会话的用目录 mtime。都是 ISO 时间串，字典序即时间序。
    sort_key: String,
    /// 置顶的排在最前（组内仍按 sort_key）
    pinned: bool,
}

fn session_updated(s: &Session) -> &str {
    if !s.updated_at.is_empty() {
        &s.updated_at
    } else if !s.created_at.is_empty() {
        &s.created_at
    } else {
        &s.last_output_at
    }
}

/// 项目 × 会话 → 侧栏行，按最近更新的会话在前（同刻按名字稳住）。一个项目一行：
/// 存活的项目会话代表它（几个同时活着取最近更新的）；退出的会话只贡献排序时间。
/// 有存活会话但注册表里没有的目录也给一行（别处 `aaa open` 开的），否则它无处可点。
fn project_rows(projects: &[Project], sessions: &[Session]) -> Vec<ProjectRow> {
    let mut rows: Vec<ProjectRow> = Vec::with_capacity(projects.len());
    let mut seen: HashSet<&str> = HashSet::new();
    let by_path = |path: &str| -> (Option<&Session>, Option<&str>) {
        let mut live: Option<&Session> = None;
        let mut latest: Option<&str> = None;
        for s in sessions.iter().filter(|s| !s.is_terminal() && s.project_path == path) {
            let t = session_updated(s);
            if latest.is_none_or(|l| t > l) {
                latest = Some(t);
            }
            if is_active(s) && live.is_none_or(|l| t > session_updated(l)) {
                live = Some(s);
            }
        }
        (live, latest)
    };
    for p in projects {
        seen.insert(p.path.as_str());
        let (live, latest) = by_path(&p.path);
        // 活着的会话的名字 → daemon 从 agent 存储读出的对话名 → 文件夹名
        let title = live
            .map(|s| s.title.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| p.session_title.clone().filter(|t| !t.is_empty()))
            .unwrap_or_else(|| p.name.clone());
        rows.push(ProjectRow {
            path: p.path.clone(),
            title,
            status: RowStatus::of(live),
            session: live.cloned(),
            project: Some(p.clone()),
            sort_key: latest.unwrap_or(p.mtime.as_str()).to_owned(),
            pinned: p.pinned,
        });
    }
    for s in sessions.iter().filter(|s| is_active(s)) {
        if seen.contains(s.project_path.as_str()) {
            continue;
        }
        seen.insert(s.project_path.as_str());
        let (live, latest) = by_path(&s.project_path);
        rows.push(ProjectRow {
            path: s.project_path.clone(),
            title: live.map(Session::display_title).unwrap_or_else(|| s.display_title()),
            status: RowStatus::of(live),
            session: live.cloned(),
            project: None,
            sort_key: latest.unwrap_or("").to_owned(),
            pinned: false,
        });
    }
    // 置顶的在最前，其余按最近更新；同刻按标题稳住
    rows.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then_with(|| b.sort_key.cmp(&a.sort_key))
            .then_with(|| a.title.cmp(&b.title))
    });
    rows
}

/// ⌃Tab 循环的候选：存活的项目会话，按侧栏顺序。
fn cyclable_ids(projects: &[Project], sessions: &[Session]) -> Vec<String> {
    project_rows(projects, sessions)
        .into_iter()
        .filter_map(|r| r.session.map(|s| s.id))
        .collect()
}

/// shell 会话按 `created_at` 升序（末尾最新），不看状态——回收 exited 标签时
/// 要按「它还在时」的顺序挑邻居，所以状态过滤留给调用方。
fn sorted_terminals<'a>(sessions: impl IntoIterator<Item = &'a Session>) -> Vec<&'a Session> {
    let mut v: Vec<&Session> = sessions.into_iter().filter(|s| s.is_terminal()).collect();
    // created_at 是同一格式的 ISO 时间串，字典序即时间序；同刻按 id 稳住顺序
    v.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    v
}

/// 终端面板的标签：存活的 shell 会话按 `created_at` 升序（末尾最新），标
/// 「终端 N」；不在项目根开的追加 ` · <目录名>`（PROTOCOL「终端」）。
/// 返回 (会话 id, 标签文字)。
fn terminal_tabs<'a>(
    sessions: impl IntoIterator<Item = &'a Session>,
    root: &str,
) -> Vec<(String, String)> {
    let live = sorted_terminals(
        sessions
            .into_iter()
            .filter(|s| s.state != SessionState::Exited),
    );
    let root = root.trim_end_matches('/');
    live.iter()
        .enumerate()
        .map(|(i, s)| {
            let mut label = format!("终端 {}", i + 1);
            let path = s.project_path.trim_end_matches('/');
            if !path.is_empty() && path != root {
                let leaf = path
                    .rsplit('/')
                    .next()
                    .filter(|l| !l.is_empty())
                    .unwrap_or(path);
                label.push_str(" · ");
                label.push_str(leaf);
            }
            (s.id.clone(), label)
        })
        .collect()
}

/// 关掉标签 `closed` 后该激活哪个：优先右邻，没有就左邻（浏览器的习惯）；
/// `closed` 不在列表里（已经被别处关掉）就退到最新的一个。`tabs` 含 `closed`。
fn next_terminal_after_close(tabs: &[String], closed: &str) -> Option<String> {
    let Some(pos) = tabs.iter().position(|t| t == closed) else {
        return tabs.last().cloned();
    };
    tabs.get(pos + 1)
        .or_else(|| pos.checked_sub(1).and_then(|p| tabs.get(p)))
        .cloned()
}

/// 当前激活标签仍存活就用它，否则回落到最新的（`tabs` 按 created_at 升序）。
fn resolve_active_terminal(active: Option<&str>, tabs: &[String]) -> Option<String> {
    match active {
        Some(a) if tabs.iter().any(|t| t == a) => Some(a.to_string()),
        _ => tabs.last().cloned(),
    }
}

/// 列表排序键：按开启时间（`created_at` 升序，末尾最新）再按 id 稳住。
/// 状态、最近输出都**不**参与排序——多个会话同时在跑时，按状态/活跃度排会
/// 让行在侧栏里跳来跳去，点都点不准；状态交给行首色点表达。
/// created_at 是同一格式的 ISO 时间串，字典序即时间序。
fn sort_key(s: &Session) -> (&str, &str) {
    (s.created_at.as_str(), s.id.as_str())
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
    /// 重启 daemon 会终止全部存活会话：有几个就先问一声
    ConfirmRestart {
        alive: usize,
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
    /// 配对二维码模块（(宽, 黑白位图)；fetch 时编码一次，渲染帧只读）
    pub qr_modules: Option<(usize, Vec<bool>)>,
    pub endpoint_from_config: bool,

    // 终端视图（会话页与终端面板共用一张表：key = 会话 id）
    terminals: HashMap<String, Entity<TerminalView>>,
    /// 会话页的打开顺序（关当前页时回落用）；终端标签不进这里
    open_order: Vec<String>,
    pending_focus: Option<String>,

    // 终端面板
    /// 当前标签；None / 指向已死会话时回落到最新存活的
    active_terminal: Option<String>,
    /// 已发过 DELETE 的终端会话：exited 的 shell 只删一次，关标签的也记在这
    /// （随后的 exited 帧不再重复删），session_removed 时清掉
    deleted_terminals: HashSet<String>,

    // v1.1 消息流：按需创建的视图 + 处于消息流模式的会话（⌘E 切换）
    msg_views: HashMap<String, Entity<MessagesView>>,
    msg_mode: HashSet<String>,

    // 会话监听端口缓存（Web 预览）
    pub ports_cache: HashMap<String, Vec<PortEntry>>,
    /// 本机手动终止的会话：exited 不弹「已退出」（自己动的手）
    pub user_killed: HashSet<String>,

    // 侧栏宽度（拖右边缘调整，松手落盘）与拖动中的 (按下时鼠标 x, 按下时宽度)
    pub sidebar_w: f32,
    sidebar_drag: Option<(f32, f32)>,
    /// 当前主题（与 theme::current 同步）：设置页切换，随 ui.toml 落盘
    pub theme: ThemeKind,

    // 详情面板（会话页右侧，⌘I）
    /// 面板展开与否，随 ui.toml 落盘
    pub detail_visible: bool,
    /// 静音通知的项目路径，随 ui.toml 落盘
    pub muted_projects: Vec<String>,
    /// 套餐用量（GET /usage + usage 帧）；None = 没有套餐信息，侧栏不画
    pub plan: Option<PlanUsage>,
    /// 会话日志（GET /history），打开「历史」页时拉
    pub history: Vec<HistoryEntry>,
    /// 日历（GET /history/days）
    pub history_days: Vec<DayDigest>,
    /// 历史页：日历里选中的日期（本地 YYYY-MM-DD）；None = 全部
    pub history_day: Option<String>,
    /// 历史页正在看的月份（YYYY-MM）
    pub history_month: String,
    /// 历史页搜索框
    pub history_input: Entity<MiniInput>,
    /// 每会话的产物 / 改动状态（含各自的拉取节流器）
    detail: HashMap<String, detail_panel::SessionDetail>,
    /// 项目路径 → 收件箱条目
    inbox: HashMap<String, Vec<InboxItem>>,
    /// 收件箱新增输入框（回车提交，根节点接住）
    pub inbox_input: Entity<MiniInput>,

    // 输入框
    /// 侧栏顶部的新建项目输入框：内容即文件夹名，回车 / ＋ 创建
    pub new_input: Entity<MiniInput>,
    /// 正在 POST /projects + /sessions：挡住第二次回车
    pub creating: bool,
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
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 窗口回到前台：正在看的那个终端把尺寸夺回来（手机看过之后 PTY 是手机的行列）
        cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.reassert_visible_size(cx);
            }
        })
        .detach();
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
        // 系统通知点击 → 回到 App、打开那条会话（notify.rs 把会话 id 丢进这条通道）
        let mut clicks = crate::notify::install();
        cx.spawn(async move |this, cx| {
            while let Some(id) = clicks.next().await {
                if this
                    .update(cx, |root: &mut RootView, cx| {
                        cx.activate(true);
                        if root.session(&id).is_some() {
                            root.open_session(id, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        // 本机偏好先于首帧：主题必须在第一次 render 之前生效，否则会闪一帧黑暗
        let ui_state = UiState::load();
        let theme_kind = ThemeKind::from_str(&ui_state.theme);
        theme::set_current(theme_kind);

        let new_input = cx.new(|cx| MiniInput::new(cx, "新建项目：文件夹名，回车"));
        let name_input = cx.new(|cx| MiniInput::new(cx, "新名字"));
        let host_input = cx.new(|cx| MiniInput::new(cx, "127.0.0.1"));
        let port_input = cx.new(|cx| MiniInput::new(cx, "2730"));
        let token_input = cx.new(|cx| MiniInput::new(cx, "aaa_tk_…"));
        let root_input = cx.new(|cx| MiniInput::new(cx, "~/project"));
        let inbox_input = cx.new(|cx| MiniInput::new(cx, "加一条，Claude 空下来时自动喂给它"));
        let history_input = cx.new(|cx| MiniInput::new(cx, "搜索：标题 / 项目 / 清单"));
        // 输入法送来的回车（见 MiniInput::replace_text_in_range）与键盘回车同一出口
        cx.subscribe(&new_input, |this, _, _: &mini_input::InputEvent, cx| {
            if matches!(this.modal, Modal::None) {
                this.create_project(cx);
            }
        })
        .detach();
        cx.subscribe(&inbox_input, |this, _, _: &mini_input::InputEvent, cx| {
            if matches!(this.modal, Modal::None) {
                this.inbox_add(cx);
            }
        })
        .detach();
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
            qr_modules: None,
            endpoint_from_config,
            terminals: HashMap::new(),
            open_order: Vec::new(),
            pending_focus: None,
            active_terminal: None,
            deleted_terminals: HashSet::new(),
            msg_views: HashMap::new(),
            msg_mode: HashSet::new(),
            ports_cache: HashMap::new(),
            user_killed: HashSet::new(),
            sidebar_w: ui_state.sidebar_w,
            sidebar_drag: None,
            theme: theme_kind,
            detail_visible: ui_state.detail_visible,
            muted_projects: ui_state.muted_projects,
            plan: None,
            history: Vec::new(),
            history_days: Vec::new(),
            history_day: None,
            history_month: chrono::Local::now().format("%Y-%m").to_string(),
            history_input,
            detail: HashMap::new(),
            inbox: HashMap::new(),
            inbox_input,
            new_input,
            creating: false,
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
                        // hello 后必然跟整屏 replay：清掉旧模型，重连不叠历史
                        t.reset_for_replay(cx);
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
                self.sync_msg_alive_all(cx);
                self.sessions_changed(cx);
                cx.notify();
            }
            DaemonEvent::Session { session } => {
                self.maybe_notify(&session, cx);
                self.upsert_session(session, cx);
            }
            DaemonEvent::SessionRemoved { id } => {
                // 当前终端标签被别处（CLI / 手机）删掉：趁顺序还在先挑好邻居
                if self.active_terminal.as_deref() == Some(id.as_str()) {
                    let order = self.live_terminal_ids();
                    self.active_terminal = next_terminal_after_close(&order, &id);
                }
                self.sessions.retain(|s| s.id != id);
                self.terminals.remove(&id);
                self.msg_views.remove(&id);
                self.msg_mode.remove(&id);
                self.open_order.retain(|x| x != &id);
                self.ports_cache.remove(&id);
                self.user_killed.remove(&id);
                self.deleted_terminals.remove(&id);
                self.forget_detail(&id);
                self.page = page_after_close(&self.page, &id, &self.open_order);
                self.sessions_changed(cx);
                cx.notify();
            }
            DaemonEvent::ProjectsChanged {} => self.fetch_projects(cx),
            DaemonEvent::Health { ssd_mounted } => {
                self.ssd_mounted = ssd_mounted;
                cx.notify();
            }
            DaemonEvent::MessagesChanged { id, last_seq } => {
                // 只有已打开消息流视图的会话才增量拉（不做无谓轮询）
                if let Some(v) = self.msg_views.get(&id) {
                    v.update(cx, |v, cx| v.fetch_if_behind(last_seq, cx));
                }
                // 详情面板正看着它：产物 / 改动按节流重拉
                self.on_detail_messages_changed(&id, cx);
            }
            DaemonEvent::Usage { plan } => {
                self.plan = plan;
                cx.notify();
            }
            DaemonEvent::InboxChanged { path } => self.on_inbox_changed(&path, cx),
            DaemonEvent::Unknown => {}
        }
    }

    /// ⌘E：当前会话在 终端 ⇄ 消息流 之间切换；视图按需创建
    fn toggle_msg_mode(&mut self, cx: &mut Context<Self>) {
        let Page::Session(id) = self.page.clone() else {
            return;
        };
        if self.msg_mode.contains(&id) {
            self.msg_mode.remove(&id);
            self.pending_focus = Some(id);
        } else {
            let net = self.net.clone();
            let sid = id.clone();
            // 新建的视图默认当会话活着；这里立刻用真实状态校准，已退出的会话
            // 里悬着的表单不能是可交互的
            let (alive, created) = self
                .session(&id)
                .map(|s| (s.state != SessionState::Exited, s.created_at.clone()))
                .unwrap_or((false, String::new()));
            let project_path = self.session(&id).map(|s| s.project_path.clone()).unwrap_or_default();
            self.msg_views
                .entry(id.clone())
                .or_insert_with(|| cx.new(|cx| MessagesView::new(sid, net, cx)))
                .update(cx, |v, cx| {
                    v.set_project_path(project_path);
                    v.set_session(alive, Some(&created), cx);
                    v.fetch(cx);
                    v.request_focus(cx);
                });
            self.msg_mode.insert(id);
        }
        cx.notify();
    }

    /// 当前会话此刻是否显示消息流（不支持的会话自动回落终端）
    fn msg_mode_active(&self, id: &str, cx: &Context<Self>) -> bool {
        self.msg_mode.contains(id)
            && self
                .msg_views
                .get(id)
                .is_some_and(|v| v.read(cx).supported != Some(false))
    }

    /// 系统通知：只有一种——「完成」（2026-09-02 用户拍板，PROTOCOL「WS」通知策略）。
    /// running→waiting（这轮干完了）与 running→exited（非本机手动 kill）各弹一条。
    /// 不识别里面在问什么、不按问题去重、没有冷却、没有空转告警；问题本身由
    /// 消息流按结构化数据原生呈现。终端不通知：shell 退出不是「完成」。
    fn maybe_notify(&mut self, new: &Session, cx: &Context<Self>) {
        if new.is_terminal() {
            return;
        }
        let old_state = self
            .sessions
            .iter()
            .find(|s| s.id == new.id)
            .map(|s| s.state);
        if old_state != Some(SessionState::Running) {
            return;
        }
        // 用户正盯着这个会话（窗口前台 + 当前页就是它）就别弹通知——
        // 眼皮底下跑完的东西再弹一条只是噪音
        let watching =
            self.page == Page::Session(new.id.clone()) && cx.active_window().is_some();
        // 详情面板里静音了这个项目：一条都不弹
        let muted = is_muted(&self.muted_projects, &new.project_path);
        match new.state {
            SessionState::Waiting => {
                if !watching && !muted {
                    crate::notify::send(&new.display_title(), "完成 · 等你下一步", &new.id);
                }
            }
            SessionState::Exited => {
                // 自己在 app 里 kill 的不弹；标记无论如何都要消耗掉
                let killed_here = self.user_killed.remove(&new.id);
                if !watching && !killed_here && !muted {
                    let body = match new.exit_code {
                        Some(code) => format!("已退出 (exit {code})"),
                        None => "已退出".to_string(),
                    };
                    crate::notify::send(&new.display_title(), &body, &new.id);
                }
            }
            SessionState::Running => {}
        }
    }

    fn upsert_session(&mut self, session: Session, cx: &mut Context<Self>) {
        let id = session.id.clone();
        let alive = session.state != SessionState::Exited;
        let created = session.created_at.clone();
        match self.sessions.iter_mut().find(|s| s.id == session.id) {
            Some(slot) => *slot = session,
            None => self.sessions.push(session),
        }
        self.sort_sessions();
        if let Some(v) = self.msg_views.get(&id) {
            v.update(cx, |v, cx| v.set_session(alive, Some(&created), cx));
        }
        self.sessions_changed(cx);
        cx.notify();
    }

    /// 全量会话列表（快照 / 重连拉取）到达后，把存活状态同步给每个已开的消息流视图；
    /// 列表里没有的会话按已死处理（对话框随进程一起没了）
    fn sync_msg_alive_all(&self, cx: &mut Context<Self>) {
        for (id, view) in &self.msg_views {
            let (alive, created) = self
                .sessions
                .iter()
                .find(|s| &s.id == id)
                .map(|s| (s.state != SessionState::Exited, s.created_at.clone()))
                .unwrap_or((false, String::new()));
            view.update(cx, |v, cx| v.set_session(alive, Some(&created), cx));
        }
    }

    fn sort_sessions(&mut self) {
        self.sessions.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
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
            self.net.sessions(),
            |r, s: Vec<Session>, cx| {
                r.sessions = s;
                r.sort_sessions();
                r.sync_msg_alive_all(cx);
                r.sessions_changed(cx);
                cx.notify();
            },
            false,
            cx,
        );
        self.fetch_projects(cx);
        self.fetch_usage(cx);
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
        // shell 会话没有会话页：一律去终端面板（旧注册表项目、别处开的 shell 都走这）
        if self.session(&id).is_some_and(Session::is_terminal) {
            self.focus_terminal(id, cx);
            return;
        }
        if self.ensure_terminal_view(&id, cx) {
            self.open_order.push(id.clone());
        }
        self.fetch_ports(id.clone(), cx);
        self.page = Page::Session(id.clone());
        self.pending_focus = Some(id.clone());
        self.reassert_visible_size(cx);
        // 详情面板开着就把这个会话的产物 / 改动 / 收件箱补齐
        self.refresh_detail(&id, cx);
        cx.notify();
    }

    /// 正在看的终端（会话页的那个，或主页终端面板的当前标签）重新宣告尺寸。
    fn reassert_visible_size(&mut self, cx: &mut Context<Self>) {
        let id = match &self.page {
            Page::Session(id) => Some(id.clone()),
            _ => self.active_terminal.clone(),
        };
        if let Some(t) = id.and_then(|id| self.terminals.get(&id).cloned()) {
            t.update(cx, |v, _| v.reassert_size());
        }
    }

    /// 没有就建这个会话的终端视图（attach WS 随之建立）。返回是否新建。
    /// 会话页与终端面板共用：前者另记 open_order，后者不记。
    fn ensure_terminal_view(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if self.terminals.contains_key(id) {
            return false;
        }
        let net = self.net.clone();
        let sid = id.to_string();
        let term = cx.new(|cx| TerminalView::new(sid, &net, cx));
        self.terminals.insert(id.to_string(), term);
        true
    }

    /// 收起本地 tab（不碰 daemon）。侧栏 × 的完整语义在 kill_session 里：
    /// 先 kill 会话再调这里收 tab；删除会话、session_removed 也走这条清理。
    pub fn close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.terminals.remove(id);
        self.msg_views.remove(id);
        self.msg_mode.remove(id);
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

    /// 项目行双击 / 打开按钮：resume 最近会话。
    /// 注册表里 agent=shell 的旧项目：没有会话页可开，改在该目录开终端标签
    /// （不 fresh：已有存活 shell 就切过去，双击「没反应」再点不会开出第二个）。
    pub fn open_project(&mut self, project: &Project, cx: &mut Context<Self>) {
        let agent = project.agent.clone().unwrap_or_else(|| "claude".into());
        if agent == "shell" {
            self.create_terminal_in(project.path.clone(), false, cx);
            return;
        }
        let fut = self.net.create_session(project.path.clone(), agent, true);
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

    /// 置顶开关：POST /projects/pin；列表靠 projects_changed 帧重拉，这里先乐观改一下
    fn set_pinned(&mut self, path: String, pinned: bool, cx: &mut Context<Self>) {
        if let Some(p) = self.projects.iter_mut().find(|p| p.path == path) {
            p.pinned = pinned;
        }
        let fut = self.net.set_pinned(&path, pinned);
        self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, true, cx);
        cx.notify();
    }

    fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    /// Ctrl-Tab / 双击下分区都会走到的「激活会话」帮手
    fn cycle_session(&mut self, cx: &mut Context<Self>) {
        let alive = cyclable_ids(&self.projects, &self.sessions);
        let cur = match &self.page {
            Page::Session(id) => Some(id.as_str()),
            _ => None,
        };
        if let Some(next) = next_session_id(&alive, cur) {
            self.open_session(next, cx);
        }
    }

    /// App 级快捷键：⌘N/Ctrl-N 光标进新建项目输入框，Ctrl-Tab 切换激活会话，⌘E 消息流⇄终端，
    /// ⌘W 关闭当前会话 / 终端标签。
    /// 挂在根节点上吃冒泡：终端把 Ctrl-Tab 放行、⌘ 组合本来就不吞。
    fn on_root_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if ks.key == "n" && (m.platform || m.control) {
            if matches!(self.modal, Modal::None) {
                self.focus_new_project(window, cx);
            }
            cx.stop_propagation();
            return;
        }
        if ks.key == "w" && m.platform {
            // 弹窗开着时不接：⌘W 关掉底下的会话而弹窗还在，太诡异
            if !matches!(self.modal, Modal::None) {
                return;
            }
            match self.page.clone() {
                // 与侧栏 × 同一套语义：执行中 → 确认后终止；等你的直接终止；
                // 已退出的没进程可杀，只收起这个页（记录留着，下次双击 resume）
                Page::Session(id) => {
                    let Some(s) = self.session(&id) else { return };
                    if s.state == SessionState::Exited {
                        self.close_tab(&id, cx);
                    } else {
                        let confirm = kill_needs_confirm(s);
                        self.request_kill(id, confirm, cx);
                    }
                    cx.stop_propagation();
                }
                // 终端标签便宜，不问直接关
                Page::Terminal => {
                    if let Some(id) = self.active_terminal.clone() {
                        self.close_terminal(&id, cx);
                        cx.stop_propagation();
                    }
                }
                Page::Home | Page::Settings | Page::History => {}
            }
            return;
        }
        if ks.key == "tab" && m.control {
            if matches!(self.modal, Modal::None) {
                self.cycle_session(cx);
            }
            cx.stop_propagation();
            return;
        }
        if ks.key == "e" && m.platform {
            if matches!(self.modal, Modal::None) {
                self.toggle_msg_mode(cx);
            }
            cx.stop_propagation();
            return;
        }
        // ⌘I：会话页右侧详情面板开 / 关
        if ks.key == "i" && m.platform {
            if matches!(self.modal, Modal::None) {
                self.toggle_detail(cx);
            }
            cx.stop_propagation();
            return;
        }
        // 新建项目输入框里回车 = 「创建并进入」。MiniInput 不消费 enter，
        // 这里在根上接住（IME 组字中的确认回车走 input handler，到不了这）。
        if ks.key == "enter"
            && matches!(self.modal, Modal::None)
            && self.new_input.read(cx).focus_handle.is_focused(window)
        {
            // IME 组字中的回车是「确认候选词」，不是「创建」——不能抢
            if self.new_input.read(cx).composing() {
                return;
            }
            self.create_project(cx);
            cx.stop_propagation();
            return;
        }
        // 详情面板收件箱输入框里回车 = 加一条
        if ks.key == "enter"
            && matches!(self.modal, Modal::None)
            && self.inbox_input.read(cx).focus_handle.is_focused(window)
        {
            if self.inbox_input.read(cx).composing() {
                return;
            }
            self.inbox_add(cx);
            cx.stop_propagation();
        }
    }

    // ── 侧栏 ───────────────────────────────────────────────────────────
    //
    // 结构（自上而下）：新建项目输入框 → 项目列表（一项目一行，状态字 + 标题，
    // 最近更新的在前）。没有大标题、没有总览页——侧栏本身就是全部导航。

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

        // ── 项目列表：单列，最近更新的会话在前（2026-09-06 用户拍板）──
        //   状态用字说话，不用色点：执行中 / 已激活 / 未激活。问题本身不在侧栏画：
        //   进消息流，表单原生呈现、原地作答。exited 会话不代表项目（标未激活，
        //   点一下 resume）；终端（shell）不在这里（归终端面板）。
        let rows = project_rows(&self.projects, &self.sessions);
        let mut list_col = div().flex().flex_col().gap(px(1.));
        for row in rows {
            let active = row
                .session
                .as_ref()
                .is_some_and(|s| self.page == Page::Session(s.id.clone()));
            let status = row.status;
            let open_session = row.session.as_ref().map(|s| s.id.clone());
            let open_project = row.project.clone();
            let kill = row.session.as_ref().map(|s| (s.id.clone(), kill_needs_confirm(s)));
            let del_path = row.project.as_ref().map(|p| p.path.clone());
            let pin = row.project.as_ref().map(|p| (p.path.clone(), p.pinned));
            let title = if row.pinned { format!("📌 {}", row.title) } else { row.title };
            // 元素 id 用路径而不是序号：排序变了悬停 / 点击态跟着行走，不留在原位
            let row_id = SharedString::from(format!("sb-proj:{}", row.path));
            let act_id = SharedString::from(format!("sb-act:{}", row.path));
            let mut el = row_base(row_id.into())
                .group("sb-row")
                .when(active, |el| el.bg(c(theme::surface_raised())))
                .hover(|st| st.bg(c(theme::surface_raised())))
                // 点一下：活着的会话直接进；未激活的 resume（daemon 幂等，找不到旧对话开新的）
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(id) = &open_session {
                        this.open_session(id.clone(), cx);
                    } else if let Some(p) = &open_project {
                        this.open_project(p, cx);
                    }
                }))
                // 状态字做成带边框的小标签：只靠字色分不开「已激活 / 未激活」，边框把它
                // 从标题里框出来，颜色（绿 / 黄 / 强调色 / 淡灰）再把四态拉开
                .child(
                    div()
                        .flex_none()
                        .px(px(4.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(c(status.color()))
                        .text_size(px(9.5))
                        .font_family("Menlo")
                        .text_color(c(status.color()))
                        .when(status == RowStatus::Inactive, |el| el.opacity(0.8))
                        .child(status.label()),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .text_color(c(if status == RowStatus::Inactive {
                            theme::dim()
                        } else {
                            theme::ink()
                        }))
                        .child(SharedString::from(title)),
                );
            // 行尾按钮（非当前行悬停才现身；invisible 连命中盒一起去掉）：
            // 「顶 / 取消」= 置顶开关（daemon 侧存，三端一起变）；
            // 活着的 × = 结束会话（只有执行中的才确认，被顺手点掉最伤）；未激活的「删」= 删项目
            let hover_btn = |id: SharedString| {
                div()
                    .id(id)
                    .flex_none()
                    .px(px(3.))
                    .rounded(px(4.))
                    .text_size(px(10.))
                    .text_color(c(theme::faint()))
                    .when(!active, |el| el.invisible().group_hover("sb-row", |st| st.visible()))
            };
            if let Some((pin_path, pinned)) = pin {
                el = el.child(
                    hover_btn(SharedString::from(format!("sb-pin:{}", row.path)))
                        .hover(|st| st.text_color(c(theme::accent())).bg(c(theme::edge_light())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.set_pinned(pin_path.clone(), !pinned, cx);
                        }))
                        .child(if pinned { "取消顶" } else { "顶" }),
                );
            }
            let button = div()
                .id(act_id)
                .flex_none()
                .px(px(3.))
                .rounded(px(4.))
                .text_size(px(10.))
                .text_color(c(theme::faint()))
                .hover(|st| st.text_color(c(theme::red())).bg(c(theme::edge_light())))
                .when(!active, |el| el.invisible().group_hover("sb-row", |st| st.visible()));
            if let Some((id_close, confirm)) = kill {
                el = el.child(
                    button
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.request_kill(id_close.clone(), confirm, cx);
                        }))
                        .child("✕"),
                );
            } else if let Some(del_path) = del_path {
                el = el.child(
                    button
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
                );
            }
            list_col = list_col.child(el);
        }

        let (conn_color, conn_text) = match self.conn {
            ConnState::Connected => (
                theme::green(),
                format!(
                    "daemon v{}",
                    self.health
                        .as_ref()
                        .map(|h| h.version.as_str())
                        .unwrap_or("?")
                ),
            ),
            ConnState::Connecting => (theme::amber(), "连接中…".to_string()),
            ConnState::Disconnected => (theme::red(), "未连接".to_string()),
        };

        div()
            .w(px(self.sidebar_w))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden() // 拖窄时标题按 ellipsis 收，不许挤出侧栏
            .bg(c(theme::surface()))
            // 新建项目就是一个输入框：字即文件夹名，回车或 ＋ 创建；⌘N 把光标放进来
            .child(
                div()
                    .mt(px(10.))
                    .mb(px(4.))
                    .mx(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(div().flex_1().min_w(px(0.)).child(self.new_input.clone()))
                    .child(
                        div()
                            .id("sb-new")
                            .flex_none()
                            .h(px(28.))
                            .w(px(28.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(c(theme::edge_light()))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|st| st.bg(c(theme::surface_raised())).border_color(c(theme::accent())))
                            .on_click(cx.listener(|this, _, _, cx| this.create_project(cx)))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(c(if self.creating { theme::faint() } else { theme::accent() }))
                                    .child(if self.creating { "…" } else { "＋" }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("sb-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .pt(px(4.))
                    .child(list_col),
            )
            // 终端面板入口：常驻工具，坐在 daemon 状态行上方
            .child(self.render_terminal_entry(cx))
            // 会话日志入口
            .child(self.render_history_entry(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .px(px(16.))
                    .py(px(9.))
                    .border_t_1()
                    .border_color(c(theme::edge()))
                    .child(dot(conn_color))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .font_family("Menlo")
                            .text_color(c(theme::dim()))
                            .child(SharedString::from(conn_text)),
                    )
                    .child(
                        div()
                            .id("sb-settings")
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(if self.page == Page::Settings {
                                c(theme::accent())
                            } else {
                                c(theme::dim())
                            })
                            .hover(|st| st.text_color(c(theme::accent())))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.page = Page::Settings;
                                cx.notify();
                            }))
                            .child("⚙"),
                    ),
            )
            // 套餐用量：最底下两行小字（没有套餐信息就整块不画）
            .when_some(self.render_plan_usage(), |el, block| el.child(block))
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
            .bg(c(if dragging { theme::accent() } else { theme::edge() }))
            .hover(|st| st.bg(c(theme::accent())))
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
            self.ui_state().save();
            cx.notify();
        }
    }

    /// 本机偏好的当前快照（落盘用）
    fn ui_state(&self) -> UiState {
        UiState {
            sidebar_w: self.sidebar_w,
            theme: self.theme.as_str().to_string(),
            detail_visible: self.detail_visible,
            muted_projects: self.muted_projects.clone(),
        }
    }

    /// 切主题：进程级调色板换掉、落盘、整窗重画。终端 / 消息流 / 输入框是独立
    /// 实体，一并 notify，保证同一帧换色而不是谁先动谁先变。
    pub(super) fn set_theme(&mut self, kind: ThemeKind, cx: &mut Context<Self>) {
        if self.theme == kind {
            return;
        }
        theme::set_current(kind);
        self.theme = kind;
        self.ui_state().save();
        for t in self.terminals.values() {
            t.update(cx, |_, cx| cx.notify());
        }
        for v in self.msg_views.values() {
            v.update(cx, |_, cx| cx.notify());
        }
        for i in [
            &self.new_input,
            &self.name_input,
            &self.host_input,
            &self.port_input,
            &self.token_input,
            &self.root_input,
            &self.inbox_input,
        ] {
            i.update(cx, |_, cx| cx.notify());
        }
        cx.notify();
    }

    fn render_statusbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let bar = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .h(px(26.))
            .flex_none()
            .px(px(12.))
            .bg(c(theme::surface()))
            .border_t_1()
            .border_color(c(theme::edge()))
            .text_size(px(11.))
            .font_family("Menlo")
            .text_color(c(theme::dim()));
        match &self.page {
            Page::Session(id) => {
                if let Some(s) = self.session(id) {
                    // 只有 Claude 一种 agent，不再报 agent 名；有 resume id 才多一格
                    let resume_part = s
                        .resume_id
                        .as_deref()
                        .filter(|r| !r.is_empty())
                        .map(|r| format!("resume {}", &r[..r.len().min(6)]));
                    let state_color = theme::state_color(s.state.as_str());
                    let sid = s.id.clone();
                    let exited = s.state == SessionState::Exited;
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
                        .when_some(resume_part, |bar, t| bar.child(SharedString::from(t)));

                    // 消息流 ⇄ 终端 切换（shell 无消息流；探明不支持后隐藏）
                    let msg_supported = self
                        .msg_views
                        .get(&sid)
                        .map(|v| v.read(cx).supported)
                        .unwrap_or(None);
                    if s.agent != "shell" && msg_supported != Some(false) {
                        let on = self.msg_mode_active(&sid, cx);
                        bar = bar.child(
                            div()
                                .id("view-toggle")
                                .px(px(6.))
                                .rounded(px(4.))
                                .cursor_pointer()
                                .text_color(c(if on { theme::accent() } else { theme::faint() }))
                                .hover(|st| st.bg(c(theme::surface_raised())))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_msg_mode(cx);
                                }))
                                .child(if on { "⌘E 终端" } else { "⌘E 消息流" }),
                        );
                    }

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
                                    .text_color(c(theme::accent()))
                                    .hover(|st| st.bg(c(theme::surface_raised())))
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
                            .text_color(c(theme::faint()))
                            .hover(|st| st.text_color(c(theme::accent())))
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
                            .hover(|st| st.bg(c(theme::surface_raised())))
                            .child(label)
                    };
                    let sid_rename = sid.clone();
                    let sid_kill = sid.clone();
                    let sid_del = sid.clone();
                    // ⓘ 详情面板开关（⌘I）
                    let detail_on = self.detail_visible;
                    bar = bar
                        .child(div().ml_auto())
                        .child(
                            act(
                                "detail-toggle",
                                "ⓘ 详情",
                                if detail_on { theme::accent() } else { theme::faint() },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_detail(cx))),
                        )
                        .child(act("sess-rename", "重命名", theme::dim()).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_rename_modal(sid_rename.clone(), window, cx);
                            },
                        )));
                    if !exited {
                        let confirm = kill_needs_confirm(s);
                        bar = bar.child(act("sess-kill", "终止", theme::amber()).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.request_kill(sid_kill.clone(), confirm, cx);
                            }),
                        ));
                    }
                    bar = bar.child(act("sess-del", "删除", theme::red()).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.modal = Modal::ConfirmDeleteSession {
                                id: sid_del.clone(),
                            };
                            cx.notify();
                        },
                    )));

                    // 「待回复」盖过状态字：asking 是结构化事实，比 running/waiting 更要紧
                    let (label, color) = if s.asking {
                        ("待回复".to_string(), theme::amber())
                    } else if s.compacting {
                        ("整理上下文中".to_string(), state_color)
                    } else if let Some(e) = s.error_label() {
                        (e, theme::amber())
                    } else {
                        (theme::state_label(s.state.as_str()).to_string(), state_color)
                    };
                    bar.child(
                        div()
                            .text_color(c(color))
                            .child(SharedString::from(format!("● {label}"))),
                    )
                } else {
                    bar.child("会话不存在")
                }
            }
            Page::Terminal => self.render_terminal_statusbar(bar),
            Page::Home | Page::Settings | Page::History => {
                // 终端不是会话，不进这里的计数
                let total = self.sessions.iter().filter(|s| !s.is_terminal()).count();
                let asking = self
                    .sessions
                    .iter()
                    .filter(|s| !s.is_terminal() && s.asking)
                    .count();
                bar.child(SharedString::from(format!(
                    "{total} 个会话 · {asking} 个待回复"
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
                .bg(ca(theme::red(), 0.14))
                .border_b_1()
                .border_color(c(theme::red()))
                .text_size(px(12.))
                .text_color(c(theme::red()))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.error = None;
                    cx.notify();
                }))
                .child(SharedString::from(err))
                .child(div().ml_auto().text_color(c(theme::dim())).child("点击关闭")),
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
                    let msg_view = self
                        .msg_mode_active(&id, cx)
                        .then(|| self.msg_views.get(&id).cloned())
                        .flatten();
                    let term = self.terminals.get(&id).cloned();
                    match (msg_view, term) {
                        (Some(mv), _) => el.child(div().flex_1().min_h(px(0.)).child(mv)),
                        (None, Some(t)) => el.child(div().flex_1().min_h(px(0.)).child(t)),
                        (None, None) => el.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c(theme::faint()))
                                .child("会话未打开"),
                        ),
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
                        .text_color(c(theme::faint()))
                        .child(div().text_size(px(13.)).child("双击左侧项目开启会话"))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_family("Menlo")
                                .child("⌘N 新建项目 · ⌃Tab 切换会话 · ⌘W 关闭会话"),
                        ),
                ),
                Page::Settings => el.child(self.render_settings(window, cx)),
                Page::Terminal => el.child(self.render_terminal_page(cx)),
                Page::History => el.child(self.render_history_page(cx)),
            }
        });

        let mut main = div().flex_1().min_w(px(0.)).flex().flex_col().bg(c(theme::bg()));
        if let Some(toast) = self.render_error_toast(cx) {
            main = main.child(toast);
        }
        if !self.ssd_mounted {
            main = main.child(
                div()
                    .flex_none()
                    .px(px(12.))
                    .py(px(5.))
                    .bg(ca(theme::amber(), 0.1))
                    .border_b_1()
                    .border_color(ca(theme::amber(), 0.4))
                    .text_size(px(11.5))
                    .text_color(c(theme::amber()))
                    .child("SSD 未挂载：创建 / 启动 / 删除已禁用（绝不建占位目录），挂载恢复后自动解除"),
            );
        }
        main = main.child(content).child(self.render_statusbar(cx));

        let mut root = div()
            .size_full()
            .flex()
            .bg(c(theme::bg()))
            .text_color(c(theme::ink()))
            .text_size(px(13.))
            .on_key_down(cx.listener(Self::on_root_key))
            .child(self.render_sidebar(cx))
            .child(self.render_sidebar_resizer(cx))
            .child(main)
            // 会话页右侧的详情面板（⌘I；只在会话页且展开时存在）
            .when_some(self.render_detail_panel(cx), |el, panel| el.child(panel));

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

    fn sess(id: &str, state: SessionState, asking: bool, last: &str) -> Session {
        Session {
            id: id.into(),
            state,
            asking,
            last_output_at: last.into(),
            ..Default::default()
        }
    }

    #[test]
    fn active_means_alive_project_session() {
        // 上栏只看「活着的项目会话」：状态/在问与否都不分组
        assert!(is_active(&sess("a", SessionState::Running, true, "")));
        assert!(is_active(&sess("a", SessionState::Waiting, true, "")));
        assert!(is_active(&sess("a", SessionState::Running, false, "")));
        assert!(is_active(&sess("a", SessionState::Waiting, false, "")));
        // exited 不进侧栏：项目回下栏，双击 resume
        assert!(!is_active(&sess("a", SessionState::Exited, false, "")));
        // 终端（shell）哪怕 running / asking 也不在上栏
        let mut term = sess("t", SessionState::Running, true, "");
        term.agent = "shell".into();
        assert!(!is_active(&term));
    }

    #[test]
    fn only_executing_sessions_ask_before_kill() {
        assert!(kill_needs_confirm(&sess("a", SessionState::Running, false, "")));
        // 在问 = 等你，哪怕屏幕还在变也不打断
        assert!(!kill_needs_confirm(&sess("a", SessionState::Running, true, "")));
        assert!(!kill_needs_confirm(&sess("a", SessionState::Waiting, false, "")));
        assert!(!kill_needs_confirm(&sess("a", SessionState::Exited, false, "")));
    }

    #[test]
    fn order_follows_created_at_not_state() {
        let mk = |id: &str, state, asking, created: &str| {
            let mut s = sess(id, state, asking, "2026-09-02T12:00:00Z");
            s.created_at = created.into();
            s
        };
        let mut v = [
            mk("wait", SessionState::Waiting, false, "2026-09-02T10:00:00Z"),
            mk("ask", SessionState::Waiting, true, "2026-09-02T09:00:00Z"),
            mk("run", SessionState::Running, false, "2026-09-02T11:00:00Z"),
            mk("exited", SessionState::Exited, false, "2026-09-02T08:00:00Z"),
        ];
        v.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
        let ids: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
        // 先开的在前，状态变了顺序不动
        assert_eq!(ids, ["exited", "ask", "wait", "run"]);
        // 同刻按 id 稳住
        let mut tie = [
            mk("b", SessionState::Running, false, "2026-09-02T10:00:00Z"),
            mk("a", SessionState::Waiting, false, "2026-09-02T10:00:00Z"),
        ];
        tie.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
        assert_eq!(tie[0].id, "a");
    }

    fn tsess(id: &str, agent: &str, state: SessionState, path: &str, created: &str) -> Session {
        Session {
            id: id.into(),
            agent: agent.into(),
            state,
            project_path: path.into(),
            created_at: created.into(),
            ..Default::default()
        }
    }

    #[test]
    fn terminal_tabs_ordered_by_created_and_labelled() {
        use SessionState::*;
        let root = "/Volumes/SSD/project";
        let sessions = vec![
            // 故意乱序放：标签顺序只看 created_at
            tsess("t2", "shell", Running, "/Volumes/SSD/project/aaa-ui", "2026-09-02T10:02:00Z"),
            tsess("c1", "claude", Running, root, "2026-09-02T10:00:00Z"), // 不是终端
            tsess("t1", "shell", Waiting, "/Volumes/SSD/project/", "2026-09-02T10:01:00Z"), // 根（带尾斜杠）
            tsess("t3", "shell", Exited, root, "2026-09-02T10:03:00Z"), // 已退出不算
            tsess("t4", "shell", Waiting, "/tmp/x/", "2026-09-02T10:04:00Z"),
        ];
        let tabs = terminal_tabs(&sessions, root);
        let expect: Vec<(String, String)> = [
            ("t1", "终端 1"),
            ("t2", "终端 2 · aaa-ui"),
            ("t4", "终端 3 · x"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        // 编号是存活标签里的序号：t3 退出后 t4 是「终端 3」而不是 4
        assert_eq!(tabs, expect);
        assert!(terminal_tabs(&Vec::<Session>::new(), root).is_empty());
        // project_path 为空（daemon 字段不全）按根处理，不追加目录名
        let bare = vec![tsess("t9", "shell", Running, "", "2026-09-02T10:00:00Z")];
        assert_eq!(terminal_tabs(&bare, root)[0].1, "终端 1");
        // 同一时刻按 id 稳住顺序
        let tie = vec![
            tsess("t_b", "shell", Running, root, "2026-09-02T10:00:00Z"),
            tsess("t_a", "shell", Running, root, "2026-09-02T10:00:00Z"),
        ];
        let tie_ids: Vec<String> = terminal_tabs(&tie, root).into_iter().map(|(i, _)| i).collect();
        assert_eq!(tie_ids, ids(&["t_a", "t_b"]));
    }

    fn proj(path: &str, name: &str, mtime: &str, title: Option<&str>) -> Project {
        Project {
            path: path.into(),
            name: name.into(),
            mtime: mtime.into(),
            session_title: title.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn rows_are_one_per_project_ordered_by_latest_update() {
        use SessionState::*;
        let projects = vec![
            proj("/p/a", "a", "2026-09-01T00:00:00Z", Some("旧对话")),
            proj("/p/b", "b", "2026-09-05T00:00:00Z", None),
            proj("/p/c", "c", "2026-09-02T00:00:00Z", None),
        ];
        let mut run = tsess("a1", "claude", Running, "/p/a", "2026-09-03T00:00:00Z");
        run.updated_at = "2026-09-04T00:00:00Z".into();
        run.title = "改登录页".into();
        // 早开、后来又退出的会话只贡献排序时间，不代表项目
        let mut old = tsess("a0", "claude", Exited, "/p/a", "2026-09-02T00:00:00Z");
        old.updated_at = "2026-09-06T00:00:00Z".into();
        let mut ask = tsess("c1", "claude", Waiting, "/p/c", "2026-09-03T12:00:00Z");
        ask.updated_at = "2026-09-03T12:00:00Z".into();
        ask.asking = true;
        let shell = tsess("t1", "shell", Running, "/p/b", "2026-09-06T12:00:00Z");
        let rows = project_rows(&projects, &[run.clone(), old, ask, shell]);
        let got: Vec<(&str, RowStatus, &str)> =
            rows.iter().map(|r| (r.path.as_str(), r.status, r.title.as_str())).collect();
        assert_eq!(
            got,
            vec![
                // a：最新一条会话（退出的那条）06 更新，排第一；活着的会话代表它
                ("/p/a", RowStatus::Running, "改登录页"),
                // b：只有终端——终端不算，按目录 mtime 05
                ("/p/b", RowStatus::Inactive, "b"),
                // c：在问 = 待回复（不选就卡住），03
                ("/p/c", RowStatus::Asking, "c"),
            ]
        );
        assert_eq!(rows[0].session.as_ref().map(|s| s.id.as_str()), Some("a1"));
        assert!(rows[1].session.is_none(), "终端不代表项目");
        // ⌃Tab 只在存活的项目会话里转，顺序跟侧栏
        assert_eq!(cyclable_ids(&projects, &[run, ]), ids(&["a1"]));
    }

    #[test]
    fn rows_fall_back_when_daemon_has_no_updated_at() {
        use SessionState::*;
        let projects = vec![proj("/p/a", "a", "2026-09-01T00:00:00Z", None), proj("/p/b", "b", "2026-09-01T00:00:00Z", None)];
        // 老 daemon：updated_at 为空 → created_at；b 后开 → 在前
        let a = tsess("a1", "claude", Waiting, "/p/a", "2026-09-02T00:00:00Z");
        let b = tsess("b1", "claude", Waiting, "/p/b", "2026-09-03T00:00:00Z");
        let rows = project_rows(&projects, &[a, b]);
        assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/p/b", "/p/a"]);
        assert!(rows.iter().all(|r| r.status == RowStatus::Active));
        // 没登记的目录里有活会话：也给一行，但没有 project（不能删 / resume）
        let stray = tsess("s1", "claude", Running, "/elsewhere/x", "2026-09-09T00:00:00Z");
        let rows = project_rows(&projects, &[stray]);
        assert_eq!(rows[0].path, "/elsewhere/x");
        assert!(rows[0].project.is_none());
        assert_eq!(rows[0].status, RowStatus::Running);
        // 未激活的标题：daemon 读出的对话名，没有才是文件夹名
        let rows = project_rows(&[proj("/p/z", "z", "", Some("上次聊的"))], &[]);
        assert_eq!(rows[0].title, "上次聊的");
        assert_eq!(rows[0].status, RowStatus::Inactive);
        // 置顶的排最前，哪怕它最久没动
        let mut old = proj("/p/old", "old", "2026-01-01T00:00:00Z", None);
        old.pinned = true;
        let fresh = proj("/p/new", "new", "2026-09-01T00:00:00Z", None);
        let rows = project_rows(&[fresh, old], &[]);
        assert_eq!(rows.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["/p/old", "/p/new"]);
        assert!(rows[0].pinned && !rows[1].pinned);
    }

    #[test]
    fn status_words() {
        use SessionState::*;
        assert_eq!(RowStatus::of(Some(&sess("a", Running, false, ""))), RowStatus::Running);
        // 在问：哪怕屏幕还在变也是「待回复」
        assert_eq!(RowStatus::of(Some(&sess("a", Running, true, ""))), RowStatus::Asking);
        assert_eq!(RowStatus::of(Some(&sess("a", Waiting, true, ""))), RowStatus::Asking);
        assert_eq!(RowStatus::of(Some(&sess("a", Waiting, false, ""))), RowStatus::Active);
        assert_eq!(RowStatus::of(Some(&sess("a", Exited, false, ""))), RowStatus::Inactive);
        assert_eq!(RowStatus::of(None), RowStatus::Inactive);
        assert_eq!(RowStatus::Running.label(), "执行中");
        assert_eq!(RowStatus::Asking.label(), "待回复");
        assert_eq!(RowStatus::Active.label(), "已激活");
        assert_eq!(RowStatus::Inactive.label(), "未激活");
    }

    #[test]
    fn closing_terminal_tab_picks_neighbour() {
        let tabs = ids(&["a", "b", "c"]);
        // 优先右邻
        assert_eq!(next_terminal_after_close(&tabs, "b").as_deref(), Some("c"));
        assert_eq!(next_terminal_after_close(&tabs, "a").as_deref(), Some("b"));
        // 最右边的关掉 → 左邻
        assert_eq!(next_terminal_after_close(&tabs, "c").as_deref(), Some("b"));
        // 只剩一个 → 没了
        assert_eq!(next_terminal_after_close(&ids(&["a"]), "a"), None);
        // 关的不在列表里（别处已删）→ 最新的
        assert_eq!(next_terminal_after_close(&tabs, "zz").as_deref(), Some("c"));
        assert_eq!(next_terminal_after_close(&[], "a"), None);
    }

    #[test]
    fn active_terminal_resolution() {
        let tabs = ids(&["a", "b", "c"]);
        // 当前还活着就不动
        assert_eq!(resolve_active_terminal(Some("b"), &tabs).as_deref(), Some("b"));
        // 指向已死 / 没有当前 → 最新（末尾）
        assert_eq!(resolve_active_terminal(Some("gone"), &tabs).as_deref(), Some("c"));
        assert_eq!(resolve_active_terminal(None, &tabs).as_deref(), Some("c"));
        assert_eq!(resolve_active_terminal(Some("a"), &[]), None);
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
