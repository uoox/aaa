//! 协议类型（PROTOCOL.md v1）+ 应用状态。
//! 所有字段尽量 `#[serde(default)]` 宽容解析，daemon 尚在并行开发中。

use serde::{Deserialize, Serialize};

// ── 会话 ────────────────────────────────────────────────────────────────────

/// 三态（2026-09-02 简化）：屏幕在变 = running；进程活着但可见屏幕 6s 没变 =
/// waiting（这轮干完了，轮到你）；进程退出 = exited。没有 idle 了——旧 daemon
/// 发来的 `idle` 按 waiting 解析，升级顺序错开也不会把客户端弄崩。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Running,
    #[default]
    #[serde(alias = "idle")]
    Waiting,
    Exited,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Running => "running",
            SessionState::Waiting => "waiting",
            SessionState::Exited => "exited",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub state: SessionState,
    /// claude：transcript 里有一条 AskUserQuestion 还没被回答（结构化事实，不是
    /// 读屏猜的）。其它 agent 没有这种信号，恒为 false。三态口径里的「待回复」。
    #[serde(default)]
    pub asking: bool,
    /// v1.16：正在等的权限对话框（Bash 授权 / ExitPlanMode 批准…）；有它就是待回复，
    /// 消息流画成「允许 / 拒绝」卡片
    #[serde(default)]
    pub permission: Option<PermissionPrompt>,
    /// v1.13：waiting 且后台还有任务（后台 Bash / 异步子代理 / Monitor）没回来——
    /// 主对话停在输入框，但它会自己被叫醒。行首标「后台」，排在运行之后
    #[serde(default)]
    pub background: bool,
    #[serde(default)]
    pub preview: String,
    #[serde(default)]
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
    #[serde(default)]
    pub pid: Option<i64>,
    #[serde(default)]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub resume_id: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_output_at: String,
    /// v1.5：状态翻转 / 改名的时刻（不随每个 PTY 字节跳），侧栏按它排序；
    /// 老 daemon 不给 → 空串，排序时退到 created_at
    #[serde(default)]
    pub updated_at: String,
    /// v1.3：状态由 Claude Code hooks 驱动
    #[serde(default)]
    pub hooked: bool,
    /// StopFailure 的错误类型（rate_limit / overloaded / authentication_failed…）
    #[serde(default)]
    pub error: Option<String>,
    /// 正在整理上下文
    #[serde(default)]
    pub compacting: bool,
    /// 用户自己结束的：退出不弹通知
    #[serde(default)]
    pub user_killed: bool,
    /// v1.4：Claude Code hooks 报的用量（模型 / 上下文占用 / 费用 / 改动行数）；
    /// 没到之前为 None，详情面板显示空态
    #[serde(default)]
    pub usage: Option<SessionUsage>,
    /// v1.7：整个对话的进度清单（`- [x] 已做` / `- [ ] 未做` 的 markdown），每轮结束后
    /// daemon 让 haiku 重写；详情面板「进度」
    #[serde(default)]
    pub summary: String,
}

/// 权限对话框（会话 JSON 的 `permission`）
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PermissionPrompt {
    /// permission（能替答）| elicitation（MCP 表单，只能去终端）
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub since: String,
}

/// 看板（GET /history/dashboard，2026-09-07 第二版：所有会话的进度，没有时间维度）
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Dashboard {
    #[serde(default)]
    pub counts: DashCounts,
    /// 故意没有 default：老 daemon 的响应缺它 → 解码失败 → 弹「请升级 daemon」，不静默画空
    pub sessions: Vec<SessionCard>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct DashCounts {
    #[serde(default)]
    pub asking: usize,
    #[serde(default)]
    pub running: usize,
    #[serde(default)]
    pub background: usize,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub paused: usize,
    #[serde(default)]
    pub open_items: usize,
}

/// 一张卡 = 一个会话的进度
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SessionCard {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub project_path: String,
    /// asking | running | background | active | paused
    #[serde(default)]
    pub status: String,
    /// 还在池子里（能点开，已退出的回放也算）
    #[serde(default)]
    pub alive: bool,
    #[serde(default)]
    pub deleted: bool,
    /// v1.15：项目已归档
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub done: usize,
    #[serde(default)]
    pub open: usize,
    #[serde(default)]
    pub items: Vec<ChecklistItem>,
    #[serde(default)]
    pub updated_at: String,
}

/// 看板搜索：标题 / 项目 / 任一清单项含关键字（不分大小写）；空串全匹配
pub fn card_matches(c: &SessionCard, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || c.title.to_lowercase().contains(&q)
        || c.project_name.to_lowercase().contains(&q)
        || c.items.iter().any(|i| i.text.to_lowercase().contains(&q))
}

/// 状态字（与侧栏同一套五态）
pub fn status_label(status: &str) -> &'static str {
    match status {
        "asking" => "待回复",
        "running" => "运行",
        "background" => "后台",
        "active" => "激活",
        _ => "暂停",
    }
}

