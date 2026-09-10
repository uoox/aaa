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

/// 排着还没送进去的一条（会话的 `queued`）。没有 id——它不是 AAA 的队列，撤回要去 TUI 里做。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct QueuedMsg {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub text: String,
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
    pub rows: u16,
    #[serde(default)]
    pub cols: u16,
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
    /// daemon 让 haiku 重写。**v1.22 起客户端不再自己解析它**：解析结果在 `checklist`
    /// 里，原文只留着调试用
    #[serde(default)]
    pub summary: String,
    /// v1.22：待答的是消息流里的哪一条 `seq`（没有待答 = None）。此前每端自己从消息流
    /// 尾部倒着找 question/answer、还要拿 `ts` 的前 19 字符跟会话 created_at 比大小去
    /// 掉「resume 带进来的旧问题」——同一个判定三端各写一遍，答案还不一样。
    #[serde(default)]
    pub asking_seq: Option<u64>,
    /// v1.22：此刻排着还没送进去的消息。**队列是 Claude Code 自己的**（模型在跑时往 TUI
    /// 里敲的字它自己排着，这一轮结束再送进去），daemon 从 transcript 读出来下发；外加
    /// 信任对话框弹着时 daemon 替用户收下的那几条。客户端只画不管——用户拍板「排队发送
    /// 按照 claude code 逻辑，不需要另外实现这个功能」。
    #[serde(default)]
    pub queued: Vec<QueuedMsg>,
    /// v1.22：`summary` 那串 markdown 由 daemon 解析好的清单（看板卡片的 `items` 同源）
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
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

/// 淡蓝底（项目行）：它还在动，不用你管（自己在跑，或后台任务还没回来）。
/// `asking` 不算——那是在等你。
pub fn status_running(s: &str) -> bool {
    s == "running" || s == "background"
}

/// 进度清单的一项。**只剩 serde 用途**：v1.22 起 `- [x] …` 的解析归 daemon，看板卡片的
/// `items` 与会话的 `checklist` 都是它解析好的结果——同一串 markdown 三端各解析一遍，
/// 谁多认一个 `*` 前缀谁就多出一条，条数对不上。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
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
            // 1e11 以上的数字只可能是毫秒（秒级要到公元 5138 年才有这么大；毫秒从
            // 1973 年起就超过它了）。**门槛与 Android `Usage.kt::parseResetsAt` 必须
            // 是同一个数**：v1.22 前这边写 1e12、那边写 1e11，落在这一段区间的时间戳
            // 两端差一千倍
            ResetsAt::Epoch(n) => {
                let secs = if *n > 1e11 { *n / 1000.0 } else { *n };
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
    /// ISO 时间
    #[serde(default)]
    pub ts: String,
}

// ── v1.35 产物里的 Markdown：GET /sessions/:id/artifacts 的 docs、GET /files/read ──

/// 项目里的一份 Markdown（详情栏「产物」一节里的一行）
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Doc {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    /// 相对项目根的位置（`docs/api.md`）
    #[serde(default)]
    pub rel: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime: f64,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FileBody {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
}

// ── v1.17 详情栏：GET /sessions/:id/detail ──────────────────────────────────

/// 一次子代理调用（Agent / Task 工具）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Subagent {
    /// 工具名：Agent（新）/ Task（老）
    #[serde(default)]
    pub tool: String,
    /// 子代理类型（subagent_type），拿不到就空
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    /// running | ok | err
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub ts: String,
    /// v1.30：派给它的整段任务书（点开看）
    #[serde(default)]
    pub prompt: String,
    /// v1.30：它交回来的报告；还在跑就是空的
    #[serde(default)]
    pub result: String,
}

/// 还没回来的后台任务
#[derive(Debug, Clone, Deserialize, Default)]
pub struct BgTask {
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub ts: String,
    /// v1.30：发起它的那一段原文（命令 / 任务书），点开看
    #[serde(default)]
    pub detail: String,
}

/// 项目 `_inbox/` 里的一个文件（响应里还有 `path`，mac 侧只显示文件名，不收）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UploadInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub ts: String,
}

