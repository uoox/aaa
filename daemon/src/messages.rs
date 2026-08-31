//! v1.1 消息流: locate + incrementally tail the agent's own session store and
//! parse it into a structured message list (mobile main view).
//!
//! claude: full support (user/assistant/tool_use/tool_result/thinking, filters
//! isSidechain/isMeta and injected blocks). codex/pi: best effort.
//! reasonix/agy/shell: `supported:false`.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::Serialize;
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

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Msg {
    pub seq: u64,
    pub ts: String,
    pub role: String, // user | assistant | tool | system
    pub kind: String, // text | thinking | tool_use | tool_result | question
    pub text: String,
    pub tool: Option<ToolInfo>,
}

pub struct MsgStore {
    pub source: &'static str, // claude | codex | pi | none
    pub supported: bool,
    pub file: Option<PathBuf>,
    pub offset: u64,
    pub partial: Vec<u8>,
    pub next_seq: u64,
    pub msgs: VecDeque<Msg>,
    /// tool_use id -> tool name (to label tool_results)
    tool_names: HashMap<String, String>,
    pub discover_ticks: u32,
    pub dirty: bool,
}

impl MsgStore {
    pub fn for_agent(agent: &str) -> Self {
        let (supported, source) = match agent {
            "claude" => (true, "claude"),
            "codex" => (true, "codex"),
            "pi" => (true, "pi"),
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
        self.next_seq += 1;
        self.msgs.push_back(Msg {
            seq: self.next_seq,
            ts: ts.to_string(),
            role: role.to_string(),
            kind: kind.to_string(),
            text,
            tool,
        });
        while self.msgs.len() > MAX_MESSAGES {
            self.msgs.pop_front();
        }
        self.dirty = true;
    }
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

// ---------- codex (best effort, rollout format) ----------

pub fn parse_codex_line(store: &mut MsgStore, v: &Value) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let ts = v.get("timestamp").and_then(|t| t.as_str()).unwrap_or("");
    let Some(p) = v.get("payload") else { return };
    let pty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match (ty, pty) {
        // user/assistant text comes through event_msg (response_item messages
        // of role developer/system are injected context — skip them)
        ("event_msg", "user_message") => {
            if let Some(m) = p.get("message").and_then(|m| m.as_str()) {
                if usable_user_text(m) {
                    store.push(ts, "user", "text", cap(m.trim(), TEXT_CAP), None);
                }
            }
        }
        ("event_msg", "agent_message") => {
            if let Some(m) = p.get("message").and_then(|m| m.as_str()) {
                if !m.trim().is_empty() {
                    store.push(ts, "assistant", "text", cap(m.trim(), TEXT_CAP), None);
                }
            }
        }
        ("response_item", "reasoning") => {
            let mut text = content_text(p.get("content"));
            if text.is_empty() {
                text = content_text(p.get("summary"));
            }
            if !text.trim().is_empty() {
                store.push(ts, "assistant", "thinking", cap(text.trim(), TEXT_CAP), None);
            }
        }
        ("response_item", "function_call") => {
            let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            if let Some(id) = p.get("call_id").and_then(|i| i.as_str()) {
                store.tool_names.insert(id.to_string(), name.clone());
            }
            // arguments is a JSON string; prefer cmd/command fields
            let args_raw = p.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
            let summary = serde_json::from_str::<Value>(args_raw)
                .ok()
                .and_then(|a| {
                    let key = a.get("cmd").or_else(|| a.get("command"))?;
                    match key {
                        Value::String(s) => Some(s.clone()),
                        Value::Array(items) => Some(
                            items
                                .iter()
                                .filter_map(|i| i.as_str())
                                .collect::<Vec<_>>()
                                .join(" "),
                        ),
                        _ => None,
                    }
                })
                .unwrap_or_else(|| args_raw.to_string());
            store.push(
                ts,
                "tool",
                "tool_use",
                String::new(),
                Some(ToolInfo {
                    name,
                    summary: cap(&one_line(&summary), SUMMARY_CAP),
                    status: "running".to_string(),
                }),
            );
        }
        ("response_item", "function_call_output") => {
            let out = p.get("output").and_then(|o| o.as_str()).unwrap_or("").to_string();
            let name = p
                .get("call_id")
                .and_then(|i| i.as_str())
                .and_then(|id| store.tool_names.get(id).cloned())
                .unwrap_or_default();
            // best-effort status from "exited with code N"
            let status = out
                .find("exited with code ")
                .and_then(|i| {
                    out["exited with code ".len() + i..]
                        .split_whitespace()
                        .next()
                        .and_then(|c| c.trim_matches(|ch: char| !ch.is_ascii_digit()).parse::<i64>().ok())
                })
                .map(|c| if c == 0 { "ok" } else { "err" })
                .unwrap_or("ok");
            store.push(
                ts,
                "tool",
                "tool_result",
                cap(out.trim(), RESULT_CAP),
                Some(ToolInfo { name, summary: String::new(), status: status.to_string() }),
            );
        }
        _ => {}
    }
}

// ---------- pi (best effort) ----------

pub fn parse_pi_line(store: &mut MsgStore, v: &Value) {
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let ts = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    // shapes seen in the wild: {"type":"message","message":{"role","content"}}
    // or claude-like {"type":"user"/"assistant","message":{...}}
    let msg = v.get("message");
    let role = msg
        .and_then(|m| m.get("role"))
        .and_then(|r| r.as_str())
        .unwrap_or(match ty {
            "user" => "user",
            "assistant" => "assistant",
            _ => "",
        });
    if !matches!(role, "user" | "assistant") {
        return;
    }
    if !matches!(ty, "message" | "user" | "assistant") {
        return;
    }
    let text = content_text(msg.and_then(|m| m.get("content")));
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if role == "user" && !usable_user_text(text) {
        return;
    }
    store.push(ts, role, "text", cap(text, TEXT_CAP), None);
}

fn parse_line(store: &mut MsgStore, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    match store.source {
        "claude" => parse_claude_line(store, &v),
        "codex" => parse_codex_line(store, &v),
        "pi" => parse_pi_line(store, &v),
        _ => {}
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
        "codex" => head.iter().find_map(|o| {
            (o.get("type").and_then(|t| t.as_str()) == Some("session_meta"))
                .then(|| o.get("payload")?.get("cwd")?.as_str().map(String::from))
                .flatten()
        }),
        "pi" => head.iter().find_map(|o| {
            (o.get("type").and_then(|t| t.as_str()) == Some("session"))
                .then(|| o.get("cwd")?.as_str().map(String::from))
                .flatten()
        }),
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
) -> Option<PathBuf> {
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
        "codex" => {
            let mut v = Vec::new();
            let root = paths.codex_root();
            if let Ok(y) = std::fs::read_dir(&root) {
                for y in y.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                    if let Ok(m) = std::fs::read_dir(&y) {
                        for m in m.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                            if let Ok(d) = std::fs::read_dir(&m) {
                                for d in d.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                                    if let Ok(f) = std::fs::read_dir(&d) {
                                        for f in f.flatten().map(|e| e.path()) {
                                            let n = f
                                                .file_name()
                                                .and_then(|n| n.to_str())
                                                .unwrap_or("");
                                            if n.starts_with("rollout-") && n.ends_with(".jsonl") {
                                                v.push(f);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            v
        }
        "pi" => {
            let mut v = Vec::new();
            if let Ok(rd) = std::fs::read_dir(paths.pi_root()) {
                for d in rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                    if let Ok(rd2) = std::fs::read_dir(&d) {
                        for f in rd2.flatten().map(|e| e.path()) {
                            if f.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                v.push(f);
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
            let stem_match = match source {
                "claude" => f.file_stem().and_then(|s| s.to_str()) == Some(rid),
                _ => false,
            };
            if stem_match {
                resume_file = Some((mt, f.clone()));
            }
        }
        if mt >= created_epoch - 5.0
            && head_cwd_matches(source, &f, project_path)
            && best.as_ref().map(|b| mt > b.0).unwrap_or(true)
        {
            best = Some((mt, f));
        }
    }
    best.or(resume_file).map(|(_, f)| f)
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
pub fn poll_session(paths: &Paths, sess: &crate::pool::Session) -> Option<u64> {
    let (agent_ok, project_path, resume_id, created_epoch, exited) = {
        let meta = sess.meta.lock().unwrap();
        (
            matches!(meta.agent.as_str(), "claude" | "codex" | "pi"),
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
        store.file = discover_file(
            paths,
            store.source,
            &project_path,
            resume_id.as_deref(),
            created_epoch,
        );
        store.file.as_ref()?;
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
    fn codex_best_effort_parse() {
        let mut store = MsgStore::for_agent("codex");
        feed_lines(
            &mut store,
            &[
                json!({"timestamp":"T0","type":"session_meta","payload":{"id":"x","cwd":"/p"}}),
                json!({"timestamp":"T1","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<app-context>injected</app-context>"}]}}),
                json!({"timestamp":"T2","type":"event_msg","payload":{"type":"user_message","message":"卸载 brew-browser\n"}}),
                json!({"timestamp":"T3","type":"response_item","payload":{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"**Planning**"}]}}),
                json!({"timestamp":"T4","type":"event_msg","payload":{"type":"agent_message","message":"我先定位安装位置。"}}),
                json!({"timestamp":"T5","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"brew list\"}","call_id":"c1"}}),
                json!({"timestamp":"T6","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Wall time: 0.6\nProcess exited with code 1\n"}}),
            ],
        );
        let msgs: Vec<&Msg> = store.slice(0, 100);
        assert_eq!(msgs.len(), 5, "developer injection filtered: {msgs:?}");
        assert_eq!((msgs[0].role.as_str(), msgs[0].text.as_str()), ("user", "卸载 brew-browser"));
        assert_eq!(msgs[1].kind, "thinking");
        assert_eq!(msgs[2].role, "assistant");
        assert_eq!(msgs[3].tool.as_ref().unwrap().summary, "brew list");
        assert_eq!(msgs[3].tool.as_ref().unwrap().name, "exec_command");
        assert_eq!(msgs[4].tool.as_ref().unwrap().status, "err");
        assert_eq!(msgs[4].tool.as_ref().unwrap().name, "exec_command");
    }

    #[test]
    fn pi_best_effort_and_unknown_agents() {
        let mut store = MsgStore::for_agent("pi");
        assert!(store.supported);
        feed_lines(
            &mut store,
            &[
                json!({"type":"session","id":"s","cwd":"/p"}),
                json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":"你好"}]},"timestamp":"T"}),
                json!({"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"回复"}]}}),
                json!({"type":"weird","payload":{}}),
            ],
        );
        assert_eq!(store.last_seq(), 2);
        let none = MsgStore::for_agent("shell");
        assert!(!none.supported);
        assert_eq!(none.source, "none");
        assert!(!MsgStore::for_agent("reasonix").supported);
        assert!(!MsgStore::for_agent("agy").supported);
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
        let found = discover_file(&paths, "claude", &proj_s, None, created).unwrap();
        assert_eq!(found, f);
        // by resume id even when mtime predates the session
        let found2 = discover_file(&paths, "claude", "/other", Some("abc-123"), created + 1e9);
        assert_eq!(found2.unwrap(), f);
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