/// 「已完成」= 暂停且清单全勾完（或没清单）：真正结束的活儿，看板默认收起来
pub fn card_is_finished(c: &SessionCard) -> bool {
    c.status == "paused" && c.open == 0
}

/// 进度清单的一项（解析 `summary` 的一行；看板卡片里由 daemon 直接给）
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
}

/// `- [x] …` / `- [ ] …` 行 → 项；其它行忽略
pub fn parse_checklist(md: &str) -> Vec<ChecklistItem> {
    md.lines()
        .filter_map(|raw| {
            let l = raw.trim().trim_start_matches(['-', '*']).trim_start();
            let (done, rest) = if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
                (true, r)
            } else if let Some(r) = l.strip_prefix("[ ]") {
                (false, r)
            } else {
                return None;
            };
            let text = rest.trim();
            (!text.is_empty()).then(|| ChecklistItem { done, text: text.to_string() })
        })
        .collect()
}

/// 会话用量（Session JSON 的 `usage`）。字段全部可缺省：daemon 拿到多少给多少。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionUsage {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    /// 0–100
    #[serde(default)]
    pub context_pct: Option<f64>,
    #[serde(default)]
    pub context_window_size: Option<u64>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub lines_added: Option<u64>,
    #[serde(default)]
    pub lines_removed: Option<u64>,
    #[serde(default)]
    pub effort: Option<String>,
    /// v1.8：提示缓存——最近一次调用里从缓存读 / 新写进缓存 / 新读的 token，与命中率（0–100）
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub fresh_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_hit_pct: Option<f64>,
}

// ── 套餐用量（GET /usage、`usage` 帧） ──────────────────────────────────────

/// 重置时刻：daemon 可能给 unix 秒（或毫秒）数字，也可能给 ISO 串，两种都收
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ResetsAt {
    Epoch(f64),
    Text(String),
}

