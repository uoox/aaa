//! 协议类型（PROTOCOL.md v1）+ 应用状态。
//! 所有字段尽量 `#[serde(default)]` 宽容解析，daemon 尚在并行开发中。

use serde::{Deserialize, Serialize};

// ── 会话 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    Running,
    Waiting,
    #[default]
    Idle,
    Exited,
}

impl SessionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Running => "running",
            SessionState::Waiting => "waiting",
            SessionState::Idle => "idle",
            SessionState::Exited => "exited",
        }
    }
    /// 排序权重：waiting 一等状态置顶
    pub fn sort_weight(&self) -> u8 {
        match self {
            SessionState::Waiting => 0,
            SessionState::Running => 1,
            SessionState::Idle => 2,
            SessionState::Exited => 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct QuestionOption {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Question {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
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
    #[serde(default)]
    pub question: Option<Question>,
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
}

impl Session {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentInfo {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub cmd: Option<String>,
    #[serde(default)]
    pub resume_cmd: Option<String>,
    #[serde(default = "default_true")]
    pub available: bool,
}

fn default_true() -> bool {
    true
}

/// agent 表兜底（daemon /agents 不可达时新建对话框仍可用），与 PROTOCOL.md 表一致
pub fn builtin_agents() -> Vec<AgentInfo> {
    let mk = |id: &str, label: &str, cmd: &str| AgentInfo {
        id: id.into(),
        label: label.into(),
        cmd: Some(cmd.into()),
        resume_cmd: None,
        available: true,
    };
    vec![
        mk("claude", "Claude", "claude --dangerously-skip-permissions"),
        mk("codex", "Codex", "codex --dangerously-bypass-approvals-and-sandbox"),
        mk("pi", "Pi", "pi"),
        mk("reasonix", "Reasonix", "reasonix --permission-mode bypassPermissions"),
        mk("agy", "Antigravity", "agy --dangerously-skip-permissions"),
        mk("shell", "普通终端", "exec zsh -l"),
    ]
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

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessage {
    pub seq: u64,
    #[serde(default)]
    pub role: String, // user|assistant|tool|system
    #[serde(default)]
    pub kind: String, // text|thinking|tool_use|tool_result|question
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub tool: Option<ToolInfo>,
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
    /// v1.1 watchdog：running 且静默 ≥ stall_minutes
    SessionStalled {
        id: String,
        #[serde(default)]
        quiet_s: u64,
    },
    /// v1.1 消息流：该会话的 agent 存储有新消息（≥500ms 节流）
    MessagesChanged {
        id: String,
        #[serde(default)]
        last_seq: u64,
    },
    /// 未知帧向前兼容（inbox_changed 等本期忽略）
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

/// daemon 的 config.toml 是双方的契约，UI 不往里写；窗口布局这类只属于本机的
/// 偏好另起一个文件。读写失败一律回落默认值——配置坏了也必须能开窗。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "default_sidebar_w")]
    pub sidebar_w: f32,
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            sidebar_w: SIDEBAR_W_DEFAULT,
        }
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
          "question": {
            "text": "原型是否需要包含 iPad 布局？",
            "options": [{"key":"1","label":"需要"},{"key":"2","label":"不需要"}]
          },
          "preview": "…最近 4 行纯文本…",
          "rows": 40, "cols": 120,
          "pid": 12345, "exit_code": null,
          "resume_id": "9f2c81",
          "created_at": "2026-08-30T13:47:00Z", "last_output_at": "2026-08-30T13:51:00Z"
        }"#;
        let s: Session = serde_json::from_str(j).unwrap();
        assert_eq!(s.state, SessionState::Waiting);
        let q = s.question.as_ref().unwrap();
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0].key, "1");
        assert_eq!(s.rows, 40);
        assert_eq!(s.pid, Some(12345));
        assert_eq!(s.exit_code, None);
        assert_eq!(s.display_title(), "aaa-ui 交互原型设计");
    }

    #[test]
    fn session_minimal() {
        // daemon 早期实现可能字段不全，必须能解析
        let s: Session = serde_json::from_str(r#"{"id":"s_1","agent":"shell"}"#).unwrap();
        assert_eq!(s.state, SessionState::Idle);
        assert_eq!(s.display_title(), "zsh");
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
        // v1.1 watchdog 帧
        let e: DaemonEvent =
            serde_json::from_str(r#"{"t":"session_stalled","id":"s_4","quiet_s":612}"#).unwrap();
        assert!(matches!(e, DaemonEvent::SessionStalled { ref id, quiet_s: 612 } if id == "s_4"));
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
        assert!(matches!(e, DaemonEvent::Unknown));
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
        let s = UiState { sidebar_w: 320.0 };
        let text = toml::to_string(&s).unwrap();
        let back: UiState = toml::from_str(&text).unwrap();
        assert_eq!(back.sidebar_w, 320.0);
        // 缺字段（旧版本写的文件）用默认值补齐，不报错
        let empty: UiState = toml::from_str("").unwrap();
        assert_eq!(empty.sidebar_w, SIDEBAR_W_DEFAULT);
    }

    #[test]
    fn waiting_sorts_first() {
        assert!(SessionState::Waiting.sort_weight() < SessionState::Running.sort_weight());
        assert!(SessionState::Running.sort_weight() < SessionState::Idle.sort_weight());
        assert!(SessionState::Idle.sort_weight() < SessionState::Exited.sort_weight());
    }
}
