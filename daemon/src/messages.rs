//! v1.1 消息流: locate + incrementally tail the agent's own session store and
//! parse it into a structured message list (mobile main view).
//!
//! claude: full support (user/assistant/tool_use/tool_result/thinking, filters
//! isSidechain/isMeta and injected blocks). 本应用只认 Claude Code；终端
//! （shell）`supported:false`。

use std::collections::{HashMap, VecDeque};
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
    pub discover_ticks: u32,
    /// 当前 file 来自 resume-id 兜底（旧 transcript）。resume 后 agent 会写
    /// **新**文件；兜底命中的旧文件永不增长，必须保留升级到新文件的机会。
    pub via_fallback: bool,
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
            dirty: false,
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
                                store.push(ts, "assistant", "text", cap(t.trim(), TEXT_CAP), None);
                            }
                        }
                    }
                    Some("thinking") => {
                        if let Some(t) = item.get("thinking").and_then(|t| t.as_str()) {
                            if !t.trim().is_empty() {
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
                        if let Some(id) = item.get("id").and_then(|i| i.as_str()) {
                            store.tool_names.insert(id.to_string(), name.clone());
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
    if store.file.is_none() {
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
}
