//! UI 根视图：侧栏（唯一的会话切换入口）+ 页面区 + 状态栏 + 模态框。

mod detail_panel;
mod files_view;
mod history;
mod kit;
mod messages_view;
mod mini_input;
mod modals;
mod scrollbar;
mod settings;
mod stream_fold;
mod terminal_panel;
mod terminal_view;

use std::collections::{HashMap, HashSet};

use futures::StreamExt;
use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, SharedString, Task, Window, div,
    prelude::*, px,
};

use crate::model::*;
use crate::net::{ConnState, Net, UiEvent};
use crate::theme;
use kit::*;
use files_view::FilesView;
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

/// 一条会话的三种看法（v1.30 加了「浏览」）。终端是**兜底那一个**：消息流可能
/// 不支持（shell / 老 daemon），浏览要项目目录，终端永远画得出来。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SessionView {
    #[default]
    Terminal,
    Messages,
    Files,
}

impl SessionView {
    pub fn label(self) -> &'static str {
        match self {
            SessionView::Terminal => "终端",
            SessionView::Messages => "消息流",
            SessionView::Files => "浏览",
        }
    }
}

/// ⌘E 的轮换顺序：终端 → 消息流 → 浏览 → 终端。`msgs` 为 false（shell、
/// 老 daemon、探明不支持）时跳过消息流那一档——切到一个画不出来的视图，
/// 用户按下去只会看见终端，还以为快捷键坏了。
pub fn next_view(cur: SessionView, msgs: bool) -> SessionView {
    let order = [SessionView::Terminal, SessionView::Messages, SessionView::Files];
    let i = order.iter().position(|v| *v == cur).unwrap_or(0);
    for step in 1..=order.len() {
        let cand = order[(i + step) % order.len()];
        if cand != SessionView::Messages || msgs {
            return cand;
        }
    }
    SessionView::Terminal
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

/// 会话进程还在（running / waiting，含 asking）。**exited 的不算**——进程没了它就只是
/// 历史，项目行画成灰的、点一下即 resume。终端（shell）不是项目会话，归终端面板管
/// （PROTOCOL「终端」）。
fn is_active(s: &Session) -> bool {
    !s.is_terminal() && s.state != SessionState::Exited
}

/// 关闭前要不要确认：只有还在执行（running 且不在问）的会话被顺手点掉最伤。
/// waiting / asking 都是「等你」，exited 没进程可杀——这些关掉没损失，不打断。
fn kill_needs_confirm(s: &Session) -> bool {
    s.state == SessionState::Running && !s.asking
}

/// 侧栏一行：一个项目（2026-09-06 起单列，不再分「激活 / 未激活」两栏）。
/// **2026-09-08 用户拍板：侧栏不再写状态字**；**2026-09-10 再拍板：也不画竖线，状态用整行
/// 的淡底色说**——**淡蓝 = 在跑**，**淡黄 = 跑完了 / 在等你回话且这台机器还没进去看**，
/// **无底色 = 已读**；当前打开的那一行标题带强调色下划线（见 [`row_bg`]）。
#[derive(Debug, Clone)]
struct ProjectRow {
    path: String,
    title: String,
    /// daemon 给的五态字符串（`model::status_running`）。v1.22 之前这里是
    /// 个本地枚举，由会话的 state/asking/background 现推——三端各推一套，同一个项目在
    /// mac 和手机上能显示成两种状态，所以整条阶梯删掉了。
    status: String,
    /// 代表这个项目的会话（按 `Project::session_id` 查得）。**可能是已退出的那一条**：
    /// 没有活会话时 daemon 给的是最近退出的一个，点它仍是 resume，见 [`ProjectRow::live`]
    session: Option<Session>,
    /// 注册表里的项目；只有会话、没登记的目录为 None（只能看，不能 resume / 删）
    project: Option<Project>,
    /// 排序键：daemon 的 `updated_at`（没给就退到目录 mtime）。ISO 时间串，字典序即时间序
    sort_key: String,
    /// 黄点：跑完一轮 / 在等你回话，而这台机器还没进去看过（本机状态，UiState::unread_projects）
    unread: bool,
}

impl ProjectRow {
    /// 还活着的那个会话：点行直接打开、行尾给 ✕。`None` = 点行 resume、行尾给「删」。
    /// `session` 本身可能是已退出的会话（daemon 拿它当代表），死没死是会话自己的
    /// `state` 说了算——这一条不归五态管，也不随 v1.22 变。
    fn live(&self) -> Option<&Session> {
        self.session.as_ref().filter(|s| is_active(s))
    }
}

/// 项目 → 侧栏行。**v1.22：一行的内容全部现成**——标题、五态、代表会话、排序时间都由
/// daemon 算好放在 `Project` 上（PROTOCOL「版本兼容」），这里只做两件纯本机的事：
/// 把 `session_id` 换成手里的 `Session` 对象，和按 `updated_at` 从新到旧排。
///
/// 删掉的旧做法（别再加回来）：① 遍历 `sessions` 按 project_path 挑「最近更新的活会话」
/// 当代表、顺带算最新时间——Android 挑法不同，两端标题和状态对不上；② 标题回退链
/// （活会话标题 → session_title → 目录名）自己走一遍；③ 为「有活会话但没登记」的目录
/// 补一行——daemon 现在自己补（`registered:false`），客户端再补就是两行。
fn project_rows(projects: &[Project], sessions: &[Session], unread: &[String]) -> Vec<ProjectRow> {
    let mut rows: Vec<ProjectRow> = projects
        .iter()
        .map(|p| ProjectRow {
            path: p.path.clone(),
            title: p
                .title
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| p.name.clone()),
            status: p.status.clone(),
            session: p
                .session_id
                .as_deref()
                .and_then(|id| sessions.iter().find(|s| s.id == id))
                .cloned(),
            // 没登记的目录（别处 `aaa open` 开的）不给 project：resume / 删项目都得有注册表
            project: p.registered.then(|| p.clone()),
            sort_key: p
                .updated_at
                .clone()
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| p.mtime.clone()),
            unread: path_list_contains(unread, &p.path),
        })
        .collect();
    // 2026-09-10 用户拍板：就按行尾那个「xxx 分钟前」从新到旧排，不分档。
    // 此前是「黄底 > 状态 > 时间」——状态已经由整行底色说了，再拿它排一遍是同一件事
    // 说两遍，而且行会因为状态翻转在列表里跳位置。同刻按标题稳住。
    // 同刻按**路径**稳住，不按标题：标题会被改名和 AI 命名改写，一改行就跳位置。
    // 两端认同一个并列键，共享向量里有一对同刻的项目盯着这条（见 fixtures/projects.json）。
    rows.sort_by(|a, b| b.sort_key.cmp(&a.sort_key).then_with(|| a.path.cmp(&b.path)));
    rows
}