impl ResetsAt {
    /// 统一成 UTC 时刻；解析不了就 None（不猜）
    pub fn to_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            // 1e12 以上的数字只可能是毫秒（秒级要到公元 33658 年才有这么大）
            ResetsAt::Epoch(n) => {
                let secs = if *n > 1e12 { *n / 1000.0 } else { *n };
                chrono::DateTime::from_timestamp(secs as i64, 0)
            }
            ResetsAt::Text(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
                .ok()
                .map(|d| d.with_timezone(&chrono::Utc)),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct PlanWindow {
    /// 0–100
    #[serde(default)]
    pub used_percentage: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<ResetsAt>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ModelWindow {
    #[serde(default)]
    pub display_name: String,
    /// 0–100
    #[serde(default)]
    pub utilization: Option<f64>,
    #[serde(default)]
    pub resets_at: Option<ResetsAt>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct PlanUsage {
    #[serde(default)]
    pub five_hour: Option<PlanWindow>,
    #[serde(default)]
    pub seven_day: Option<PlanWindow>,
    #[serde(default)]
    pub model_scoped: Option<Vec<ModelWindow>>,
}

/// `GET /usage` → `{"plan": null | {…}}`
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UsageResponse {
    #[serde(default)]
    pub plan: Option<PlanUsage>,
}

// ── 产物 / 改动 / 收件箱 ────────────────────────────────────────────────────

/// `GET /sessions/:id/artifacts` 的一项：会话发布过的 Artifact 页面
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Artifact {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub file_path: String,
    /// ISO 时间
    #[serde(default)]
    pub ts: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ArtifactsResponse {
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// 任务收件箱一项（按项目路径归属）
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct InboxItem {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub created_at: String,
}

impl Session {
    /// StopFailure 的错误类型 → 一句人话
    pub fn error_label(&self) -> Option<String> {
        self.error.as_deref().map(|k| match k {
            "rate_limit" => "上轮出错：限流".to_string(),
            "overloaded" => "上轮出错：服务过载".to_string(),
            "authentication_failed" => "上轮出错：登录失效".to_string(),
            "billing_error" => "上轮出错：账单问题".to_string(),
            other => format!("上轮出错：{other}"),
        })
    }

    /// 终端 = `agent:"shell"` 的会话：常驻工具，不是项目的会话（PROTOCOL「终端」）。
    /// 不进三态分组、不算项目激活、不参与 ⌃Tab，归终端面板管。
    pub fn is_terminal(&self) -> bool {
        self.agent == "shell"
    }

    /// 侧栏/tab 显示名：title 为空时退回 agent 或 zsh
    pub fn display_title(&self) -> String {
        if !self.title.is_empty() {
            self.title.clone()
        } else if self.agent == "shell" {
            "zsh".to_string()
        } else if !self.project_name.is_empty() {
            self.project_name.clone()
        } else {
            self.id.clone()
        }
    }
}

// ── 项目 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Project {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mtime: String,
    #[serde(default)]
    pub dir_size: u64,
    #[serde(default)]
    pub ctx_size: u64,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub session_title: Option<String>,
    /// v1.8：置顶（daemon 侧存，三端一起变）
    #[serde(default)]
    pub pinned: bool,
    /// v1.15：归档（daemon 侧存）——侧栏 / 看板默认藏起来，一个开关翻出来
    #[serde(default)]
    pub archived: bool,
}

// ── 其它 REST 响应 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Health {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub ssd_mounted: bool,
    #[serde(default)]
    pub project_root: String,
    #[serde(default)]
    pub uptime_s: u64,
    /// 二进制被重新构建过、跑的还是旧进程：设置页亮「需重启」
    #[serde(default)]
    pub update_pending: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PurgeEntry {
    #[serde(default)]
    pub agent_label: String,
    #[serde(default)]
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeleteResult {
    pub path: String,
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub purged: Vec<PurgeEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeleteResponse {
    #[serde(default)]
    pub results: Vec<DeleteResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PairResponse {
    #[serde(default)]
    pub payload: String,
}

/// GET /sessions/:id/ports（Web 预览入口）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PortEntry {
    #[serde(default)]
    pub port: u16,
    #[serde(default)]
    pub cmd: String,
}

// ── 错误 ────────────────────────────────────────────────────────────────────

// ── v1.1 消息流 ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ToolInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub status: String, // ok|err|running
}

/// AskUserQuestion 的一个选项（原样来自工具入参）
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct QOption {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// AskUserQuestion 的一道题；`multi_select` 决定画单选还是复选
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct QuestionItem {
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub question: String,
    #[serde(default)]
    pub options: Vec<QOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// `kind:"question"` 消息携带的整张表单（一次工具调用可以问好几题）
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct QuestionSpec {
    #[serde(default)]
    pub questions: Vec<QuestionItem>,
}

/// `POST /sessions/:id/answer` 里的一项：对应一题，顺序同 `QuestionSpec::questions`。
/// `selected` 是 0 起的选项下标；`other` 是「其它」自填，空就不发字段。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct AnswerItem {
    pub selected: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub seq: u64,
    /// ISO 时间（transcript 里带毫秒）；待答判定拿它与会话 created_at 比
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub role: String, // user|assistant|tool|system
    #[serde(default)]
    pub kind: String, // text|thinking|tool_use|tool_result|question|answer
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tool: Option<ToolInfo>,
    /// 仅 `kind:"question"`：结构化表单，客户端原生画对话框
    #[serde(default)]
    pub question: Option<QuestionSpec>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessagesResponse {
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub last_seq: u64,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiError {
    #[serde(default)]
    pub error: ApiErrorBody,
}

// ── /events WS 帧 ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
// 变体大小差异是刻意的：这些是每帧从 WS 反序列化出来、立刻被消费掉的短命值，
// 装箱换来的是每个事件一次堆分配，比多占几百字节栈更贵。
#[allow(clippy::large_enum_variant)]
pub enum DaemonEvent {
    Snapshot { sessions: Vec<Session> },
    Session { session: Session },
    SessionRemoved { id: String },
    ProjectsChanged {},
    Health {
        ssd_mounted: bool,
    },
    /// v1.1 消息流：该会话的 agent 存储有新消息（≥500ms 节流）
    MessagesChanged {
        id: String,
        #[serde(default)]
        last_seq: u64,
    },
    /// v1.4：套餐用量刷新（plan 为 null = 拿不到套餐信息，侧栏隐藏该块）
    Usage {
        #[serde(default)]
        plan: Option<PlanUsage>,
    },
    /// 收件箱变了（按项目路径）：详情面板正显示该项目就重拉
    InboxChanged {
        #[serde(default)]
        path: String,
    },
    /// 未知帧向前兼容（已移除的 session_stalled 也落到这里）
    #[serde(other)]
    Unknown,
}

// ── attach WS 帧 ────────────────────────────────────────────────────────────

// 同上：短命的反序列化帧，装箱不划算
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum AttachServerMsg {
    Hello {
        session: Session,
        #[serde(default)]
        rows: u16,
        #[serde(default)]
        cols: u16,
    },
    #[serde(other)]
    Unknown,
}

pub fn resize_frame(cols: u16, rows: u16) -> String {
    format!("{{\"t\":\"resize\",\"cols\":{cols},\"rows\":{rows}}}")
}

// ── daemon config.toml（同机零配置） ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DaemonConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub token: String,
    // 其余字段（project_root / ntfy…）serde 默认忽略，UI 端用不到
}

fn default_port() -> u16 {
    2730
}

impl DaemonConfig {
    pub fn load() -> Option<Self> {
        let path = dirs::home_dir()?.join(".config/aaa-daemon/config.toml");
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }
}

// ── UI 本机偏好（~/.config/aaa-ui/ui.toml） ─────────────────────────────────

/// 侧栏宽度可拖区间：窄于 180 会话标题只剩两三个字，宽于 480 终端就没地方了。
pub const SIDEBAR_W_MIN: f32 = 180.0;
pub const SIDEBAR_W_MAX: f32 = 480.0;
pub const SIDEBAR_W_DEFAULT: f32 = 210.0;

/// 拖拽/读盘两条路径共用一个夹取，任何来源的脏值都不会把侧栏拖没。
pub fn clamp_sidebar_width(w: f32) -> f32 {
    if w.is_nan() {
        return SIDEBAR_W_DEFAULT;
    }
    w.clamp(SIDEBAR_W_MIN, SIDEBAR_W_MAX)
}

fn default_sidebar_w() -> f32 {
    SIDEBAR_W_DEFAULT
}

/// 主题默认黑暗（原始设计令牌）；值的解释见 theme::ThemeKind::from_str
pub const THEME_DEFAULT: &str = "dark";

fn default_theme() -> String {
    THEME_DEFAULT.to_string()
}

/// daemon 的 config.toml 是双方的契约，UI 不往里写；窗口布局、主题这类只属于
/// 本机的偏好另起一个文件。读写失败一律回落默认值——配置坏了也必须能开窗。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    /// "dark" | "light" | "claude"（认不出的按 dark）
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 会话页右侧详情面板是否展开（⌘I 切换；默认展开）
    #[serde(default = "default_true")]
    pub detail_visible: bool,
    /// 静音通知的项目路径（详情面板「通知」段的开关）
    #[serde(default)]
    pub muted_projects: Vec<String>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            sidebar_w: SIDEBAR_W_DEFAULT,
            theme: default_theme(),
            detail_visible: true,
            muted_projects: Vec::new(),
        }
    }
}

/// 项目是否被静音：路径按去尾斜杠后精确匹配（同一目录写法不同不该算两个项目）
pub fn is_muted(muted: &[String], project_path: &str) -> bool {
    let p = project_path.trim_end_matches('/');
    !p.is_empty() && muted.iter().any(|m| m.trim_end_matches('/') == p)
}

/// 切换静音；返回切换后是否静音
pub fn toggle_muted(muted: &mut Vec<String>, project_path: &str) -> bool {
    let p = project_path.trim_end_matches('/');
    if p.is_empty() {
        return false;
    }
    if is_muted(muted, p) {
        muted.retain(|m| m.trim_end_matches('/') != p);
        false
    } else {
        muted.push(p.to_string());
        true
    }
}

impl UiState {
    fn path() -> Option<std::path::PathBuf> {
        Some(dirs::home_dir()?.join(".config/aaa-ui/ui.toml"))
    }

    pub fn load() -> Self {
        let mut s: UiState = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        s.sidebar_w = clamp_sidebar_width(s.sidebar_w);
        s
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, toml::to_string(self).unwrap_or_default())
        };
        if let Err(e) = write() {
            log::warn!("写入 {} 失败: {e}", path.display());
        }
    }
}

/// 连接参数（config.toml 读取成功 or 手动输入）
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub token: String,
}

impl Endpoint {
    pub fn from_config(c: &DaemonConfig) -> Self {
        Endpoint {
            host: "127.0.0.1".into(),
            port: c.port,
            token: c.token.clone(),
        }
    }
    pub fn http_base(&self) -> String {
        format!("http://{}:{}/api/v1", self.host, self.port)
    }
    pub fn ws_base(&self) -> String {
        format!("ws://{}:{}/api/v1", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_full_roundtrip() {
        let j = r#"{
          "id": "s_9f2c81ab",
          "title": "aaa-ui 交互原型设计",
          "project_path": "/Volumes/SSD/project/aaa-ui",
          "project_name": "aaa-ui",
          "agent": "claude",
          "state": "waiting",
          "asking": true,
          "preview": "…最近 4 行纯文本…",
          "rows": 40, "cols": 120,
          "pid": 12345, "exit_code": null,
          "resume_id": "9f2c81",
          "created_at": "2026-08-30T13:47:00Z", "last_output_at": "2026-08-30T13:51:00Z"
        }"#;
        let s: Session = serde_json::from_str(j).unwrap();
        assert_eq!(s.state, SessionState::Waiting);
        assert!(s.asking);
        assert_eq!(s.rows, 40);
        assert_eq!(s.pid, Some(12345));
        assert_eq!(s.exit_code, None);
        assert_eq!(s.display_title(), "aaa-ui 交互原型设计");
    }

    #[test]
    fn shell_agent_is_terminal() {
        let s: Session = serde_json::from_str(r#"{"id":"s_1","agent":"shell"}"#).unwrap();
        assert!(s.is_terminal());
        let s: Session = serde_json::from_str(r#"{"id":"s_2","agent":"claude"}"#).unwrap();
        assert!(!s.is_terminal());
    }

    /// 三端共享向量 fixtures/dashboard.json：客户端的过滤口径（已完成 / 搜索）必须得到 expect
    #[test]
    fn shared_fixture_filters() {
        let fx: serde_json::Value = serde_json::from_str(include_str!("../../fixtures/dashboard.json")).unwrap();
        let d: Dashboard = serde_json::from_value(serde_json::json!({"sessions": fx["sessions"]})).unwrap();
        let ids = |f: &dyn Fn(&SessionCard) -> bool| d.sessions.iter().filter(|c| f(c)).map(|c| c.id.clone()).collect::<Vec<_>>();
        let expect = |k: &str| fx["expect"][k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(ids(&|c| card_is_finished(c) && !c.deleted), expect("finished"));
        assert_eq!(ids(&|c| card_matches(c, "测试")), expect("match_测试"));
        // 顺序照 daemon 给的，客户端不重排
        assert_eq!(d.sessions.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["ask", "run", "bg", "act", "fin", "old", "arch", "del"]);
        assert_eq!(ids(&|c| !c.deleted && !c.archived), expect("visible_default"));
    }

    #[test]
    fn dashboard_cards_search_labels_and_finished() {
        let c = SessionCard {
            title: "改登录页".into(),
            project_name: "Shop".into(),
            status: "paused".into(),
            open: 1,
            items: vec![ChecklistItem { done: false, text: "补测试".into() }],
            ..Default::default()
        };
        assert!(card_matches(&c, "") && card_matches(&c, "登录") && card_matches(&c, "shop") && card_matches(&c, "测试"));
        assert!(!card_matches(&c, "支付"));
        assert_eq!(status_label("asking"), "待回复");
        assert_eq!(status_label("background"), "后台");
        assert_eq!(status_label("whatever"), "暂停");
        assert!(!card_is_finished(&c), "暂停但还有没勾的：不算完");
        let done = SessionCard { status: "paused".into(), open: 0, ..Default::default() };
        assert!(card_is_finished(&done));
        let live = SessionCard { status: "active".into(), open: 0, ..Default::default() };
        assert!(!card_is_finished(&live), "还活着的不算完");
        // 老 daemon 的形状（没有 sessions）必须解码失败，不能静默画空看板
        assert!(serde_json::from_str::<Dashboard>(r#"{"today":{"sessions":1},"days":[]}"#).is_err());
        assert!(serde_json::from_str::<Dashboard>(r#"{"sessions":[]}"#).is_ok());
    }

    #[test]
    fn session_summary_checklist_parses() {
        let s: Session = serde_json::from_str(r#"{"id":"s","summary":"- [x] 修好登录页\n- [ ] 补测试\n瞎话"}"#).unwrap();
        let items = parse_checklist(&s.summary);
        assert_eq!(items.len(), 2);
        assert!(items[0].done && items[0].text == "修好登录页");
        assert!(!items[1].done && items[1].text == "补测试");
        let old: Session = serde_json::from_str(r#"{"id":"s"}"#).unwrap();
        assert!(parse_checklist(&old.summary).is_empty(), "老 daemon 没有这个字段");
    }

    #[test]
    fn session_minimal() {
        // daemon 早期实现可能字段不全，必须能解析；asking 缺省 false
        let s: Session = serde_json::from_str(r#"{"id":"s_1","agent":"shell"}"#).unwrap();
        assert_eq!(s.state, SessionState::Waiting);
        assert!(!s.asking);
        assert_eq!(s.display_title(), "zsh");
    }

    #[test]
    fn idle_from_old_daemon_parses_as_waiting() {
        // 2026-09-02 之前的 daemon 还会发 idle：别名兜住，不能让升级顺序把 UI 弄崩
        let s: Session = serde_json::from_str(r#"{"id":"s_1","state":"idle"}"#).unwrap();
        assert_eq!(s.state, SessionState::Waiting);
        // 序列化只认新词
        assert_eq!(serde_json::to_string(&SessionState::Waiting).unwrap(), "\"waiting\"");
        assert!(serde_json::from_str::<SessionState>("\"bogus\"").is_err());
    }

    #[test]
    fn chat_message_question_parses() {
        let j = r#"{"seq":7,"ts":"…","role":"assistant","kind":"question",
          "text":"Pick a color",
          "tool":{"name":"AskUserQuestion","summary":"Color · Tools","status":"running"},
          "question":{"questions":[
            {"header":"Color","question":"Pick a color",
             "options":[{"label":"Red","description":"A warm color"},{"label":"Blue"}],
             "multi_select":false},
            {"question":"Which tools?","options":[{"label":"Bash"},{"label":"Read"},{"label":"Edit"}],
             "multi_select":true}
          ]}}"#;
        let m: ChatMessage = serde_json::from_str(j).unwrap();
        assert_eq!(m.kind, "question");
        let q = m.question.as_ref().expect("question 字段");
        assert_eq!(q.questions.len(), 2);
        assert_eq!(q.questions[0].header, "Color");
        assert_eq!(q.questions[0].options[0].description, "A warm color");
        assert_eq!(q.questions[0].options[1].description, "", "description 可缺省");
        assert!(!q.questions[0].multi_select);
        assert_eq!(q.questions[1].header, "", "header 可缺省");
        assert!(q.questions[1].multi_select);
        assert_eq!(q.questions[1].options.len(), 3);
        // 普通消息没有 question 字段
        let m: ChatMessage =
            serde_json::from_str(r#"{"seq":8,"role":"user","kind":"answer","text":"Red"}"#).unwrap();
        assert_eq!(m.kind, "answer");
        assert!(m.question.is_none());
    }

    #[test]
    fn answer_item_shape() {
        // 与 PROTOCOL 一致：selected 下标数组；other 为空时整个字段不发
        let v = serde_json::to_value(vec![
            AnswerItem { selected: vec![0, 2], other: None },
            AnswerItem { selected: vec![], other: Some("Zed".into()) },
        ])
        .unwrap();
        assert_eq!(v[0]["selected"], serde_json::json!([0, 2]));
        assert!(v[0].get("other").is_none());
        assert_eq!(v[1]["selected"], serde_json::json!([]));
        assert_eq!(v[1]["other"], "Zed");
    }

    #[test]
    fn events_frames() {
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"snapshot","sessions":[{"id":"s_1"}]}"#).unwrap();
        match e {
            DaemonEvent::Snapshot { sessions } => assert_eq!(sessions.len(), 1),
            _ => panic!("wrong variant"),
        }
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"session","session":{"id":"s_2","state":"running"}}"#)
                .unwrap();
        match e {
            DaemonEvent::Session { session } => {
                assert_eq!(session.state, SessionState::Running)
            }
            _ => panic!("wrong variant"),
        }
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"session_removed","id":"s_3"}"#).unwrap();
        assert!(matches!(e, DaemonEvent::SessionRemoved { id } if id == "s_3"));
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"projects_changed"}"#).unwrap();
        assert!(matches!(e, DaemonEvent::ProjectsChanged {}));
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"health","ssd_mounted":false}"#).unwrap();
        assert!(matches!(e, DaemonEvent::Health { ssd_mounted: false }));
        // watchdog 已移除（2026-09-02）：旧 daemon 若还发 session_stalled，当未知帧忽略
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"session_stalled","id":"s_4","quiet_s":612}"#).unwrap();
        assert!(matches!(e, DaemonEvent::Unknown));
        // 向前兼容：未知帧不报错（inbox_changed 等）
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"future_frame","x":1}"#).unwrap();
        assert!(matches!(e, DaemonEvent::Unknown));
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"messages_changed","id":"s_1","last_seq":42}"#).unwrap();
        assert!(
            matches!(e, DaemonEvent::MessagesChanged { ref id, last_seq: 42 } if id == "s_1")
        );
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"inbox_changed","path":"/p/x"}"#).unwrap();
        assert!(matches!(e, DaemonEvent::InboxChanged { ref path } if path == "/p/x"));
        // usage 帧：plan 可为 null
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"usage","plan":null}"#).unwrap();
        assert!(matches!(e, DaemonEvent::Usage { plan: None }));
        let e: DaemonEvent = serde_json::from_str(
            r#"{"t":"usage","plan":{"five_hour":{"used_percentage":32,"resets_at":1757000000},
                "seven_day":null,"model_scoped":[{"display_name":"Fable","utilization":40.5,"resets_at":"2026-09-04T06:00:00Z"}],
                "updated_at":"2026-09-03T10:00:00Z"}}"#,
        )
        .unwrap();
        match e {
            DaemonEvent::Usage { plan: Some(p) } => {
                assert_eq!(p.five_hour.as_ref().unwrap().used_percentage, Some(32.0));
                assert!(p.seven_day.is_none());
                let m = &p.model_scoped.unwrap()[0];
                assert_eq!(m.display_name, "Fable");
                assert_eq!(m.utilization, Some(40.5));
                assert!(matches!(m.resets_at, Some(ResetsAt::Text(_))));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn session_usage_parses_and_is_optional() {
        let s: Session = serde_json::from_str(r#"{"id":"s_1","usage":null}"#).unwrap();
        assert!(s.usage.is_none());
        let s: Session = serde_json::from_str(
            r#"{"id":"s_1","usage":{"model":"Fable 5.1","model_id":"claude-fable-5-1","context_pct":30.5,
                "context_window_size":200000,"input_tokens":1000,"output_tokens":200,"cost_usd":1.25,
                "duration_ms":65000,"lines_added":10,"lines_removed":2,"effort":"high"}}"#,
        )
        .unwrap();
        let u = s.usage.unwrap();
        assert_eq!(u.model.as_deref(), Some("Fable 5.1"));
        assert_eq!(u.context_pct, Some(30.5));
        assert_eq!(u.cost_usd, Some(1.25));
        assert_eq!(u.lines_added, Some(10));
        assert_eq!(u.effort.as_deref(), Some("high"));
        // 字段不全也能解
        let s: Session = serde_json::from_str(r#"{"id":"s_1","usage":{"model":"x"}}"#).unwrap();
        assert_eq!(s.usage.unwrap().context_pct, None);
    }

    #[test]
    fn resets_at_epoch_seconds_millis_and_iso() {
        let t = ResetsAt::Epoch(1_757_000_000.0).to_utc().unwrap();
        assert_eq!(t.timestamp(), 1_757_000_000);
        // 毫秒也认
        let t = ResetsAt::Epoch(1_757_000_000_000.0).to_utc().unwrap();
        assert_eq!(t.timestamp(), 1_757_000_000);
        let t = ResetsAt::Text("2026-09-04T06:00:00Z".into()).to_utc().unwrap();
        assert_eq!(t.to_rfc3339(), "2026-09-04T06:00:00+00:00");
        assert!(ResetsAt::Text("tomorrow".into()).to_utc().is_none());
        // usage 响应：plan 为 null
        let r: UsageResponse = serde_json::from_str(r#"{"plan":null}"#).unwrap();
        assert!(r.plan.is_none());
    }

    #[test]
    fn artifacts_inbox_parse() {
        let a: ArtifactsResponse = serde_json::from_str(
            r#"{"artifacts":[{"url":"https://claude.ai/a/1","title":"报告","description":"desc","file_path":"/x.html","ts":"2026-09-03T10:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(a.artifacts[0].title, "报告");
        let i: Vec<InboxItem> =
            serde_json::from_str(r#"[{"id":"i1","text":"修 bug","created_at":"2026-09-03T10:00:00Z"}]"#)
                .unwrap();
        assert_eq!(i[0].text, "修 bug");
    }

    #[test]
    fn mute_lookup_and_toggle() {
        let mut muted: Vec<String> = vec!["/p/a/".into()];
        // 尾斜杠不影响匹配
        assert!(is_muted(&muted, "/p/a"));
        assert!(is_muted(&muted, "/p/a/"));
        assert!(!is_muted(&muted, "/p/ab"));
        assert!(!is_muted(&muted, ""));
        // 切换：开 → 关 → 开
        assert!(!toggle_muted(&mut muted, "/p/a"));
        assert!(muted.is_empty());
        assert!(toggle_muted(&mut muted, "/p/b/"));
        assert_eq!(muted, vec!["/p/b".to_string()]);
        assert!(is_muted(&muted, "/p/b"));
        // 空路径不记
        assert!(!toggle_muted(&mut muted, ""));
        assert_eq!(muted.len(), 1);
    }

    #[test]
    fn attach_hello() {
        let m: AttachServerMsg = serde_json::from_str(
            r#"{"t":"hello","session":{"id":"s_1","state":"running"},"rows":40,"cols":120}"#,
        )
        .unwrap();
        match m {
            AttachServerMsg::Hello { session, rows, cols } => {
                assert_eq!(session.id, "s_1");
                assert_eq!((rows, cols), (40, 120));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn resize_frame_shape() {
        let f = resize_frame(120, 40);
        let v: serde_json::Value = serde_json::from_str(&f).unwrap();
        assert_eq!(v["t"], "resize");
        assert_eq!(v["cols"], 120);
        assert_eq!(v["rows"], 40);
    }

    #[test]
    fn purge_report() {
        let j = r#"{"results":[{"path":"/p/a","ok":true,
            "purged":[{"agent_label":"Claude","count":14},{"agent_label":"Grok","count":1}]}]}"#;
        let r: DeleteResponse = serde_json::from_str(j).unwrap();
        assert_eq!(r.results[0].purged[0].agent_label, "Claude");
        assert_eq!(r.results[0].purged[1].count, 1);
    }

    #[test]
    fn api_error() {
        let e: ApiError =
            serde_json::from_str(r#"{"error":{"code":"ssd_unmounted","message":"SSD 未挂载"}}"#)
                .unwrap();
        assert_eq!(e.error.code, "ssd_unmounted");
    }

    #[test]
    fn config_parse() {
        let c: DaemonConfig = toml::from_str(
            "port = 2730\ntoken = \"aaa_tk_deadbeef\"\nproject_root = \"/Volumes/SSD/project\"\n[ntfy]\nurl = \"https://x\"\ntopic = \"aaa\"\n",
        )
        .unwrap();
        assert_eq!(c.port, 2730);
        assert_eq!(c.token, "aaa_tk_deadbeef");
        let ep = Endpoint::from_config(&c);
        assert_eq!(ep.http_base(), "http://127.0.0.1:2730/api/v1");
        assert_eq!(ep.ws_base(), "ws://127.0.0.1:2730/api/v1");
    }

    #[test]
    fn sidebar_width_clamped() {
        assert_eq!(clamp_sidebar_width(300.0), 300.0);
        assert_eq!(clamp_sidebar_width(10.0), SIDEBAR_W_MIN);
        assert_eq!(clamp_sidebar_width(9999.0), SIDEBAR_W_MAX);
        assert_eq!(clamp_sidebar_width(f32::NAN), SIDEBAR_W_DEFAULT);
        // 边界含端点，拖到极限不应被再挪一格
        assert_eq!(clamp_sidebar_width(SIDEBAR_W_MIN), SIDEBAR_W_MIN);
        assert_eq!(clamp_sidebar_width(SIDEBAR_W_MAX), SIDEBAR_W_MAX);
    }

    #[test]
    fn ui_state_roundtrip_and_tolerance() {
        let s = UiState {
            sidebar_w: 320.0,
            theme: "claude".into(),
            detail_visible: false,
            muted_projects: vec!["/p/a".into()],
        };
        let text = toml::to_string(&s).unwrap();
        let back: UiState = toml::from_str(&text).unwrap();
        assert_eq!(back.sidebar_w, 320.0);
        assert_eq!(back.theme, "claude");
        assert!(!back.detail_visible);
        assert_eq!(back.muted_projects, vec!["/p/a".to_string()]);
        // 缺字段（旧版本写的文件）用默认值补齐，不报错；详情面板默认展开
        let empty: UiState = toml::from_str("").unwrap();
        assert_eq!(empty.sidebar_w, SIDEBAR_W_DEFAULT);
        assert_eq!(empty.theme, THEME_DEFAULT);
        assert!(empty.detail_visible);
        assert!(empty.muted_projects.is_empty());
        // 只有旧字段的文件：主题回落默认，不报错
        let old: UiState = toml::from_str("sidebar_w = 250.0\n").unwrap();
        assert_eq!(old.theme, "dark");
        assert_eq!(old.sidebar_w, 250.0);
    }
}
