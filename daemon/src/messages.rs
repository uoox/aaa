//! v1.1 消息流: locate + incrementally tail the agent's own session store and
//! parse it into a structured message list (mobile main view).
//!
//! claude: full support (user/assistant/tool_use/tool_result/thinking, filters
//! isSidechain/isMeta and injected blocks). 本应用只认 Claude Code；终端
//! （shell）`supported:false`。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::paths::Paths;

pub const MAX_MESSAGES: usize = 2000;
const TEXT_CAP: usize = 4000;
const RESULT_CAP: usize = 2000;
const SUMMARY_CAP: usize = 160;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub summary: String,
    pub status: String, // ok | err | running
}

/// One choice of an AskUserQuestion item.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

/// One question of an AskUserQuestion form (Claude Code renders the form as
/// one tab per item plus a Submit tab when it cannot auto-submit).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionItem {
    #[serde(default)]
    pub header: String,
    pub question: String,
    pub options: Vec<QOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// The structured payload of a `kind:"question"` message.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionSpec {
    pub questions: Vec<QuestionItem>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Msg {
    pub seq: u64,
    pub ts: String,
    pub role: String, // user | assistant | tool | system
    pub kind: String, // text | thinking | tool_use | tool_result | question | answer
    pub text: String,
    pub tool: Option<ToolInfo>,
    /// `kind:"question"` only: the form, verbatim from the agent's tool call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<QuestionSpec>,
}

/// 会话里发布过的 Artifact（Claude Code 的 Artifact 工具：报告、原型、图）。按 url 去重，
/// 重复发布同一 url 只更新时间与描述。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    pub title: String,
    pub description: String,
    pub file_path: String,
    pub ts: String,
}

pub struct MsgStore {
    pub source: &'static str, // claude | none
    pub supported: bool,
    pub file: Option<PathBuf>,
    pub offset: u64,
    pub partial: Vec<u8>,
    pub next_seq: u64,
    pub msgs: VecDeque<Msg>,
    /// tool_use id -> tool name (to label tool_results)
    tool_names: HashMap<String, String>,
    /// Artifact 工具调用：tool_use id -> (title, description, file_path)，等 tool_result 里的 url
    pending_artifacts: HashMap<String, (String, String, String)>,
    pub artifacts: Vec<Artifact>,
    /// v1.13「后台」态：tool_use id → 发起时刻。`bg_launch` 是看到了 tool_use
    /// （`run_in_background` / Monitor）还没等到 tool_result 的；`bg_pending` 是
    /// tool_result 确认「在后台跑」了、还没等到 `<task-notification>` 回来的。
    bg_launch: HashMap<String, String>,
    bg_pending: HashMap<String, String>,
    /// 发出去还没等到 tool_result 的**前台**工具调用：有它在就说明模型还在等结果，
    /// 60s 兜底不许把会话压回 waiting（gpt-6 审阅：长编译期间会被误判成静止）
    awaiting_result: HashSet<String>,
    /// 最近一条 assistant 内容 / 工具调用的时间戳：waiting 的会话在这之后又有
    /// 动静 = 被后台任务叫醒了（那不是用户发言，UserPromptSubmit 不会触发）
    pub last_activity_ts: String,
    pub discover_ticks: u32,
    /// 当前 file 来自 resume-id 兜底（旧 transcript）。resume 后 agent 会写
    /// **新**文件；兜底命中的旧文件永不增长，必须保留升级到新文件的机会。
    pub via_fallback: bool,
    /// file 来自 hooks 的 transcript_path（确定事实）：不再做目录扫描式的发现/升级
    pub authoritative: bool,
    pub dirty: bool,
}

impl MsgStore {
    pub fn for_agent(agent: &str) -> Self {
        let (supported, source) = match agent {
            "claude" => (true, "claude"),
            _ => (false, "none"),
        };
        MsgStore {
            source,
            supported,
            file: None,
            offset: 0,
            partial: Vec::new(),
            next_seq: 0,
            msgs: VecDeque::new(),
            tool_names: HashMap::new(),
            discover_ticks: 0,
            via_fallback: false,
            authoritative: false,
            pending_artifacts: HashMap::new(),
            artifacts: Vec::new(),
            bg_launch: HashMap::new(),
            bg_pending: HashMap::new(),
            awaiting_result: HashSet::new(),
            last_activity_ts: String::new(),
            dirty: false,
        }
    }

    /// 还没回来的后台任务数（发起时刻不早于 `since`：resume 带进来的旧 transcript 里
    /// 挂着的任务早随上一个进程死了，不算）
    pub fn pending_background(&self, since: Option<&str>) -> usize {
        self.bg_pending
            .values()
            .filter(|ts| since.is_none_or(|s| ts.is_empty() || ts.as_str() >= s))
            .count()
    }

    /// 前台还有工具调用没等到结果
    pub fn awaiting_tool_result(&self) -> bool {
        !self.awaiting_result.is_empty()
    }

    /// 物理清掉早于 `since` 的挂起项（上一个进程的），以及挂了超过 `max_age` 的：
    /// 通知永远不来（daemon 曾重启、Claude 改了通知形状）也不该让「后台」一直亮着
    pub fn prune_background(&mut self, since: &str, now: &str, max_age: chrono::Duration) {
        let cutoff = chrono::DateTime::parse_from_rfc3339(now).ok().map(|t| t - max_age);
        self.bg_pending.retain(|_, ts| {
            if ts.as_str() < since {
                return false;
            }
            match (cutoff, chrono::DateTime::parse_from_rfc3339(ts)) {
                (Some(c), Ok(t)) => t >= c,
                _ => true,
            }
        });
        self.bg_launch.retain(|_, ts| ts.as_str() >= since);
    }

    fn note_activity(&mut self, ts: &str) {
        if ts > self.last_activity_ts.as_str() {
            self.last_activity_ts = ts.to_string();
        }
    }