/// 用过的一个技能（Skill 工具），按名字合并
#[derive(Debug, Clone, Deserialize, Default)]
pub struct McpUse {
    #[serde(default)]
    pub server: String,
    /// 这个服务器下用过的工具（去掉 `mcp__服务器__` 前缀）
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub last_ts: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct SkillUse {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub count: usize,
    #[serde(default)]
    pub last_ts: String,
}

/// 老 daemon（< v1.17）没有这个接口：404 时四样都是空表，面板照画不报错
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionDetailResponse {
    #[serde(default)]
    pub subagents: Vec<Subagent>,
    #[serde(default)]
    pub background_tasks: Vec<BgTask>,
    #[serde(default)]
    pub uploads: Vec<UploadInfo>,
    #[serde(default)]
    pub skills: Vec<SkillUse>,
    /// v1.38：用过的 MCP，按服务器合并（老 daemon 不给 → 空）
    #[serde(default)]
    pub mcp: Vec<McpUse>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ArtifactsResponse {
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// v1.35：项目里的 Markdown（老 daemon 不给这一段 → 空）
    #[serde(default)]
    pub docs: Vec<Doc>,
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

/// 一行项目。**v1.22 起「这个项目此刻是什么样子」整块由 daemon 算好**（PROTOCOL
/// 「版本兼容」）：代表会话、五态、标题、排序时间、登记与否，客户端一个都不再自己推。
/// 此前 mac 取 `updated_at` 最大的会话、Android 先按 agent 过滤再按别的口径排，同一台
/// daemon 在两端显示的标题和状态不一样，连行数都能不一样。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mtime: String,
    #[serde(default)]
    pub dir_size: u64,
    #[serde(default)]
    pub agent: Option<String>,
    /// daemon 从 agent 存储里读出的上一次对话名。标题回退链已经在 daemon 里走完
    /// （见 `title`），这里只剩调试/兼容价值
    #[serde(default)]
    pub session_title: Option<String>,
    /// v1.22：代表这个项目的那个会话。没有活会话时是**最近退出的那个**（点它是 resume，
    /// 不是打开），一个都没有 → None
    #[serde(default)]
    pub session_id: Option<String>,
    /// v1.22：五态之一（PROTOCOL「/projects」那张 5 行表）。没有活会话 = `paused`；老 daemon 不给 → 空串
    #[serde(default)]
    pub status: String,
    /// v1.22：标题回退链（活会话标题 → session_title → 目录名）daemon 走完的结果
    #[serde(default)]
    pub title: Option<String>,
    /// v1.22：排序键 = 该项目最新一条非终端会话的 `updated_at`（含已退出的），
    /// 一个会话都没有 → 目录 mtime
    #[serde(default)]
    pub updated_at: Option<String>,
    /// v1.22：在注册表里。`false` = 在别处 `aaa open` 开出来、注册表里没有但此刻有活
    /// 会话的目录（daemon 自己补的行），只能看，不能 resume / 删项目。
    /// 默认 `true`：老 daemon 的每一行都来自注册表
    #[serde(default = "default_true")]
    pub registered: bool,
}

/// `GET /agents`：有哪些 agent、这台机器装没装（PROTOCOL「Agent 表」）。
/// 客户端不自己硬编码这张表——硬编码就会给一个没装的 agent 开会话，
/// 然后对着一屏 `command not found` 发呆。
#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub available: bool,
}

// `registered` 的默认是 true，derive 出来的 Default 会给 false —— 手写一份，免得
// 「构造一个空 Project」时悄悄变成「没登记的目录」（那会连删除按钮都不给画）。
impl Default for Project {
    fn default() -> Self {
        Project {
            path: String::new(),
            name: String::new(),
            mtime: String::new(),
            dir_size: 0,
            agent: None,
            session_title: None,
            session_id: None,
            status: String::new(),
            title: None,
            updated_at: None,
            registered: true,
        }
    }
}

// ── 其它 REST 响应 ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Health {
    #[serde(default)]
    pub version: String,
    /// v1.22：**客户端唯一的兼容闸门**（PROTOCOL「版本兼容」）。当前 = 2；缺失（老
    /// daemon）→ 0，此时项目状态那几个字段不在，客户端挂降级横幅、**不退回自己算**
    #[serde(default)]
    pub schema: u32,
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
    /// v1.28：这个项目排着的任务变了（加了 / 被喂掉了 / 删了）
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

/// daemon 的 config.toml 是双方的契约，UI 不往里写；窗口布局、未读这类只属于
/// 本机的偏好另起一个文件。老文件里的 `theme` 键（2026-09-10 拿掉主题开关）被忽略。读写失败一律回落默认值——配置坏了也必须能开窗。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
    /// 会话页右侧详情面板是否展开（⌘I 切换；默认展开）
    #[serde(default = "default_true")]
    pub detail_visible: bool,
    /// 系统通知总开关（设置页；2026-09-10 用户拍板「通知只需要总开关，不需要分项目开关」）。
    /// 老文件里的 `muted_projects` 被忽略，不需要迁移。
    #[serde(default = "default_true")]
    pub notify: bool,
    /// 有黄点的项目路径（2026-09-08 用户拍板 Q1(a)：**只存本地**）——这台机器还没进去看过的
    /// 「跑完了 / 在等你回话」。Mac 看过不影响手机上的黄点：黄点说的是「我这台还没看」。
    #[serde(default)]
    pub unread_projects: Vec<String>,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            sidebar_w: SIDEBAR_W_DEFAULT,
            detail_visible: true,
            notify: true,
            unread_projects: Vec::new(),
        }
    }
}