/// 行尾那个时间（2026-09-08 用户：「MacOS 这边也显示出来时间」——Android 项目列表
/// 一直有，mac 侧栏没有）。口径与 Android 的 `relativeTime` 一字不差：一分钟内「刚刚」，
/// 一小时内「N 分钟前」，一天内「N 小时前」，再远就是本地时区的 `MM-DD HH:mm`。
fn relative_time(iso: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso.trim()) else {
        return String::new();
    };
    let secs = (now.timestamp() - t.timestamp()).max(0);
    match secs {
        s if s < 60 => "刚刚".to_string(),
        s if s < 3600 => format!("{} 分钟前", s / 60),
        s if s < 86_400 => format!("{} 小时前", s / 3600),
        _ => t.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string(),
    }
}

/// 这一版客户端要求的 `/health` schema（PROTOCOL「版本兼容」，当前 = 2）：低于它的
/// daemon 不下发五态 / 标题 / 代表会话，项目列表顶上挂降级横幅。
const SCHEMA_MIN: u32 = 2;

/// 这一行的状态底色（2026-09-10 用户拍板：「去掉竖线状态的设计，改为背景色，用浅色」）：
/// **淡黄** = 跑完了 / 在等你回话而这台机器还没进去看；**淡蓝** = 在跑（含后台任务还没回来）；
/// `None` = 已读，没什么要你操心的，就是侧栏自己的底。黄盖过蓝——黄的那个在等你。
/// 底色只管底色：2026-09-10 起排序不看状态，只看时间。
/// 行高不随状态跳，也没有任何按帧重画的动画。
fn row_bg(status: &str, unread: bool) -> Option<u32> {
    if unread {
        Some(theme::ROW_UNREAD)
    } else if status_running(status) {
        Some(theme::ROW_RUNNING)
    } else {
        None
    }
}