    /// 用户侧消息里的 `<task-notification>…<tool-use-id>X</tool-use-id>`：X 的后台任务回来了。
    /// 只认包在 task-notification 里的，别的用户文本里出现这串字不算
    fn settle_background(&mut self, text: &str) {
        if !text.contains("<task-notification") {
            return;
        }
        let mut rest = text;
        while let Some(i) = rest.find("<tool-use-id>") {
            let after = &rest[i + "<tool-use-id>".len()..];
            let Some(j) = after.find("</tool-use-id>") else { break };
            self.bg_pending.remove(after[..j].trim());
            rest = &after[j..];
        }
    }

    pub fn last_seq(&self) -> u64 {
        self.next_seq
    }

    pub fn slice(&self, after: u64, limit: usize) -> Vec<&Msg> {
        self.msgs
            .iter()
            .filter(|m| m.seq > after)
            .take(limit)
            .collect()
    }

    fn push(&mut self, ts: &str, role: &str, kind: &str, text: String, tool: Option<ToolInfo>) {
        self.push_full(ts, role, kind, text, tool, None);
    }

    fn push_full(
        &mut self,
        ts: &str,
        role: &str,
        kind: &str,
        text: String,
        tool: Option<ToolInfo>,
        question: Option<QuestionSpec>,
    ) {
        self.next_seq += 1;
        self.msgs.push_back(Msg {
            seq: self.next_seq,
            ts: ts.to_string(),
            role: role.to_string(),
            kind: kind.to_string(),
            text,
            tool,
            question,
        });
        while self.msgs.len() > MAX_MESSAGES {
            self.msgs.pop_front();
        }
        self.dirty = true;
    }

    /// The form the agent is currently waiting on: the newest question with
    /// no answer after it, and not older than `since` (a resumed transcript
    /// can carry a question the *previous* process never got answered — the
    /// new process shows no dialog for it, so it must not count).
    pub fn pending_question(&self, since: Option<&str>) -> Option<&Msg> {
        for m in self.msgs.iter().rev() {
            match m.kind.as_str() {
                "answer" => return None,
                "question" => {
                    if let Some(since) = since {
                        if !m.ts.is_empty() && m.ts.as_str() < since {
                            return None;
                        }
                    }
                    return Some(m);
                }
                _ => {}
            }
        }
        None
    }
}

const ASK_TOOL: &str = "AskUserQuestion";