/// 这个项目路径在不在这份名单里：去尾斜杠后精确匹配（同一目录写法不同不该算两个项目；
/// 前缀相同的 `/p/b` 与 `/p/bg` 也不许互相命中）。
/// 只剩未读一处在用（静音 2026-09-10 收成了总开关）。
pub fn path_list_contains(list: &[String], project_path: &str) -> bool {
    let p = project_path.trim_end_matches('/');
    !p.is_empty() && list.iter().any(|m| m.trim_end_matches('/') == p)
}

/// 打黄点 / 清黄点；返回集合是否真的变了（没变就不用落盘）
pub fn set_flagged(flags: &mut Vec<String>, project_path: &str, on: bool) -> bool {
    let p = project_path.trim_end_matches('/');
    if p.is_empty() {
        return false;
    }
    if on {
        if path_list_contains(flags, p) {
            return false;
        }
        flags.push(p.to_string());
    } else {
        if !path_list_contains(flags, p) {
            return false;
        }
        flags.retain(|m| m.trim_end_matches('/') != p);
    }
    true
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


    /// v1.22：清单和「待答的是哪一条」都由 daemon 给，客户端不再解析 `summary`、
    /// 也不再从消息流尾部倒着找 question
    /// 待发送来自会话的 `queued`——**Claude Code 自己的队列**（v1.22 用户拍板：排队发送
    /// 按 claude code 逻辑，AAA 不另做一套）。老 daemon 不下发 → 空表，不是解码失败。
    #[test]
    fn session_carries_the_claude_code_queue() {
        let s: Session = serde_json::from_str(
            r#"{"id":"s1","queued":[{"ts":"2026-09-08T10:00:00Z","text":"跑一遍测试"}]}"#,
        )
        .unwrap();
        assert_eq!(s.queued.len(), 1);
        assert_eq!(s.queued[0].text, "跑一遍测试");
        let old: Session = serde_json::from_str(r#"{"id":"s2"}"#).unwrap();
        assert!(old.queued.is_empty());
    }

    #[test]
    fn session_takes_checklist_and_asking_seq_from_daemon() {
        let s: Session = serde_json::from_str(
            r#"{"id":"s","summary":"- [x] 修好登录页\n- [ ] 补测试\n瞎话",
                "asking_seq":42,
                "checklist":[{"done":true,"text":"修好登录页"},{"done":false,"text":"补测试"}]}"#,
        )
        .unwrap();
        assert_eq!(s.asking_seq, Some(42));
        assert_eq!(s.checklist.len(), 2);
        assert!(s.checklist[0].done && s.checklist[0].text == "修好登录页");
        assert!(!s.checklist[1].done && s.checklist[1].text == "补测试");
        // 老 daemon（schema < 2）两样都不给：空清单 + 没有待答，客户端不去 summary 里补算
        let old: Session =
            serde_json::from_str(r#"{"id":"s","summary":"- [x] 修好登录页"}"#).unwrap();
        assert!(old.checklist.is_empty());
        assert_eq!(old.asking_seq, None);
    }

    /// 五态里唯一还被客户端读的一件事：底色画不画成蓝的
    #[test]
    fn status_running_mirrors_the_protocol_table() {
        assert!(status_running("running") && status_running("background"));
        // asking 不蓝：那是在等你，不是「它还在动」
        assert!(!status_running("asking") && !status_running("active") && !status_running("paused"));
        assert!(!status_running(""));
    }

    /// 老 daemon 的 /health 没有 schema → 0，闸门关上（客户端挂横幅，不退回自己算）
    #[test]
    fn health_schema_gate() {
        let h: Health = serde_json::from_str(r#"{"version":"1.22.0","schema":2}"#).unwrap();
        assert_eq!(h.schema, 2);
        let old: Health = serde_json::from_str(r#"{"version":"1.21.0"}"#).unwrap();
        assert_eq!(old.schema, 0);
    }

    /// v1.22：项目行上「此刻是什么样子」那几样全部来自 daemon
    #[test]
    fn project_carries_daemon_computed_row() {
        let p: Project = serde_json::from_str(
            r#"{"path":"/p/a","name":"a","session_id":"s_1","status":"asking",
                "title":"在等你回话","updated_at":"2026-09-08T12:00:00Z","registered":false}"#,
        )
        .unwrap();
        assert_eq!(p.session_id.as_deref(), Some("s_1"));
        assert_eq!(p.status, "asking");
        assert_eq!(p.title.as_deref(), Some("在等你回话"));
        assert_eq!(p.updated_at.as_deref(), Some("2026-09-08T12:00:00Z"));
        assert!(!p.registered);
        // 老 daemon 的行：状态空串（排最后）、标题/时间为 None（退到 name/mtime），
        // registered 默认 true —— 它那儿每一行都来自注册表
        let old: Project = serde_json::from_str(r#"{"path":"/p/b","name":"b"}"#).unwrap();
        assert!(old.status.is_empty() && old.title.is_none() && old.updated_at.is_none());
        assert!(old.registered && Project::default().registered);
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
        // v1.28 收件箱有入口了，这一帧重新被读：路径对得上才刷新那一节
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"inbox_changed","path":"/p/x"}"#).unwrap();
        assert!(matches!(e, DaemonEvent::InboxChanged { ref path } if path == "/p/x"));
        // 真正不认识的帧仍然落进 Unknown，不能反序列化失败
        let e: DaemonEvent = serde_json::from_str(r#"{"t":"session_stalled","id":"s_1"}"#).unwrap();
        assert!(matches!(e, DaemonEvent::Unknown));
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
    fn artifacts_parse() {
        // file_path 仍在线上，客户端不再收：多出来的键必须被静静吃掉
        let a: ArtifactsResponse = serde_json::from_str(
            r#"{"artifacts":[{"url":"https://claude.ai/a/1","title":"报告","description":"desc","file_path":"/x.html","ts":"2026-09-03T10:00:00Z"}]}"#,
        )
        .unwrap();
        assert_eq!(a.artifacts[0].title, "报告");
    }

    #[test]
    fn path_list_lookup_and_toggle() {
        let mut unread: Vec<String> = vec!["/p/a/".into()];
        // 尾斜杠不影响匹配
        assert!(path_list_contains(&unread, "/p/a"));
        assert!(path_list_contains(&unread, "/p/a/"));
        assert!(!path_list_contains(&unread, "/p/ab"));
        assert!(!path_list_contains(&unread, ""));
        // 返回值是「有没有变」，没变就不用落盘
        assert!(set_flagged(&mut unread, "/p/a", false));
        assert!(unread.is_empty());
        assert!(set_flagged(&mut unread, "/p/b/", true));
        assert_eq!(unread, vec!["/p/b".to_string()]);
        assert!(path_list_contains(&unread, "/p/b"));
        // 空路径不记
        assert!(!set_flagged(&mut unread, "", true));
        assert_eq!(unread.len(), 1);
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
            detail_visible: false,
            notify: false,
            unread_projects: vec!["/p/b".into()],
        };
        let text = toml::to_string(&s).unwrap();
        let back: UiState = toml::from_str(&text).unwrap();
        assert_eq!(back.sidebar_w, 320.0);
        assert!(!back.detail_visible);
        assert!(!back.notify);
        // 缺字段（旧版本写的文件）用默认值补齐，不报错；详情面板默认展开、通知默认开
        let empty: UiState = toml::from_str("").unwrap();
        assert_eq!(empty.sidebar_w, SIDEBAR_W_DEFAULT);
        assert!(empty.detail_visible);
        assert!(empty.notify);
        // 老版本写过 theme / muted_projects（主题 2026-09-10 拿掉、分项目静音同日收成总开关）：
        // 不认识的键忽略，不报错，也不需要迁移
        let old: UiState =
            toml::from_str("sidebar_w = 250.0\ntheme = \"claude\"\nmuted_projects = [\"/p/a\"]\n").unwrap();
        assert_eq!(old.sidebar_w, 250.0);
        assert!(old.notify);
    }
}