/// ⌃Tab 循环的候选：存活的项目会话，按侧栏顺序。
fn cyclable_ids(projects: &[Project], sessions: &[Session]) -> Vec<String> {
    project_rows(projects, sessions, &[])
        .into_iter()
        .filter_map(|r| r.live().map(|s| s.id.clone()))
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
    /// agent 表（GET /agents）。老 daemon 没有这个路由 → 空表 → 一个切换入口都不画
    pub agents: Vec<AgentInfo>,
    /// 下一个新建项目用哪个 agent（表里第一个装了的）
    pub new_agent: String,
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

    // 会话的三种看法（⌘E 轮换，状态栏也能直接点）：视图按需创建，
    // 没记过的会话默认终端——它永远画得出来
    msg_views: HashMap<String, Entity<MessagesView>>,
    files_views: HashMap<String, Entity<FilesView>>,
    view_mode: HashMap<String, SessionView>,

    /// 详情栏里点开了的那几行（`sub:<i>` / `bg:<i>`）：**只预览**，没有干预的口子
    detail_open: HashSet<String>,

    /// 本机手动终止的会话：exited 不弹「已退出」（自己动的手）
    pub user_killed: HashSet<String>,

    // 侧栏宽度（拖右边缘调整，松手落盘）与拖动中的 (按下时鼠标 x, 按下时宽度)
    pub sidebar_w: f32,
    sidebar_drag: Option<(f32, f32)>,

    // 详情面板（会话页右侧，⌘I）
    /// 面板展开与否，随 ui.toml 落盘
    pub detail_visible: bool,
    /// 系统通知总开关，随 ui.toml 落盘（设置页；没有分项目静音了）
    pub notify_on: bool,
    /// 有黄点的项目路径（本机，见 UiState::unread_projects）
    pub unread_projects: Vec<String>,
    /// 套餐用量（GET /usage + usage 帧）；None = 没有套餐信息，侧栏不画
    pub plan: Option<PlanUsage>,
    /// 看板（GET /history/dashboard），打开「看板」页时拉
    pub dashboard: Dashboard,
    /// 看板搜索框
    pub history_input: Entity<MiniInput>,
    /// 看板：显示已删除的
    pub dash_show_deleted: bool,
    /// 看板：「不在 AAA 里」那一节展开与否（默认折起来，2026-09-08 用户「东西太多了」）
    pub dash_show_gone: bool,
    /// 看板的滚动条（瀑布流一屏装不下，滚起来得知道自己在哪儿）
    dash_scroll: scrollbar::Scrollbar,
    /// 窗口宽度（render 开头刷新；看板按它算瀑布流列数）
    pub win_w: f32,
    /// 每会话的产物 / 改动状态（含各自的拉取节流器）
    detail: HashMap<String, detail_panel::SessionDetail>,
    /// 项目路径 → 排着的任务（GET /inbox）
    pub inbox: HashMap<String, Vec<InboxEntry>>,
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
        // 一分钟一次的空转重画：侧栏行尾写的是「N 分钟前」，而这个 App 只在 daemon
        // 有事推过来时重画——整套系统都闲着的时候，那行字会一直停在「刚刚」。
        // 一分钟一帧的代价可以忽略，时间说的是真话更要紧。
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        // 系统通知点击 → 回到 App、打开那条会话（notify.rs 把会话 id 丢进这条通道）
        let mut clicks = crate::notify::install();
        cx.spawn(async move |this, cx| {
            while let Some(action) = clicks.next().await {
                if this
                    .update(cx, |root: &mut RootView, cx| match action {
                        // 横幅上按的「允许 / 拒绝」：不抢焦点，直接替答
                        crate::notify::NotifyAction::Decide(id, behavior) => {
                            root.spawn_fetch_ignore(root.net.session_permission(&id, behavior), false, cx);
                        }
                        crate::notify::NotifyAction::Open(id) => {
                            cx.activate(true);
                            if root.session(&id).is_some() {
                                root.open_session(id, cx);
                            }
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let ui_state = UiState::load();

        let new_input = cx.new(|cx| MiniInput::new(cx, "新建项目：文件夹名，回车"));
        let name_input = cx.new(|cx| MiniInput::new(cx, "新名字"));
        let host_input = cx.new(|cx| MiniInput::new(cx, "127.0.0.1"));
        let port_input = cx.new(|cx| MiniInput::new(cx, "2730"));
        let token_input = cx.new(|cx| MiniInput::new(cx, "aaa_tk_…"));
        let root_input = cx.new(|cx| MiniInput::new(cx, "~/project"));
        let history_input = cx.new(|cx| MiniInput::new(cx, "搜索：标题 / 项目 / 条目"));
        let inbox_input = cx.new(|cx| MiniInput::new(cx, "排一句话，空下来自动发"));
        cx.subscribe(&inbox_input, |this, _, _: &mini_input::InputEvent, cx| {
            this.submit_inbox(cx);
        })
        .detach();
        // 输入法送来的回车（见 MiniInput::replace_text_in_range）与键盘回车同一出口
        cx.subscribe(&new_input, |this, _, _: &mini_input::InputEvent, cx| {
            if matches!(this.modal, Modal::None) {
                this.create_project(cx);
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
            agents: Vec::new(),
            new_agent: "claude".into(),
            qr_modules: None,
            endpoint_from_config,
            terminals: HashMap::new(),
            open_order: Vec::new(),
            pending_focus: None,
            active_terminal: None,
            deleted_terminals: HashSet::new(),
            msg_views: HashMap::new(),
            files_views: HashMap::new(),
            view_mode: HashMap::new(),
            detail_open: HashSet::new(),
            user_killed: HashSet::new(),
            sidebar_w: ui_state.sidebar_w,
            sidebar_drag: None,
            detail_visible: ui_state.detail_visible,
            notify_on: ui_state.notify,
            unread_projects: ui_state.unread_projects,
            plan: None,
            dashboard: Dashboard::default(),
            history_input,
            inbox: HashMap::new(),
            inbox_input,
            dash_show_deleted: false,
            dash_show_gone: false,
            dash_scroll: scrollbar::Scrollbar::default(),
            win_w: 1200.,
            detail: HashMap::new(),
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
                self.files_views.remove(&id);
                self.view_mode.remove(&id);
                self.open_order.retain(|x| x != &id);
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
                self.refresh_detail(&id, cx);
            }
            DaemonEvent::Usage { plan } => {
                self.plan = plan;
                cx.notify();
            }
            // 队列被喂掉一条 / 别处加了一条：详情栏那一节跟着走
            DaemonEvent::InboxChanged { path } => {
                // 两边都去尾斜杠再比（与未读同一个口径）：daemon 发的是 realpath 过的
                if self.current_project_path().as_deref() == Some(path.trim_end_matches('/')) {
                    self.refresh_inbox(cx);
                }
            }
            DaemonEvent::Unknown => {}
        }
    }

    /// ⌘E：当前会话在 终端 → 消息流 → 浏览 之间轮换
    fn cycle_view(&mut self, cx: &mut Context<Self>) {
        let Page::Session(id) = self.page.clone() else {
            return;
        };
        let next = next_view(self.view_of(&id, cx), self.msgs_available(&id, cx));
        self.set_view(id, next, cx);
    }

    /// 切到某一种看法；视图按需创建，切走时把焦点还给终端
    fn set_view(&mut self, id: String, view: SessionView, cx: &mut Context<Self>) {
        match view {
            SessionView::Terminal => {
                self.pending_focus = Some(id.clone());
            }
            SessionView::Messages => {
                let net = self.net.clone();
                let sid = id.clone();
                // 新建的视图默认当会话活着；这里立刻用真实状态校准，已退出的会话
                // 里悬着的表单不能是可交互的
                let (alive, running, perm, asking_seq, queued) = self
                    .session(&id)
                    .map(|s| (s.state != SessionState::Exited, s.state == SessionState::Running, s.permission.clone(), s.asking_seq, s.queued.clone()))
                    .unwrap_or((false, false, None, None, Vec::new()));
                let project_path = self.session(&id).map(|s| s.project_path.clone()).unwrap_or_default();
                self.msg_views
                    .entry(id.clone())
                    .or_insert_with(|| cx.new(|cx| MessagesView::new(sid, net, cx)))
                    .update(cx, |v, cx| {
                        v.set_project_path(project_path);
                        v.set_session(alive, running, perm, asking_seq, queued.clone(), cx);
                        v.fetch(cx);
                        v.request_focus(cx);
                    });
            }
            SessionView::Files => {
                let net = self.net.clone();
                let dir = self.session(&id).map(|s| s.project_path.clone()).unwrap_or_default();
                match self.files_views.entry(id.clone()) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        // 会话换过项目（resume 到别处）时把浏览器指到新目录
                        e.get().update(cx, |v, cx| v.set_dir(dir, cx));
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(cx.new(|cx| FilesView::new(net, dir, cx)));
                    }
                }
            }
        }
        self.view_mode.insert(id, view);
        cx.notify();
    }

    /// 这条会话此刻在看哪一种。**记着的那一种可能已经画不出来了**（消息流探明
    /// 不支持、会话没有项目目录），那就当它在看终端——回落只判这一处。
    fn view_of(&self, id: &str, cx: &Context<Self>) -> SessionView {
        match self.view_mode.get(id).copied().unwrap_or_default() {
            SessionView::Messages if !self.msgs_available(id, cx) => SessionView::Terminal,
            v => v,
        }
    }

    /// 这条会话有没有消息流可看（shell / 老 daemon / 探明不支持 → 没有）
    fn msgs_available(&self, id: &str, cx: &Context<Self>) -> bool {
        self.session(id).is_some_and(|s| s.agent != "shell")
            && self
                .msg_views
                .get(id)
                .map(|v| v.read(cx).supported)
                .unwrap_or(None)
                != Some(false)
    }

    /// 系统通知只有三种（2026-09-07 用户拍板，PROTOCOL「WS」通知策略）：
    /// **待回复**（asking 翻 true：弹着选项 / 授权等你）、**运行结束**（running→waiting，
    /// 这轮干完了）、**出错**（StopFailure 报的错误，或非 0 退出）。正常退出、自己在
    /// app 里 kill 的、正盯着看的、静音的项目都不弹。终端不通知。
    fn maybe_notify(&mut self, new: &Session, cx: &mut Context<Self>) {
        if new.is_terminal() {
            return;
        }
        let old = self.sessions.iter().find(|s| s.id == new.id);
        let (old_state, old_asking, old_error) = old
            .map(|s| (Some(s.state), s.asking, s.error.clone()))
            .unwrap_or((None, false, None));
        // 用户正盯着这个会话（窗口前台 + 当前页就是它）就别弹通知——
        // 眼皮底下跑完的东西再弹一条只是噪音
        let watching = self.page == Page::Session(new.id.clone()) && cx.active_window().is_some();

        // 标记无论如何都要消耗掉
        let killed_here = self.user_killed.remove(&new.id);
        // 黄点（2026-09-08）：打点的时机和三种通知完全一样——响一声、列表上留一个点，是同一
        // 件事的两种说法。区别只有一个：**通知总开关只关通知，不关黄点**（关通知是「别吵我」，
        // 不是「别记着」）；正盯着这个会话看的时候不打点，那已经看见了。
        let worth_flagging = (new.asking && !old_asking && new.state != SessionState::Exited)
            || new
                .error
                .as_deref()
                .filter(|e| !e.is_empty())
                .is_some_and(|e| old_error.as_deref() != Some(e))
            || (old_state == Some(SessionState::Running)
                && match new.state {
                    SessionState::Waiting => true,
                    SessionState::Exited => !killed_here && new.exit_code.is_some_and(|c| c != 0),
                    SessionState::Running => false,
                });
        if worth_flagging && !watching {
            self.set_unread(new.project_path.clone(), true, cx);
        }
        if watching || !self.notify_on {
            return;
        }
        if new.asking && !old_asking && new.state != SessionState::Exited {
            // 权限请求能在横幅上直接答；结构化提问（AskUserQuestion）只能进会话，不给按钮
            let p = new.permission.as_ref();
            let body = match p {
                None => "待回复 · 等你选一个".to_string(),
                Some(p) if p.tool_name.is_empty() => p.summary.clone(),
                Some(p) => format!("{}：{}", p.tool_name, p.summary),
            };
            crate::notify::send(&new.display_title(), &body, &new.id, p.is_some());
            return;
        }
        if let Some(err) = new.error.as_deref().filter(|e| !e.is_empty())
            && old_error.as_deref() != Some(err)
        {
            crate::notify::send(&new.display_title(), &format!("出错 · {err}"), &new.id, false);
            return;
        }
        if old_state != Some(SessionState::Running) {
            return;
        }
        match new.state {
            SessionState::Waiting => crate::notify::send(&new.display_title(), "运行结束 · 等你下一步", &new.id, false),
            SessionState::Exited => {
                if !killed_here && new.exit_code.is_some_and(|c| c != 0) {
                    crate::notify::send(&new.display_title(), &format!("出错 · 退出码 {}", new.exit_code.unwrap_or(0)), &new.id, false);
                }
            }
            SessionState::Running => {}
        }
    }

    fn upsert_session(&mut self, session: Session, cx: &mut Context<Self>) {
        let id = session.id.clone();
        let alive = session.state != SessionState::Exited;
        let running = session.state == SessionState::Running;
        let perm = session.permission.clone();
        let asking_seq = session.asking_seq;
        let queued = session.queued.clone();
        match self.sessions.iter_mut().find(|s| s.id == session.id) {
            Some(slot) => *slot = session,
            None => self.sessions.push(session),
        }
        self.sort_sessions();
        if let Some(v) = self.msg_views.get(&id) {
            v.update(cx, |v, cx| v.set_session(alive, running, perm, asking_seq, queued.clone(), cx));
        }
        self.sessions_changed(cx);
        cx.notify();
    }

    /// 全量会话列表（快照 / 重连拉取）到达后，把存活状态同步给每个已开的消息流视图；
    /// 列表里没有的会话按已死处理（对话框随进程一起没了）
    fn sync_msg_alive_all(&self, cx: &mut Context<Self>) {
        for (id, view) in &self.msg_views {
            let (alive, running, perm, asking_seq, queued) = self
                .sessions
                .iter()
                .find(|s| &s.id == id)
                .map(|s| (s.state != SessionState::Exited, s.state == SessionState::Running, s.permission.clone(), s.asking_seq, s.queued.clone()))
                .unwrap_or((false, false, None, None, Vec::new()));
            view.update(cx, |v, cx| v.set_session(alive, running, perm, asking_seq, queued.clone(), cx));
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

    /// 只管发出去、不看返回体的请求（重命名 / 终止 / 关终端）：几处以前
    /// 各写一遍同一个 `|_, _: serde_json::Value, _| {}`，那个闭包里没有一个字是
    /// 某一处独有的。`toast_error` 留着——「删失败只记日志」和「终止失败要弹」
    /// 是两个不同的决定，不能一起写死。
    fn spawn_fetch_ignore(
        &self,
        fut: impl Future<Output = anyhow::Result<serde_json::Value>> + 'static,
        toast_error: bool,
        cx: &mut Context<Self>,
    ) {
        self.spawn_fetch(fut, |_, _: serde_json::Value, _| {}, toast_error, cx);
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
            self.net.agents(),
            |r, a: Vec<AgentInfo>, cx| {
                // 选中的 agent 没装（或表里没有）就退到第一个装了的
                if !a.iter().any(|x| x.id == r.new_agent && x.available) {
                    if let Some(first) = a.iter().find(|x| x.available) {
                        r.new_agent = first.id.clone();
                    }
                }
                r.agents = a;
                cx.notify();
            },
            false,
            cx,
        );
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
        // 进来了就把黄点清掉
        if let Some(path) = self.session(&id).map(|s| s.project_path.clone()) {
            self.set_unread(path, false, cx);
        }
        self.page = Page::Session(id.clone());
        self.pending_focus = Some(id.clone());
        self.reassert_visible_size(cx);
        // 详情面板开着就把这个会话的产物 / 改动 / 收件箱补齐
        self.refresh_detail(&id, cx);
        self.refresh_inbox(cx);
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
        self.files_views.remove(id);
        self.view_mode.remove(id);
        self.open_order.retain(|x| x != id);
        self.page = page_after_close(&self.page, id, &self.open_order);
        cx.notify();
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

    fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// 表里下一个装了的 agent。只装了一个就是 `None`——没有「换」这回事，
    /// 切换入口整个不画。例外：当前这个**没装**（卸载了 / 换了台机器）时给一条
    /// 回到装了的那个的路，否则这一行永远换不回来。
    fn next_agent(&self, cur: &str) -> Option<String> {
        let usable: Vec<&AgentInfo> = self.agents.iter().filter(|a| a.available).collect();
        let first = usable.first()?.id.clone();
        match usable.iter().position(|a| a.id == cur) {
            None => Some(first),
            Some(_) if usable.len() < 2 => None,
            Some(i) => Some(usable[(i + 1) % usable.len()].id.clone()),
        }
    }

    fn agent_label(&self, id: &str) -> String {
        self.agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label.clone())
            .unwrap_or_else(|| id.to_string())
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
                self.cycle_view(cx);
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
    }

    // ── 侧栏 ───────────────────────────────────────────────────────────
    //
    // 结构（自上而下）：新建项目输入框 → 项目列表（一项目一行，记号 + 标题，最近
    // 更新的在前）→ 终端一节（同一列里，与项目行平级）。没有大标题、没有总览页
    // ——侧栏本身就是全部导航。

    /// daemon 老到不下发项目状态（`/health` 的 `schema < 2`，PROTOCOL「版本兼容」）。
    /// `health` 还没到（刚启动 / 断线）时不算过旧——那会儿只是不知道，别先吓人一跳。
    fn schema_too_old(&self) -> bool {
        self.health.as_ref().is_some_and(|h| h.schema < SCHEMA_MIN)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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
            .child(self.render_new_project_row(cx))
            .when(self.schema_too_old(), |el| el.child(self.render_schema_banner()))
            .child(
                div()
                    .id("sb-scroll")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .pt(px(4.))
                    .child(self.render_sidebar_list(cx)),
            )
            // 会话日志入口
            .child(self.render_history_entry(cx))
            .child(self.render_sidebar_footer(conn_color, conn_text, cx))
            // 套餐用量：最底下两行小字（没有套餐信息就整块不画）
            .when_some(self.render_plan_usage(), |el, block| el.child(block))
    }

    /// 侧栏主体那一列：项目行一条条排下来，终端行接在同一列后面。
    ///
    /// ── 项目列表：单列（2026-09-06 用户拍板）──
    ///   整行淡底色说状态：淡蓝 = 在跑 / 淡黄 = 未读 / 无底 = 已读（2026-09-10 用户拍板，竖线也去掉了）。
    ///   问题本身不在侧栏画：进消息流，表单原生呈现、原地作答。exited 会话不代表项目
    ///   （点一下 resume）；终端（shell）不在这里（归终端面板）。
    fn render_sidebar_list(&self, cx: &mut Context<Self>) -> gpui::Div {
        let rows = project_rows(&self.projects, &self.sessions, &self.unread_projects);
        let now = chrono::Local::now();
        let mut list_col = div().flex().flex_col().gap(px(1.));
        for row in rows {
            list_col = list_col.child(self.render_project_row(row, now, cx));
        }
        // 终端与对话同级（2026-09-08 用户拍板）：终端不再是侧栏底部通往标签页的
        // 一个入口，而是接着项目行排在同一列里，点一行就是那一个终端。
        list_col.child(self.render_terminal_rows(cx))
    }

    /// 侧栏的一个项目行：标题 + 行尾按钮（悬停才现身）+ 更新时间。状态是整行的淡底色
    /// （[`row_bg`]），选中是整行一圈强调色边框（2026-09-10 用户拍板）。
    fn render_project_row(
        &self,
        row: ProjectRow,
        now: chrono::DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // 代表会话可能是已退出的那一条（daemon 在没有活会话时给的就是它）：
        // 点它 resume、行尾给「删」，一如既往——所以这几处一律走 live()
        let active = row
            .live()
            .is_some_and(|s| self.page == Page::Session(s.id.clone()));
        // 灰标题 = 暂停（没有活会话）且没有黄点
        let dim_title = row.status == "paused" && !row.unread;
        let open_session = row.live().map(|s| s.id.clone());
        let open_project = row.project.clone();
        let kill = row.live().map(|s| (s.id.clone(), kill_needs_confirm(s)));
        let del_path = row.project.as_ref().map(|p| p.path.clone());
        let bg = row_bg(&row.status, row.unread);
        // 悬停是在这一行自己的底上压深一点，不是换成灰底——否则鼠标一过，状态底就没了
        let hover_bg = theme::mix(bg.unwrap_or(theme::SURFACE), theme::INK, 0.06);
        let time = relative_time(&row.sort_key, now);
        let title = row.title;
        // 元素 id 用路径而不是序号：排序变了悬停 / 点击态跟着行走，不留在原位
        let row_id = SharedString::from(format!("sb-proj:{}", row.path));
        let act_id = SharedString::from(format!("sb-act:{}", row.path));
        let mut el = sidebar_row(row_id.into())
            .when_some(bg, |el, bg| el.bg(c(bg)))
            // 选中 = 整行一圈强调色边框：底色归状态用了，选中态不再抢整行的底
            .when(active, |el| el.border_color(c(theme::ACCENT)))
            .hover(move |st| st.bg(c(hover_bg)))
            // 点一下：活着的会话直接进；未激活的 resume（daemon 幂等，找不到旧对话开新的）
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(id) = &open_session {
                    this.open_session(id.clone(), cx);
                } else if let Some(p) = &open_project {
                    this.open_project(p, cx);
                }
            }))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .text_color(c(if active { theme::ACCENT } else if dim_title { theme::DIM } else { theme::INK }))
                    .child(SharedString::from(title)),
            );
        // 行尾：更新时间——「这个项目上一次有动静」；状态已经在底色里，这里不再挂记号。
        // 时间在按钮**前面**（2026-09-10 用户拍板「x/删 放在分钟数后面」）
        if !time.is_empty() {
            el = el.child(
                div()
                    .flex_none()
                    .text_size(px(10.5))
                    .text_color(c(theme::FAINT))
                    .child(SharedString::from(time)),
            );
        }
        // 最右：活着的 × = 结束会话（只有执行中的才确认，被顺手点掉最伤）；
        // 未激活的「删」= 删项目。一直画着，不再悬停才现身
        let button = row_btn(act_id)
            .hover(|st| st.text_color(c(theme::RED)).bg(c(theme::EDGE_LIGHT)));
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
        el
    }

    /// 侧栏顶上的新建项目行：输入框（字即文件夹名，回车或 ＋ 创建；⌘N 把光标放进来）
    /// + agent 轮换小标（装了不止一个 agent 时才出现）
    fn render_new_project_row(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .mt(px(10.))
            .mb(px(4.))
            .mx(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(div().flex_1().min_w(px(0.)).child(self.new_input.clone()))
            // 新项目开谁：点一下在装了的 agent 之间轮换。只有一个可用就不画
            .when_some(self.next_agent(&self.new_agent), |el, next| {
                el.child(
                    div()
                        .id("sb-new-agent")
                        .flex_none()
                        .h(px(28.))
                        .w(px(28.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(c(theme::EDGE_LIGHT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(c(theme::DIM))
                        .hover(|st| st.border_color(c(theme::ACCENT)).text_color(c(theme::ACCENT)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.new_agent = next.clone();
                            cx.notify();
                        }))
                        // 首字母，和项目行的小标同一套写法：侧栏最窄 180px，
                        // 一个「Antigravity」就把输入框挤没了
                        .child(SharedString::from(
                            self.agent_label(&self.new_agent).chars().next().unwrap_or('?').to_string(),
                        )),
                )
            })
            .child(
                div()
                    .id("sb-new")
                    .flex_none()
                    .h(px(28.))
                    .w(px(28.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(c(theme::EDGE_LIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)).border_color(c(theme::ACCENT)))
                    .on_click(cx.listener(|this, _, _, cx| this.create_project(cx)))
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(c(if self.creating { theme::FAINT } else { theme::ACCENT }))
                            .child(if self.creating { "…" } else { "＋" }),
                    ),
            )
    }

    /// schema 闸门（PROTOCOL「版本兼容」）：老 daemon 不下发项目状态，客户端
    /// **不保留第二套算法**——留着就等于把 v1.22 刚删掉的分歧又养回来。
    /// 所以这里只说实话：列表照画（无底色、标题退到目录名），顶上明写「不可用」。
    fn render_schema_banner(&self) -> gpui::Div {
        div()
            .flex_none()
            .mx(px(6.))
            .mb(px(4.))
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(ca(theme::AMBER, 0.1))
            .border_1()
            .border_color(ca(theme::AMBER, 0.4))
            .text_size(px(11.))
            .text_color(c(theme::AMBER))
            .child(SharedString::from(format!(
                "daemon 版本过旧（v{}），项目状态不可用 —— 请更新 daemon",
                self.health
                    .as_ref()
                    .map(|h| h.version.as_str())
                    .filter(|v| !v.is_empty())
                    .unwrap_or("?")
            )))
    }

    /// 侧栏最底下那条：连接状态点 + daemon 版本 + ⚙ 设置入口
    fn render_sidebar_footer(
        &self,
        conn_color: u32,
        conn_text: String,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
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
                        c(theme::ACCENT)
                    } else {
                        c(theme::DIM)
                    })
                    .hover(|st| st.text_color(c(theme::ACCENT)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Settings;
                        cx.notify();
                    }))
                    .child("⚙"),
            )
    }


    fn render_sidebar_resizer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let dragging = self.sidebar_drag.is_some();
        div()
            .id("sidebar-resizer")
            .w(px(4.))
            .flex_none()
            .h_full()
            .cursor_col_resize()
            .bg(c(if dragging { theme::ACCENT } else { theme::EDGE }))
            .hover(|st| st.bg(c(theme::ACCENT)))
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

    /// 打 / 清一个项目的黄点，变了才落盘。`cx` 只用来重画侧栏。
    fn set_unread(&mut self, path: String, on: bool, cx: &mut Context<Self>) {
        if crate::model::set_flagged(&mut self.unread_projects, &path, on) {
            self.ui_state().save();
            cx.notify();
        }
    }

    /// 本机偏好的当前快照（落盘用）
    fn ui_state(&self) -> UiState {
        UiState {
            sidebar_w: self.sidebar_w,
            detail_visible: self.detail_visible,
            notify: self.notify_on,
            unread_projects: self.unread_projects.clone(),
        }
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
                    // 不报 agent 名（侧栏那一行的小标已经说了）；有 resume id 才多一格
                    let resume_part = s
                        .resume_id
                        .as_deref()
                        .filter(|r| !r.is_empty())
                        .map(|r| format!("resume {}", &r[..r.len().min(6)]));
                    let state_color = theme::state_color(s.state.as_str());
                    let sid = s.id.clone();
                    let exited = s.state == SessionState::Exited;
                    let mut bar = bar
                        .child(
                            div()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(SharedString::from(s.project_path.clone())),
                        )
                        .when_some(resume_part, |bar, t| bar.child(SharedString::from(t)));

                    // 状态栏右侧那一排文字按钮（视图切换 + 会话操作）：一个样式，
                    // 只有字和颜色不同
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

                    // 三种看法各一格，当前那一格是主色（⌘E 也走同一条路）。
                    // 消息流那一格在 shell / 探明不支持时整个不画——不画比画一个
                    // 点不动的灰字诚实
                    let cur = self.view_of(&sid, cx);
                    let msgs_ok = self.msgs_available(&sid, cx);
                    bar = bar.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .children([SessionView::Terminal, SessionView::Messages, SessionView::Files].into_iter().filter_map(|v| {
                                if v == SessionView::Messages && !msgs_ok {
                                    return None;
                                }
                                let sid = sid.clone();
                                Some(
                                    act(
                                        match v {
                                            SessionView::Terminal => "view-term",
                                            SessionView::Messages => "view-msgs",
                                            SessionView::Files => "view-files",
                                        },
                                        v.label(),
                                        if v == cur { theme::ACCENT } else { theme::FAINT },
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_view(sid.clone(), v, cx);
                                    })),
                                )
                            })),
                    );

                    // 会话操作
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
                                if detail_on { theme::ACCENT } else { theme::FAINT },
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_detail(cx))),
                        )
                        .child(act("sess-rename", "重命名", theme::DIM).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_rename_modal(sid_rename.clone(), window, cx);
                            },
                        )));
                    if !exited {
                        let confirm = kill_needs_confirm(s);
                        bar = bar.child(act("sess-kill", "终止", theme::AMBER).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.request_kill(sid_kill.clone(), confirm, cx);
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

                    // 「待回复」盖过状态字：asking 是结构化事实，比 running/waiting 更要紧
                    let (label, color) = if s.asking {
                        ("待回复".to_string(), theme::AMBER)
                    } else if s.compacting {
                        ("整理上下文中".to_string(), state_color)
                    } else if let Some(e) = s.error_label() {
                        (e, theme::AMBER)
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
        self.win_w = f32::from(window.viewport_size().width);
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
                    // 记着的那一种视图拿不出实体（还没建 / 刚被清掉）就落回终端，
                    // 终端也没有才是「会话未打开」
                    let body: Option<gpui::AnyElement> = match self.view_of(&id, cx) {
                        SessionView::Messages => self.msg_views.get(&id).cloned().map(IntoElement::into_any_element),
                        SessionView::Files => self.files_views.get(&id).cloned().map(IntoElement::into_any_element),
                        SessionView::Terminal => None,
                    }
                    .or_else(|| self.terminals.get(&id).cloned().map(IntoElement::into_any_element));
                    match body {
                        Some(b) => el.child(div().flex_1().min_h(px(0.)).child(b)),
                        None => el.child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(c(theme::FAINT))
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
                        .text_color(c(theme::FAINT))
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

    /// 行尾那个时间（2026-09-08 用户「MacOS 这边也显示出来时间」）与 Android
    /// `relativeTime` 同口径：刚刚 / N 分钟前 / N 小时前 / MM-DD HH:mm
    #[test]
    fn row_time_reads_like_android() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-08T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Local);
        assert_eq!(relative_time("2026-09-08T11:59:31Z", now), "刚刚");
        assert_eq!(relative_time("2026-09-08T11:30:00Z", now), "30 分钟前");
        assert_eq!(relative_time("2026-09-08T09:00:00Z", now), "3 小时前");
        // 超过一天写日期，按**本地**时区（跑在哪个时区都得对，所以期望值也让 chrono 算）
        let far = "2026-09-05T09:00:00Z";
        let local = chrono::DateTime::parse_from_rfc3339(far).unwrap().with_timezone(&chrono::Local);
        assert_eq!(relative_time(far, now), local.format("%m-%d %H:%M").to_string());
        // 解析不了 / 没有时间戳就整块不画
        assert_eq!(relative_time("", now), "");
        assert_eq!(relative_time("不是时间", now), "");
        // daemon 的时钟稍微快一点也不该写成负数
        assert_eq!(relative_time("2026-09-08T12:00:30Z", now), "刚刚");
    }

    /// ⌘E 的轮换：终端 → 消息流 → 浏览 → 终端；没有消息流的会话（shell、老 daemon）
    /// 跳过那一档，两下就回到终端。
    #[test]
    fn view_cycle_skips_what_cannot_be_drawn() {
        use SessionView::*;
        assert_eq!(next_view(Terminal, true), Messages);
        assert_eq!(next_view(Messages, true), Files);
        assert_eq!(next_view(Files, true), Terminal);
        assert_eq!(next_view(Terminal, false), Files, "没有消息流就直接到浏览");
        assert_eq!(next_view(Files, false), Terminal);
        assert_eq!(next_view(Messages, false), Files, "记着消息流却已不支持：往下走，不卡住");
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

    /// 造一行 daemon 已经算好的项目（v1.22 的 `/projects` 一行）
    fn proj(path: &str, title: &str, status: &str, updated_at: &str) -> Project {
        Project {
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).into(),
            title: Some(title.into()),
            status: status.into(),
            updated_at: Some(updated_at.into()),
            ..Default::default()
        }
    }

    /// 一行的内容整块来自 daemon：标题、五态、排序时间、代表会话都不再自己推
    #[test]
    fn rows_take_everything_from_the_daemon_row() {
        use SessionState::*;
        let mut p = proj("/p/a", "改登录页", "running", "2026-09-04T00:00:00Z");
        p.session_id = Some("a1".into());
        let run = tsess("a1", "claude", Running, "/p/a", "2026-09-03T00:00:00Z");
        // 同项目下别的会话（已退出的、终端）不再参与任何判定：daemon 已经挑好代表了
        let old = tsess("a0", "claude", Exited, "/p/a", "2026-09-02T00:00:00Z");
        let shell = tsess("t1", "shell", Running, "/p/a", "2026-09-06T12:00:00Z");
        let rows = project_rows(&[p], &[run, old, shell], &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "改登录页");
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[0].sort_key, "2026-09-04T00:00:00Z");
        assert_eq!(rows[0].live().map(|s| s.id.as_str()), Some("a1"));
        assert_eq!(cyclable_ids(&[rows[0].project.clone().unwrap()], &[rows[0].session.clone().unwrap()]), ids(&["a1"]));
    }

    /// 代表会话可能是**已退出**的那一条（daemon 在没有活会话时给最近退出的一个）：
    /// 点它仍是 resume、行尾仍是「删」，所以 `live()` 只认还活着的
    #[test]
    fn exited_representative_session_is_not_live() {
        use SessionState::*;
        let mut p = proj("/p/a", "上次聊的", "paused", "2026-09-04T00:00:00Z");
        p.session_id = Some("a0".into());
        let gone = tsess("a0", "claude", Exited, "/p/a", "2026-09-02T00:00:00Z");
        let rows = project_rows(&[p.clone()], &[gone.clone()], &[]);
        assert!(rows[0].session.is_some(), "回放还要用它");
        assert!(rows[0].live().is_none(), "已退出：点行 resume，不是打开");
        // 会话根本不在手里（还没拉到 / 已被 GC）：查不到就是 None，不猜
        let rows = project_rows(&[p], &[], &[]);
        assert!(rows[0].session.is_none());
        // kill_needs_confirm 仍看会话自己的 state，与五态无关
        assert!(!kill_needs_confirm(&gone));
    }

    /// 没登记的目录（别处 `aaa open` 开的）daemon 自己补一行，客户端只是不给它
    /// resume / 删项目——**不再自己为「有会话没登记」补行**，那会补出重复的一行
    #[test]
    fn unregistered_rows_come_from_daemon_and_cannot_be_deleted() {
        use SessionState::*;
        let mut stray = proj("/elsewhere/x", "在别处开的", "running", "2026-09-09T00:00:00Z");
        stray.registered = false;
        stray.session_id = Some("s1".into());
        let sess = tsess("s1", "claude", Running, "/elsewhere/x", "2026-09-09T00:00:00Z");
        let rows = project_rows(&[stray], &[sess.clone()], &[]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].project.is_none(), "没注册表就没有 resume / 删项目");
        assert!(rows[0].live().is_some(), "但点得开");
        // 列表里没有这个目录时，客户端不会凭一个活会话变出一行来
        assert!(project_rows(&[], &[sess], &[]).is_empty());
    }

    /// 老 daemon（schema < 2）不给 title / updated_at：退到目录名和目录 mtime。
    /// 状态是空串 → 排最后、没有底色（横幅另说，见 schema_too_old）
    #[test]
    fn falls_back_to_dir_name_and_mtime_when_fields_missing() {
        let bare = Project {
            path: "/p/bare".into(),
            name: "bare".into(),
            mtime: "2026-09-03T00:00:00Z".into(),
            ..Default::default()
        };
        let rows = project_rows(&[bare], &[], &[]);
        assert_eq!(rows[0].title, "bare");
        assert_eq!(rows[0].sort_key, "2026-09-03T00:00:00Z");
        assert!(!status_running(&rows[0].status));
        assert_eq!(row_bg(&rows[0].status, false), None);
        // 黄盖过蓝：在跑但还没看过的行是黄底；看过的在跑行是蓝底；停着且看过的没有底
        assert_eq!(row_bg("running", true), Some(theme::ROW_UNREAD));
        assert_eq!(row_bg("background", false), Some(theme::ROW_RUNNING));
        assert_eq!(row_bg("asking", false), None, "在问 = 在等你，不是在跑");
        assert_eq!(row_bg("paused", false), None);
        // title 给了空串也退回目录名（空标题的行等于没有行）
        let mut empty = proj("/p/x", "", "paused", "");
        empty.name = "x".into();
        empty.mtime = "2026-09-01T00:00:00Z".into();
        let rows = project_rows(&[empty], &[], &[]);
        assert_eq!((rows[0].title.as_str(), rows[0].sort_key.as_str()), ("x", "2026-09-01T00:00:00Z"));
    }

    /// 三端共享向量 fixtures/projects.json：同一份 `/projects` + 同一份本机黄点，
    /// 两端必须排出同一个顺序、同一批蓝底黄底。此前两端各测各的，所以谁都没发现
    /// 两边推出来的标题和状态不一样。
    #[test]
    fn shared_fixture_project_rows() {
        let fx: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/projects.json")).unwrap();
        let projects: Vec<Project> = serde_json::from_value(fx["projects"].clone()).unwrap();
        let unread: Vec<String> = serde_json::from_value(fx["unread"].clone()).unwrap();
        let strs = |k: &str| -> Vec<String> {
            fx["expect"][k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
        };
        // 会话一个都没给：行只靠 daemon 那几个字段就能画全
        let rows = project_rows(&projects, &[], &unread);
        assert_eq!(rows.iter().map(|r| r.path.clone()).collect::<Vec<_>>(), strs("order"));
        // 淡蓝底 = status ∈ {running, background}；asking 不蓝，那是在等你
        let running = strs("running");
        for r in &rows {
            assert_eq!(status_running(&r.status), running.contains(&r.path), "在跑: {}", r.path);
        }
        // 淡黄底 = 本机 unread 集合里有它（路径去尾斜杠比较），且盖过蓝
        let unread_rows = strs("unread_rows");
        for r in &rows {
            assert_eq!(r.unread, unread_rows.contains(&r.path), "未读: {}", r.path);
            let want = if r.unread {
                Some(theme::ROW_UNREAD)
            } else if running.contains(&r.path) {
                Some(theme::ROW_RUNNING)
            } else {
                None
            };
            assert_eq!(row_bg(&r.status, r.unread), want, "底色: {}", r.path);
        }
        // 标题回退链已在 daemon 里走完，客户端直接用
        for (path, want) in fx["expect"]["titles"].as_object().unwrap() {
            let row = rows.iter().find(|r| &r.path == path).unwrap();
            assert_eq!(row.title, want.as_str().unwrap());
        }
        // 未读集合里存的是 /p/bg/（带尾斜杠）：查 /p/bg 命中、前缀相同的 /p/b 不命中
        for (path, want) in fx["expect"]["unread_path_normalized"].as_object().unwrap() {
            assert_eq!(path_list_contains(&unread, path), want.as_bool().unwrap(), "未读: {path}");
        }
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