/// AskUserQuestion input → structured form. `None` when the shape is off
/// (then it is shown as an ordinary tool call).
fn parse_question_spec(input: Option<&Value>) -> Option<QuestionSpec> {
    let items = input?.get("questions")?.as_array()?;
    let mut questions = Vec::new();
    for it in items {
        let question = it.get("question")?.as_str()?.to_string();
        let options = it
            .get("options")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| {
                        Some(QOption {
                            label: o.get("label")?.as_str()?.to_string(),
                            description: o
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if options.is_empty() {
            return None;
        }
        questions.push(QuestionItem {
            header: it.get("header").and_then(|h| h.as_str()).unwrap_or("").to_string(),
            question,
            options,
            multi_select: it.get("multiSelect").and_then(|b| b.as_bool()).unwrap_or(false),
        });
    }
    if questions.is_empty() { None } else { Some(QuestionSpec { questions }) }
}

/// Human text of an AskUserQuestion result. Prefers the structured
/// `toolUseResult.answers` on the transcript line; falls back to the
/// tool_result content (which claude phrases as `The user answered: …`).
fn answer_text(line: &Value, content: &str) -> String {
    if let Some(answers) = line
        .get("toolUseResult")
        .and_then(|r| r.get("answers"))
        .and_then(|a| a.as_object())
    {
        let parts: Vec<String> = answers
            .iter()
            .map(|(q, a)| {
                let a = a.as_str().unwrap_or("").trim();
                if answers.len() == 1 { a.to_string() } else { format!("{} → {a}", one_line(q)) }
            })
            .collect();
        let joined = parts.join("\n");
        if !joined.trim().is_empty() {
            return joined;
        }
    }
    content.trim().to_string()
}

fn cap(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Injected-content filter (shared with the namer): skip `<…>` blocks, slash
/// commands, JSON blobs and Caveat notes.
pub(crate) fn usable_user_text(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() {
        return false;
    }
    let first = t.chars().next().unwrap();
    !matches!(first, '<' | '{' | '/') && !t.starts_with("Caveat")
}

/// message.content item list or plain string -> concatenated text.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => {
            let mut out = String::new();
            for item in a {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(t);
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// Short human summary of a tool invocation input.
fn summarize_tool_input(name: &str, input: Option<&Value>) -> String {
    let Some(input) = input else { return String::new() };
    let by_key = |k: &str| input.get(k).and_then(|v| v.as_str()).map(String::from);
    let s = match name {
        "Bash" | "BashOutput" => by_key("command"),
        "Read" | "Write" | "Edit" | "NotebookEdit" => by_key("file_path"),
        "Grep" | "Glob" => by_key("pattern"),
        "WebFetch" | "WebSearch" => by_key("url").or_else(|| by_key("query")),
        "Task" => by_key("description"),
        _ => None,
    };
    let s = s.unwrap_or_else(|| {
        // first string field, else compact JSON
        input
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(String::from)))
            .unwrap_or_else(|| input.to_string())
    });
    cap(&one_line(&s), SUMMARY_CAP)
}

// ---------- claude ----------

pub fn parse_claude_line(store: &mut MsgStore, v: &Value) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // 后台任务的 <task-notification> 不一定是 user 消息：模型正在跑的时候它先进队列
    // （`queue-operation` 的 content），再作为 `attachment`（queued_command 的 prompt）
    // 并进这一轮——实测 Bash run_in_background 回来就是这条路，只认 user 消息会漏销
    match ty {
        "queue-operation" => {
            if let Some(c) = v.get("content").and_then(|c| c.as_str()) {
                store.settle_background(c);
            }
            return;
        }
        "attachment" => {
            if let Some(p) = v.pointer("/attachment/prompt").and_then(|c| c.as_str()) {
                store.settle_background(p);
            }
            return;
        }
        _ => {}
    }
    if !matches!(ty, "user" | "assistant") {
        return;
    }
    if v.get("isSidechain").and_then(|b| b.as_bool()).unwrap_or(false) {
        return;
    }
    if v.get("isMeta").and_then(|b| b.as_bool()).unwrap_or(false) {
        return; // injected caveats / companion prompts
    }
    let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    let content = v.get("message").and_then(|m| m.get("content"));
    match ty {
        "user" => match content {
            Some(Value::String(s)) => {
                store.settle_background(s);
                if usable_user_text(s) {
                    store.push(ts, "user", "text", cap(s.trim(), TEXT_CAP), None);
                }
            }
            Some(Value::Array(items)) => {
                for item in items {
                    match item.get("type").and_then(|t| t.as_str()) {
                        Some("tool_result") => {
                            let text = content_text(item.get("content"));
                            let is_err = item
                                .get("is_error")
                                .and_then(|b| b.as_bool())
                                .unwrap_or(false);
                            let name = item
                                .get("tool_use_id")
                                .and_then(|i| i.as_str())
                                .and_then(|id| store.tool_names.get(id).cloned())
                                .unwrap_or_default();
                            let status = if is_err { "err" } else { "ok" };
                            // 结果回来也是 transcript 的动静（长编译 60s 没别的输出，不能算静止）
                            store.note_activity(ts);
                            if let Some(id) = item.get("tool_use_id").and_then(|i| i.as_str()) {
                                store.awaiting_result.remove(id);
                                let launched = store.bg_launch.remove(id).is_some();
                                if !is_err && (launched || looks_like_background_result(&text)) {
                                    store.bg_pending.insert(id.to_string(), ts.to_string());
                                }
                            }
                            if let Some(pending) = item
                                .get("tool_use_id")
                                .and_then(|i| i.as_str())
                                .and_then(|id| store.pending_artifacts.remove(id))
                            {
                                if !is_err {
                                    record_artifact(store, ts, pending, &text);
                                }
                            }
                            if name == ASK_TOOL {
                                // the user's reply to the form: shown on the
                                // user's side, closes the pending question
                                let text = if is_err { text.trim().to_string() } else { answer_text(v, &text) };
                                store.push(
                                    ts,
                                    "user",
                                    "answer",
                                    cap(&text, TEXT_CAP),
                                    Some(ToolInfo {
                                        name,
                                        summary: String::new(),
                                        status: status.to_string(),
                                    }),
                                );
                                continue;
                            }
                            store.push(
                                ts,
                                "tool",
                                "tool_result",
                                cap(text.trim(), RESULT_CAP),
                                Some(ToolInfo {
                                    name,
                                    summary: String::new(),
                                    status: status.to_string(),
                                }),
                            );
                        }
                        Some("text") | None => {
                            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                                store.settle_background(t);
                                if usable_user_text(t) {
                                    store.push(ts, "user", "text", cap(t.trim(), TEXT_CAP), None);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        "assistant" => {
            let Some(Value::Array(items)) = content else { return };
            for item in items {
                match item.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                            if !t.trim().is_empty() {
                                store.note_activity(ts);
                                store.push(ts, "assistant", "text", cap(t.trim(), TEXT_CAP), None);
                            }
                        }
                    }
                    Some("thinking") => {
                        if let Some(t) = item.get("thinking").and_then(|t| t.as_str()) {
                            if !t.trim().is_empty() {
                                store.note_activity(ts);
                                store.push(ts, "assistant", "thinking", cap(t.trim(), TEXT_CAP), None);
                            }
                        }
                    }
                    Some("tool_use") => {
                        let name = item
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string();
                        store.note_activity(ts);
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            store.tool_names.insert(id.to_string(), name.clone());
                            if is_background_launch(&name, item.get("input")) {
                                store.bg_launch.insert(id.to_string(), ts.to_string());
                            } else {
                                store.awaiting_result.insert(id.to_string());
                            }
                            if name == ARTIFACT_TOOL {
                                let inp = item.get("input");
                                let g = |k: &str| inp.and_then(|i| i.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
                                store.pending_artifacts.insert(id.to_string(), (g("title"), g("description"), g("file_path")));
                            }
                        }
                        if name == ASK_TOOL {
                            if let Some(spec) = parse_question_spec(item.get("input")) {
                                let first = &spec.questions[0];
                                let text = first.question.clone();
                                let summary = spec
                                    .questions
                                    .iter()
                                    .map(|q| if q.header.is_empty() { one_line(&q.question) } else { q.header.clone() })
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                                store.push_full(
                                    ts,
                                    "assistant",
                                    "question",
                                    cap(&text, TEXT_CAP),
                                    Some(ToolInfo { name, summary: cap(&summary, SUMMARY_CAP), status: "running".to_string() }),
                                    Some(spec),
                                );
                                continue;
                            }
                        }
                        let summary = summarize_tool_input(&name, item.get("input"));
                        store.push(
                            ts,
                            "tool",
                            "tool_use",
                            String::new(),
                            Some(ToolInfo { name, summary, status: "running".to_string() }),
                        );
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

pub const ARTIFACT_TOOL: &str = "Artifact";

/// 这次工具调用会在后台跑、之后用 `<task-notification>` 回来：Bash / Agent 带
/// `run_in_background:true`，或 Monitor（本身就是挂起等条件）。
fn is_background_launch(name: &str, input: Option<&Value>) -> bool {
    if name == "Monitor" {
        return true;
    }
    input
        .and_then(|i| i.get("run_in_background"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
}

/// tool_result 的文案说它去后台了（没在 tool_use 入参里看出来的兜底，比如
/// 子代理一律异步：「Async agent launched」）
fn looks_like_background_result(text: &str) -> bool {
    text.contains("running in background with ID") || text.contains("Async agent launched")
}

/// 「Published <path> at https://claude.ai/code/artifact/<id>」→ 一条产物记录。
/// 只认 claude.ai 的 artifact 链接；没有 url 的结果（list / read 等动作）忽略。
fn record_artifact(store: &mut MsgStore, ts: &str, pending: (String, String, String), result: &str) {
    let Some(url) = artifact_url(result) else { return };
    let (title, description, file_path) = pending;
    let title = if !title.is_empty() {
        title
    } else if !file_path.is_empty() {
        std::path::Path::new(&file_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.clone())
    } else {
        url.clone()
    };
    let art = Artifact { url: url.clone(), title, description, file_path, ts: ts.to_string() };
    if let Some(existing) = store.artifacts.iter_mut().find(|a| a.url == url) {
        *existing = art;
    } else {
        store.artifacts.push(art);
    }
}

pub fn artifact_url(text: &str) -> Option<String> {
    let start = text.find("https://claude.ai/code/artifact/")?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '>' | '"' | '\'' | ','))
        .unwrap_or(rest.len());
    let url = &rest[..end];
    if url.len() > "https://claude.ai/code/artifact/".len() { Some(url.to_string()) } else { None }
}

fn parse_line(store: &mut MsgStore, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if store.source == "claude" {
        parse_claude_line(store, &v);
    }
}

/// Feed raw bytes read from the store file; handles partial trailing lines.
pub fn ingest(store: &mut MsgStore, bytes: &[u8]) {
    store.partial.extend_from_slice(bytes);
    if store.partial.len() > 2 * 1024 * 1024 {
        store.partial.clear(); // pathological line; resync at next newline
        return;
    }
    while let Some(pos) = store.partial.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = store.partial.drain(..=pos).collect();
        let s = String::from_utf8_lossy(&line[..line.len() - 1]);
        let s = s.trim();
        if !s.is_empty() {
            parse_line(store, s);
        }
    }
}

// ---------- discovery + tailing ----------

fn head_cwd_matches(source: &str, file: &Path, project_path: &str) -> bool {
    let head = crate::stores::jsonl_head(file, 8);
    let cwd = match source {
        "claude" => head
            .iter()
            .find_map(|o| o.get("cwd").and_then(|c| c.as_str()).map(String::from)),
        _ => None,
    };
    match cwd {
        Some(c) => c == project_path || crate::stores::realpath(&c) == project_path,
        None => false,
    }
}

/// Locate the session's store file: resume file when known, else newest file
/// whose head cwd matches and whose mtime is >= session start.
pub fn discover_file(
    paths: &Paths,
    source: &str,
    project_path: &str,
    resume_id: Option<&str>,
    created_epoch: f64,
    claimed: &std::collections::HashSet<PathBuf>,
) -> Option<(PathBuf, bool)> {
    let candidates: Vec<PathBuf> = match source {
        "claude" => {
            let mut v = Vec::new();
            if let Ok(rd) = std::fs::read_dir(paths.claude_root()) {
                for d in rd.flatten() {
                    let dp = d.path();
                    if !dp.is_dir() {
                        continue;
                    }
                    if let Ok(rd2) = std::fs::read_dir(&dp) {
                        for f in rd2.flatten() {
                            let fp = f.path();
                            if fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                v.push(fp);
                            }
                        }
                    }
                }
            }
            v
        }
        _ => return None,
    };

    let mut best: Option<(f64, PathBuf)> = None;
    let mut resume_file: Option<(f64, PathBuf)> = None;
    for f in candidates {
        let (_, mt) = crate::stores::fstat(&f);
        if let Some(rid) = resume_id {
            let stem_match = source == "claude" && f.file_stem().and_then(|s| s.to_str()) == Some(rid);
            if stem_match {
                resume_file = Some((mt, f.clone()));
            }
        }
        if !claimed.contains(&f)
            && mt >= created_epoch - 5.0
            && head_cwd_matches(source, &f, project_path)
            && best.as_ref().map(|b| mt > b.0).unwrap_or(true)
        {
            best = Some((mt, f));
        }
    }
    // (file, is_fallback)：best = 会话自己写的新文件；resume_file = 旧 id 的
    // transcript，只是兜底——它不再增长，调用方要保留换到新文件的机会
    best.map(|(_, f)| (f, false))
        .or(resume_file.map(|(_, f)| (f, true)))
}

/// Incremental tail of the discovered file.
pub fn poll_file(store: &mut MsgStore) {
    use std::io::{Read, Seek, SeekFrom};
    let Some(file) = store.file.clone() else { return };
    let Ok(md) = std::fs::metadata(&file) else { return };
    let len = md.len();
    if len < store.offset {
        // truncated/rotated: re-read from scratch
        store.offset = 0;
        store.partial.clear();
    }
    if len == store.offset {
        return;
    }
    let Ok(mut f) = std::fs::File::open(&file) else { return };
    if f.seek(SeekFrom::Start(store.offset)).is_err() {
        return;
    }
    let mut remaining = (len - store.offset).min(4 * 1024 * 1024);
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        match f.read(&mut buf[..want]) {
            Ok(0) => break,
            Ok(n) => {
                store.offset += n as u64;
                remaining -= n as u64;
                ingest(store, &buf[..n]);
            }
            Err(_) => break,
        }
    }
}

/// v1.13：把 transcript 里的「后台任务」与「被叫醒」镜像到会话状态（每秒轮询后调用）。
/// - `background` = 本进程发起、还没回来的后台任务数（waiting 且 >0 → 客户端标「后台」）。
/// - waiting 的 hooked 会话，在最近一次 Stop 之后 transcript 又出现 assistant 内容 /
///   工具调用 → 它被后台通知叫醒又在跑了（不是用户发言，UserPromptSubmit 不触发）→ running。
///   Stop 时刻 +2s 起算，避免 Stop 之前落盘、daemon 之后才 tail 到的最后一条回答误判。
/// - 这样判出来的 running，Stop 若一直没来，transcript 60s 没动就压回 waiting 兜底。
pub fn mirror_background(sess: &crate::pool::Session) {
    use crate::pool::State;
    let (exited, created_at) = {
        let m = sess.meta.lock().unwrap();
        (m.state == State::Exited, m.created_at)
    };
    if exited {
        return;
    }
    let now = chrono::Utc::now();
    let since = created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    // 锁序约定（全库）：meta 与 msgs 两把锁永远不同时持有——取完就放，再拿另一把
    let (bg, last_act, awaiting) = {
        let mut ms = sess.msgs.lock().unwrap();
        ms.prune_background(&since, &now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true), chrono::Duration::hours(2));
        (ms.pending_background(Some(&since)), ms.last_activity_ts.clone(), ms.awaiting_tool_result())
    };
    // Claude 的时间戳是 JS toISOString（毫秒 Z）；解析后比较，不赌字符串格式
    let last_act = chrono::DateTime::parse_from_rfc3339(&last_act).ok().map(|t| t.with_timezone(&chrono::Utc));
    let mut meta = sess.meta.lock().unwrap();
    let mut dirty = false;
    if meta.background != bg {
        meta.background = bg;
        dirty = true;
    }
    if meta.hooked && meta.state == State::Waiting {
        let floor = meta
            .last_stop_at
            .map(|t| t + chrono::Duration::seconds(2))
            .map_or(created_at, |t| t.max(created_at));
        if last_act.is_some_and(|t| t > floor) {
            meta.state = State::Running;
            meta.running_by_transcript = true;
            meta.touch();
            dirty = true;
        }
    } else if meta.running_by_transcript && meta.state == State::Running && !awaiting {
        // 兜底只在「没有工具调用在等结果」时生效：长编译期间 transcript 本来就没新行
        let idle = match last_act {
            Some(t) => now - t > chrono::Duration::seconds(60),
            None => true, // 解析不了当作没动静：别卡死在 running
        };
        if idle {
            meta.state = State::Waiting;
            meta.running_by_transcript = false;
            meta.touch();
            dirty = true;
        }
    }
    drop(meta);
    if dirty {
        sess.mark_dirty();
    }
}

/// One polling pass for a session; returns Some(last_seq) when new messages
/// arrived (caller emits the throttled messages_changed event).
/// `claimed`：其他会话已认领的存储文件——同目录并发两个同 agent 会话时，
/// 各自按 mtime 认领会互相抢同一份 transcript（审查 P1），已被认领的
/// 文件不再作为候选。
pub fn poll_session(
    paths: &Paths,
    sess: &crate::pool::Session,
    claimed: &std::collections::HashSet<PathBuf>,
) -> Option<u64> {
    let (agent_ok, project_path, resume_id, created_epoch, exited) = {
        let meta = sess.meta.lock().unwrap();
        (
            meta.agent == "claude",
            meta.project_path.clone(),
            meta.resume_id.clone(),
            meta.created_at.timestamp() as f64,
            meta.state == crate::pool::State::Exited,
        )
    };
    if !agent_ok {
        return None;
    }
    let mut store = sess.msgs.lock().unwrap();
    if !store.supported {
        return None;
    }
    if store.authoritative {
        // hooks 已给出 transcript 路径：直接尾随，不发现、不升级
    } else if store.file.is_none() {
        // discovery: every 5th tick; stop after ~5 minutes for exited sessions
        if exited && store.discover_ticks > 300 {
            return None;
        }
        let attempt = store.discover_ticks.is_multiple_of(5);
        store.discover_ticks += 1;
        if !attempt {
            return None;
        }
        match discover_file(
            paths,
            store.source,
            &project_path,
            resume_id.as_deref(),
            created_epoch,
            claimed,
        ) {
            // resume 兜底（旧 transcript）先压 ~30s 再接受：resume 后 agent
            // 很快会写出**新**文件（best 命中），过早锁死旧文件就只剩历史、
            // 永无增量（审查 P0）。30s 内新文件仍没出现才用旧的垫底。
            Some((_, true)) if store.discover_ticks <= 30 => return None,
            Some((f, fb)) => {
                store.file = Some(f);
                store.via_fallback = fb;
            }
            None => return None,
        }
    } else if store.via_fallback {
        // 已在兜底文件上：持续找真正的新文件，出现即升级。旧文件不会再
        // 增长，升级只可能带来新内容；历史消息已按旧文件编号，保留不动。
        store.discover_ticks += 1;
        if store.discover_ticks.is_multiple_of(5) {
            if let Some((f, false)) = discover_file(
                paths,
                store.source,
                &project_path,
                resume_id.as_deref(),
                created_epoch,
                claimed,
            ) {
                if Some(&f) != store.file.as_ref() {
                    store.file = Some(f);
                    store.offset = 0;
                    store.partial.clear();
                    store.via_fallback = false;
                }
            }
        }
    }
    poll_file(&mut store);
    if store.dirty {
        store.dirty = false;
        Some(store.last_seq())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed_lines(store: &mut MsgStore, lines: &[Value]) {
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        ingest(store, body.as_bytes());
    }


    /// v1.13「后台」：后台 Bash / 子代理发起 → pending；<task-notification> 回来 → 销掉；
    /// 早于 since 的（resume 带进来的旧对话）不算
    /// v1.13 被叫醒：waiting 的 hooked 会话在最近一次 Stop 之后 transcript 又有 assistant
    /// 动静 → running；Stop 前落盘、之后才 tail 到的那条不算（+2s 缓冲）；后台数镜像到 meta
    #[test]
    fn mirror_background_wakes_a_waiting_session_after_the_last_stop() {
        use crate::pool::State;
        let sess = crate::pool::Session::for_test("claude", "/p");
        let created = chrono::Utc::now() - chrono::Duration::seconds(30);
        {
            let mut m = sess.meta.lock().unwrap();
            m.hooked = true;
            m.state = State::Waiting;
            m.created_at = created;
            m.last_stop_at = Some(chrono::Utc::now() - chrono::Duration::seconds(10));
        }
        let iso = |secs_ago: i64| (chrono::Utc::now() - chrono::Duration::seconds(secs_ago)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        // 1) Stop 之前落盘的最后一条回答（比 Stop 早 3s）：不算叫醒
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]},"timestamp":iso(13)}));
        mirror_background(&sess);
        assert_eq!(sess.meta.lock().unwrap().state, State::Waiting);
        // 2) 后台任务发起并确认：waiting 且 background>0
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"x","run_in_background":true}}]},"timestamp":iso(12)}));
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"Command running in background with ID: b1"}]},"timestamp":iso(12)}));
        mirror_background(&sess);
        {
            let m = sess.meta.lock().unwrap();
            assert_eq!(m.state, State::Waiting, "发起时刻还在 Stop 之前，仍是 waiting");
            assert_eq!(m.background, 1);
        }
        // 3) 通知回来、模型开跑（Stop 之后 5s 有新 assistant 内容）→ running，后台数归零
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"user","message":{"role":"user","content":"<task-notification><tool-use-id>tu1</tool-use-id></task-notification>"},"timestamp":iso(5)}));
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"结果来了"}]},"timestamp":iso(5)}));
        mirror_background(&sess);
        {
            let m = sess.meta.lock().unwrap();
            assert_eq!(m.state, State::Running);
            assert!(m.running_by_transcript);
            assert_eq!(m.background, 0);
        }
        // 4) 前台发了个长工具调用（编译），61s 没有新行但结果还没回来：**不能**压回 waiting
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_slow","name":"Bash","input":{"command":"cargo build"}}]},"timestamp":iso(61)}));
        sess.msgs.lock().unwrap().last_activity_ts = iso(61);
        mirror_background(&sess);
        assert_eq!(sess.meta.lock().unwrap().state, State::Running, "还在等 tool_result，不算静止");
        // 结果回来了：本身算动静（时间戳新），仍是 running
        parse_claude_line(&mut sess.msgs.lock().unwrap(), &json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_slow","content":"ok"}]},"timestamp":iso(1)}));
        mirror_background(&sess);
        assert_eq!(sess.meta.lock().unwrap().state, State::Running);
        // 5) 之后 Stop 一直不来、transcript 61s 没动、也没有在等的工具：兜底压回 waiting
        sess.msgs.lock().unwrap().last_activity_ts = iso(61);
        mirror_background(&sess);
        assert_eq!(sess.meta.lock().unwrap().state, State::Waiting);
        // 6) 已退出的什么都不动
        sess.meta.lock().unwrap().state = State::Exited;
        sess.msgs.lock().unwrap().last_activity_ts = iso(0);
        mirror_background(&sess);
        assert_eq!(sess.meta.lock().unwrap().state, State::Exited);
    }

    #[test]
    fn background_tasks_are_tracked_until_their_notification_returns() {
        let mut store = MsgStore::for_agent("claude");
        let lines = [
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_bg","name":"Bash","input":{"command":"cargo build","run_in_background":true}}]},"timestamp":"2026-09-07T01:00:00.000Z"}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_bg","content":"Command running in background with ID: b1"}]},"timestamp":"2026-09-07T01:00:01.000Z"}),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_ag","name":"Agent","input":{"prompt":"review"}}]},"timestamp":"2026-09-07T01:00:02.000Z"}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_ag","content":[{"type":"text","text":"Async agent launched successfully. agentId: a9"}]}]},"timestamp":"2026-09-07T01:00:03.000Z"}),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_fg","name":"Bash","input":{"command":"ls"}}]},"timestamp":"2026-09-07T01:00:04.000Z"}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_fg","content":"a b"}]},"timestamp":"2026-09-07T01:00:05.000Z"}),
        ];
        for l in &lines {
            parse_claude_line(&mut store, l);
        }
        assert_eq!(store.pending_background(None), 2, "后台 Bash + 异步子代理挂着，前台 ls 不算");
        assert_eq!(store.last_activity_ts, "2026-09-07T01:00:05.000Z", "tool_result 回来也算动静");
        // 通知回来（user 字符串消息，带 tool-use-id）
        parse_claude_line(&mut store, &json!({"type":"user","message":{"role":"user","content":"<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>tu_bg</tool-use-id>\n<status>completed</status>\n</task-notification>"},"timestamp":"2026-09-07T01:05:00.000Z"}));
        assert_eq!(store.pending_background(None), 1);
        // 早于 since 的不算（上一个进程发起的，进程一死任务就没了）
        assert_eq!(store.pending_background(Some("2026-09-07T01:00:02.500Z")), 1);
        assert_eq!(store.pending_background(Some("2026-09-07T01:00:03.500Z")), 0);
        // 通知不进消息流（'<' 开头的用户文本本来就滤掉）
        assert!(store.msgs.iter().all(|m| !m.text.contains("task-notification")));
        // 模型正在跑时，通知走 queue-operation / attachment 而不是 user 消息（实测）
        parse_claude_line(&mut store, &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_q","name":"Bash","input":{"command":"sleep 5","run_in_background":true}}]},"timestamp":"2026-09-07T01:05:10.000Z"}));
        parse_claude_line(&mut store, &json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_q","content":"Command running in background with ID: q1"}]},"timestamp":"2026-09-07T01:05:11.000Z"}));
        assert_eq!(store.pending_background(None), 2);
        parse_claude_line(&mut store, &json!({"type":"queue-operation","operation":"enqueue","timestamp":"2026-09-07T01:05:20.000Z","content":"<task-notification>\n<task-id>q1</task-id>\n<tool-use-id>tu_q</tool-use-id>\n<status>completed</status>\n</task-notification>"}));
        assert_eq!(store.pending_background(None), 1, "queue-operation 也能销掉");
        parse_claude_line(&mut store, &json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"tu_a","name":"Bash","input":{"command":"x","run_in_background":true}}]},"timestamp":"2026-09-07T01:05:30.000Z"}));
        parse_claude_line(&mut store, &json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_a","content":"Command running in background with ID: a1"}]},"timestamp":"2026-09-07T01:05:31.000Z"}));
        parse_claude_line(&mut store, &json!({"type":"attachment","attachment":{"type":"queued_command","commandMode":"task-notification","prompt":"<task-notification>\n<task-id>a1</task-id>\n<tool-use-id>tu_a</tool-use-id>\n</task-notification>"},"timestamp":"2026-09-07T01:05:40.000Z"}));
        assert_eq!(store.pending_background(None), 1, "attachment 也能销掉");
        // 不在 task-notification 里的 <tool-use-id> 不算回来
        parse_claude_line(&mut store, &json!({"type":"user","message":{"role":"user","content":"帮我看看 <tool-use-id>tu_ag</tool-use-id> 是什么"},"timestamp":"2026-09-07T01:06:00.000Z"}));
        assert_eq!(store.pending_background(None), 1);
        // 挂超过 max_age 的物理清掉
        store.prune_background("2026-09-07T00:00:00.000Z", "2026-09-07T04:00:00.000Z", chrono::Duration::hours(2));
        assert_eq!(store.pending_background(None), 0);
    }

    #[test]
    fn claude_full_parse_with_filters() {
        let mut store = MsgStore::for_agent("claude");
        feed_lines(
            &mut store,
            &[
                json!({"type":"file-history-snapshot","messageId":"x"}),
                json!({"type":"user","isMeta":true,"message":{"role":"user","content":"<local-command-caveat>Caveat: ..."},"timestamp":"T0"}),
                json!({"type":"user","message":{"role":"user","content":"帮我实现 daemon"},"timestamp":"T1"}),
                json!({"type":"user","isSidechain":true,"message":{"role":"user","content":"sidechain prompt"},"timestamp":"T2"}),
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"let me think"}]},"timestamp":"T3"}),
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"我先看看脚本。"}]},"timestamp":"T4"}),
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo build","description":"build"}}]},"timestamp":"T5"}),
                json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"Finished dev profile","is_error":false}]},"timestamp":"T6"}),
                json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":[{"type":"text","text":"boom"}],"is_error":true}]},"timestamp":"T7"}),
                json!({"type":"user","message":{"role":"user","content":"/compact"},"timestamp":"T8"}),
            ],
        );
        let msgs: Vec<&Msg> = store.slice(0, 100);
        assert_eq!(msgs.len(), 6);
        assert_eq!((msgs[0].role.as_str(), msgs[0].kind.as_str(), msgs[0].text.as_str()), ("user", "text", "帮我实现 daemon"));
        assert_eq!(msgs[1].kind, "thinking");
        assert_eq!(msgs[2].text, "我先看看脚本。");
        let tu = msgs[3];
        assert_eq!(tu.kind, "tool_use");
        let tool = tu.tool.as_ref().unwrap();
        assert_eq!(tool.name, "Bash");
        assert_eq!(tool.summary, "cargo build");
        assert_eq!(tool.status, "running");
        let tr = msgs[4];
        assert_eq!(tr.kind, "tool_result");
        assert_eq!(tr.text, "Finished dev profile");
        assert_eq!(tr.tool.as_ref().unwrap().status, "ok");
        assert_eq!(tr.tool.as_ref().unwrap().name, "Bash", "result labelled via tool_use id");
        let te = msgs[5];
        assert_eq!(te.tool.as_ref().unwrap().status, "err");
        assert_eq!(te.text, "boom");
        // seq/after slicing
        assert_eq!(store.last_seq(), 6);
        let tail = store.slice(4, 100);
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].seq, 5);
        assert_eq!(store.slice(0, 2).len(), 2);
    }

    #[test]
    fn claude_ask_user_question_becomes_a_structured_question_and_an_answer() {
        let mut store = MsgStore::for_agent("claude");
        feed_lines(
            &mut store,
            &[
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_q","name":"AskUserQuestion","input":{"questions":[
                    {"question":"Pick a color","header":"Color","options":[{"label":"Red","description":"warm"},{"label":"Green","description":"natural"}],"multiSelect":false},
                    {"question":"Pick fruits","header":"Fruits","options":[{"label":"Apple","description":""},{"label":"Cherry","description":""}],"multiSelect":true}
                ]}}]},"timestamp":"2026-09-02T10:00:00.000Z"}),
            ],
        );
        let q = store.msgs.back().unwrap();
        assert_eq!((q.role.as_str(), q.kind.as_str()), ("assistant", "question"));
        assert_eq!(q.text, "Pick a color");
        let spec = q.question.as_ref().expect("structured form attached");
        assert_eq!(spec.questions.len(), 2);
        assert_eq!(spec.questions[0].options[1].label, "Green");
        assert!(spec.questions[1].multi_select);
        assert_eq!(q.tool.as_ref().unwrap().summary, "Color · Fruits");
        // pending until the result lands; a `since` after it hides it (resume case)
        assert!(store.pending_question(None).is_some());
        assert!(store.pending_question(Some("2026-09-02T09:00:00.000Z")).is_some());
        assert!(store.pending_question(Some("2026-09-02T10:30:00.000Z")).is_none(), "older than this process");

        feed_lines(
            &mut store,
            &[json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_q","content":"The user answered: …"}]},
                "toolUseResult":{"answers":{"Pick a color":"Green","Pick fruits":"Apple, Cherry"}},"timestamp":"2026-09-02T10:01:00.000Z"})],
        );
        let a = store.msgs.back().unwrap();
        assert_eq!((a.role.as_str(), a.kind.as_str()), ("user", "answer"));
        assert!(a.text.contains("Pick a color → Green"), "{}", a.text);
        assert!(a.text.contains("Pick fruits → Apple, Cherry"), "{}", a.text);
        assert_eq!(a.tool.as_ref().unwrap().status, "ok");
        assert!(store.pending_question(None).is_none(), "answered");
        // wire shape: question only present on question messages
        let j = serde_json::to_value(store.msgs.front().unwrap()).unwrap();
        assert!(j.get("question").is_some());
        let j = serde_json::to_value(store.msgs.back().unwrap()).unwrap();
        assert!(j.get("question").is_none());
    }

    #[test]
    fn single_question_answer_is_just_the_answer() {
        let mut store = MsgStore::for_agent("claude");
        feed_lines(
            &mut store,
            &[
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"AskUserQuestion","input":{"questions":[{"question":"Name?","header":"Name","options":[{"label":"A"},{"label":"B"}]}]}}]},"timestamp":"T1"}),
                json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x"}]},"toolUseResult":{"answers":{"Name?":"Zed"}},"timestamp":"T2"}),
            ],
        );
        assert_eq!(store.msgs.back().unwrap().text, "Zed");
        // malformed form (no options) stays an ordinary tool call
        feed_lines(
            &mut store,
            &[json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_2","name":"AskUserQuestion","input":{"questions":[{"question":"?"}]}}]},"timestamp":"T3"})],
        );
        assert_eq!(store.msgs.back().unwrap().kind, "tool_use");
        assert!(store.pending_question(None).is_none());
    }

    #[test]
    fn claude_partial_line_across_chunks() {
        let mut store = MsgStore::for_agent("claude");
        let line = json!({"type":"user","message":{"role":"user","content":"跨块消息"},"timestamp":"T"}).to_string() + "\n";
        let bytes = line.as_bytes();
        let (a, b) = bytes.split_at(bytes.len() / 2);
        ingest(&mut store, a);
        assert_eq!(store.last_seq(), 0, "half a line parses nothing");
        ingest(&mut store, b);
        assert_eq!(store.last_seq(), 1);
        assert_eq!(store.msgs[0].text, "跨块消息");
    }


    #[test]
    fn only_claude_is_supported() {
        assert!(MsgStore::for_agent("claude").supported);
        for other in ["shell", "codex", "pi", "reasonix", "agy", "grok"] {
            let st = MsgStore::for_agent(other);
            assert!(!st.supported, "{other} 不再解析");
            assert_eq!(st.source, "none");
        }
    }

    #[test]
    fn discovery_and_tail_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let proj_s = crate::stores::realpath(&proj.to_string_lossy());
        let f = paths.claude_root().join("p1").join("abc-123.jsonl");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(
            &f,
            format!(
                "{}\n{}\n",
                json!({"type":"user","cwd":proj_s,"message":{"role":"user","content":"first"},"timestamp":"T1"}),
                json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"reply"}]},"timestamp":"T2"}),
            ),
        )
        .unwrap();
        // mtime is "now"; created_epoch slightly in the past
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
            - 10.0;
        let (found, fb) = discover_file(&paths, "claude", &proj_s, None, created, &Default::default()).unwrap();
        assert_eq!(found, f);
        assert!(!fb, "cwd+mtime 命中不是兜底");
        // by resume id even when mtime predates the session —— 但要标成兜底
        let (found2, fb2) =
            discover_file(&paths, "claude", "/other", Some("abc-123"), created + 1e9, &Default::default()).unwrap();
        assert_eq!(found2, f);
        assert!(fb2, "resume-id 命中是兜底，调用方要保留升级机会");
        // tail incrementally
        let mut store = MsgStore::for_agent("claude");
        store.file = Some(f.clone());
        poll_file(&mut store);
        assert_eq!(store.last_seq(), 2);
        // append and re-poll
        use std::io::Write;
        let mut fh = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        writeln!(
            fh,
            "{}",
            json!({"type":"user","message":{"role":"user","content":"second"},"timestamp":"T3"})
        )
        .unwrap();
        drop(fh);
        poll_file(&mut store);
        assert_eq!(store.last_seq(), 3);
        assert_eq!(store.msgs[2].text, "second");
    }

    #[test]
    fn artifact_tool_calls_become_artifact_records() {
        let mut store = MsgStore::for_agent("claude");
        let use_line = serde_json::json!({"type":"assistant","timestamp":"2026-09-03T10:00:00Z","message":{"content":[
            {"type":"tool_use","id":"tu1","name":"Artifact","input":{"file_path":"/p/报告.html","description":"季度报告","favicon":"📊"}}]}});
        parse_claude_line(&mut store, &use_line);
        assert!(store.artifacts.is_empty(), "url comes with the result");
        let res_line = serde_json::json!({"type":"user","timestamp":"2026-09-03T10:00:05Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"tu1","content":"Published /p/报告.html at https://claude.ai/code/artifact/abc-123\n\nLive subscription: skipped"}]}});
        parse_claude_line(&mut store, &res_line);
        assert_eq!(store.artifacts.len(), 1);
        let a = &store.artifacts[0];
        assert_eq!(a.url, "https://claude.ai/code/artifact/abc-123");
        assert_eq!(a.title, "报告", "no title → file stem");
        assert_eq!(a.description, "季度报告");
        // republish of the same url replaces, does not duplicate
        let use2 = serde_json::json!({"type":"assistant","timestamp":"2026-09-03T11:00:00Z","message":{"content":[
            {"type":"tool_use","id":"tu2","name":"Artifact","input":{"file_path":"/p/报告.html","title":"Q3 报告"}}]}});
        let res2 = serde_json::json!({"type":"user","timestamp":"2026-09-03T11:00:01Z","message":{"content":[
            {"type":"tool_result","tool_use_id":"tu2","content":[{"type":"text","text":"Published /p/报告.html at https://claude.ai/code/artifact/abc-123 (redeployed)"}]}]}});
        parse_claude_line(&mut store, &use2);
        parse_claude_line(&mut store, &res2);
        assert_eq!(store.artifacts.len(), 1);
        assert_eq!(store.artifacts[0].title, "Q3 报告");
        assert_eq!(store.artifacts[0].ts, "2026-09-03T11:00:01Z");
        // errors and non-publish actions record nothing
        let use3 = serde_json::json!({"type":"assistant","timestamp":"t","message":{"content":[
            {"type":"tool_use","id":"tu3","name":"Artifact","input":{"action":"list"}}]}});
        let res3 = serde_json::json!({"type":"user","timestamp":"t","message":{"content":[
            {"type":"tool_result","tool_use_id":"tu3","content":"3 artifacts: ..."}]}});
        parse_claude_line(&mut store, &use3);
        parse_claude_line(&mut store, &res3);
        assert_eq!(store.artifacts.len(), 1);
        assert_eq!(artifact_url("see https://claude.ai/code/artifact/x-1, ok"), Some("https://claude.ai/code/artifact/x-1".into()));
        assert_eq!(artifact_url("https://claude.ai/code/artifact/"), None);
    }
}
