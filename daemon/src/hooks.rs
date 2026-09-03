//! Claude Code hooks as the daemon's event source.
//!
//! Every claude session is started with `--settings <state>/claude-hooks.json`,
//! a settings fragment that registers HTTP hooks pointing back at this daemon
//! (`POST /api/v1/hooks/<event>`). Claude Code merges it with the user's own
//! settings, so nothing of theirs is touched. The session is identified by the
//! `X-AAA-Session` header, filled from the `AAA_SESSION` environment variable
//! the PTY was spawned with (hooks substitute `$VAR` in headers when the name
//! is listed in `allowedEnvVars`).
//!
//! What the events replace:
//! - `UserPromptSubmit` / `Stop` / `StopFailure` / `Notification(idle_prompt)`
//!   drive `running ↔ waiting` exactly, instead of the 6s screen-silence guess.
//! - `PreToolUse(AskUserQuestion)` raises `asking` the moment the form appears.
//! - every event carries `session_id` + `transcript_path`: the resume id and
//!   the transcript to tail are known without scanning `~/.claude/projects`.
//! - `PreCompact` / `PostCompact` expose "整理上下文中" so a silent half minute
//!   has an explanation; `StopFailure` exposes the error kind (rate limit…).
//!
//! All observational hooks are `async` (Claude never waits on us). A session
//! that has produced at least one hook event is "hooked": from then on the
//! screen heuristics stand down for it. Sessions without hooks (shell, an old
//! Claude Code) keep the old behaviour.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::{json, Value};

use crate::api::App;
use crate::pool::{Session, State};

/// Header carrying the daemon session id (value = `$AAA_SESSION`).
pub const SESSION_HEADER: &str = "X-AAA-Session";
pub const SESSION_ENV: &str = "AAA_SESSION";

/// Events we subscribe to. Unknown events posted by a newer Claude Code are
/// accepted and ignored.
pub const EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "Stop",
    "StopFailure",
    "Notification",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

/// Settings fragment for `claude --settings <file>`.
pub fn settings_json(port: u16, token: &str) -> Value {
    let mut hooks = serde_json::Map::new();
    for ev in EVENTS {
        let mut entry = json!({
            "hooks": [{
                "type": "http",
                "url": format!("http://127.0.0.1:{port}/api/v1/hooks/{ev}"),
                "headers": {
                    "Authorization": format!("Bearer {token}"),
                    SESSION_HEADER: format!("${SESSION_ENV}"),
                },
                "allowedEnvVars": [SESSION_ENV],
                "async": true,
                "timeout": 10
            }]
        });
        if *ev == "PreToolUse" {
            entry["matcher"] = json!("AskUserQuestion");
        }
        hooks.insert(ev.to_string(), json!([entry]));
    }
    json!({ "hooks": Value::Object(hooks) })
}

/// Write (or refresh) the settings file; returns its path. Token is inside, so
/// the file is 0600. Rewritten only when the content changed.
pub fn ensure_settings(app: &App) -> std::io::Result<PathBuf> {
    let port = match app.bound_port.load(std::sync::atomic::Ordering::Relaxed) {
        0 => app.cfg.port,
        p => p,
    };
    let path = settings_path(&app.paths);
    let body = serde_json::to_vec_pretty(&settings_json(port, &app.cfg.token))?;
    if std::fs::read(&path).map(|cur| cur == body).unwrap_or(false) {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::paths::write_atomic(&path, &body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn settings_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.state_dir().join("claude-hooks.json")
}

/// Append `--settings <file>` to a claude command line.
pub fn with_settings(cmd: String, agent: &crate::agents::AgentDef, path: &Path) -> String {
    if agent.id == "claude" {
        format!("{cmd} --settings {}", crate::agents::shell_quote(&path.to_string_lossy()))
    } else {
        cmd
    }
}

/// What a hook did to the session, for the caller to act on.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// running → waiting just happened (run the inbox feed)
    pub entered_waiting: bool,
    /// session json changed
    pub dirty: bool,
    /// (project_path, claude session id) learned for the first time → registry
    pub learned_id: Option<(String, String)>,
}

/// Fold one hook event into the session. Pure with respect to I/O: the caller
/// persists / notifies based on [`Applied`]. `now` is injectable for tests.
pub fn apply(sess: &Session, event: &str, body: &Value, now: Instant) -> Applied {
    let mut out = Applied::default();
    let session_id = body.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty());
    let transcript = body
        .get("transcript_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);

    // transcript: authoritative, replaces discovery
    if let Some(tp) = transcript {
        let mut store = sess.msgs.lock().unwrap();
        if store.supported && store.file.as_ref() != Some(&tp) {
            // 之前靠 cwd 猜到的文件（或 resume 兜底的旧文件）让位；已编号的历史
            // 消息保留，新文件从头读——不同文件内容不同，不会重复
            store.file = Some(tp);
            store.offset = 0;
            store.partial.clear();
            store.via_fallback = false;
            store.authoritative = true;
        } else if store.supported {
            store.authoritative = true;
        }
    }

    let mut meta = sess.meta.lock().unwrap();
    if meta.state == State::Exited {
        return out;
    }
    if !meta.hooked {
        meta.hooked = true;
        out.dirty = true;
    }
    if let Some(sid) = session_id {
        if meta.resume_id.as_deref() != Some(sid) {
            meta.resume_id = Some(sid.to_string());
            out.learned_id = Some((meta.project_path.clone(), sid.to_string()));
            out.dirty = true;
        }
    }
    match event {
        "UserPromptSubmit" => {
            if meta.state != State::Running {
                meta.state = State::Running;
                meta.needs_name = true;
            }
            meta.error = None;
            meta.compacting = false;
            out.dirty = true;
        }
        "Stop" => {
            if meta.state == State::Running {
                meta.state = State::Waiting;
                out.entered_waiting = true;
            }
            meta.compacting = false;
            out.dirty = true;
        }
        "StopFailure" => {
            let kind = ["error_type", "error", "reason", "matcher"]
                .iter()
                .find_map(|k| body.get(*k).and_then(Value::as_str))
                .unwrap_or("error")
                .to_string();
            if meta.state == State::Running {
                meta.state = State::Waiting;
                out.entered_waiting = true;
            }
            meta.error = Some(kind);
            meta.compacting = false;
            out.dirty = true;
        }
        "Notification" => {
            let kind = body
                .get("notification_type")
                .and_then(Value::as_str)
                .or_else(|| body.get("matcher").and_then(Value::as_str))
                .unwrap_or("");
            if kind == "idle_prompt" && meta.state == State::Running {
                meta.state = State::Waiting;
                out.entered_waiting = true;
                out.dirty = true;
            }
        }
        "PreToolUse" => {
            if body.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion") {
                meta.asking_hint_inst = Some(now);
                if !meta.asking {
                    meta.asking = true;
                    out.dirty = true;
                }
            }
        }
        "PreCompact" => {
            if !meta.compacting {
                meta.compacting = true;
                out.dirty = true;
            }
        }
        "PostCompact" => {
            if meta.compacting {
                meta.compacting = false;
                out.dirty = true;
            }
        }
        _ => {}
    }
    out
}

/// Resolve the session a hook belongs to: the header first; else the single
/// live claude session whose cwd matches (parallel sessions in one directory
/// are ambiguous → None).
pub fn resolve(app: &App, header: Option<&str>, body: &Value) -> Option<Arc<Session>> {
    if let Some(id) = header.filter(|h| !h.is_empty()) {
        if let Some(s) = app.pool.get(id) {
            return Some(s);
        }
    }
    if let Some(sid) = body.get("session_id").and_then(Value::as_str) {
        let by_id: Vec<Arc<Session>> = app
            .pool
            .all()
            .into_iter()
            .filter(|s| s.meta.lock().unwrap().resume_id.as_deref() == Some(sid))
            .collect();
        if by_id.len() == 1 {
            return by_id.into_iter().next();
        }
    }
    let cwd = body.get("cwd").and_then(Value::as_str)?;
    let by_cwd: Vec<Arc<Session>> = app
        .pool
        .all()
        .into_iter()
        .filter(|s| {
            let m = s.meta.lock().unwrap();
            m.agent == "claude" && m.state != State::Exited && m.project_path == cwd
        })
        .collect();
    if by_cwd.len() == 1 {
        by_cwd.into_iter().next()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_register_every_event_as_async_http_with_session_header() {
        let v = settings_json(2730, "aaa_tk_x");
        let hooks = v["hooks"].as_object().unwrap();
        for ev in EVENTS {
            let h = &hooks[*ev][0]["hooks"][0];
            assert_eq!(h["type"], "http");
            assert_eq!(h["url"], format!("http://127.0.0.1:2730/api/v1/hooks/{ev}"));
            assert_eq!(h["headers"]["Authorization"], "Bearer aaa_tk_x");
            assert_eq!(h["headers"][SESSION_HEADER], "$AAA_SESSION");
            assert_eq!(h["allowedEnvVars"][0], SESSION_ENV);
            assert_eq!(h["async"], true);
        }
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "AskUserQuestion");
        assert!(hooks["Stop"][0].get("matcher").is_none());
    }

    #[test]
    fn with_settings_only_touches_claude() {
        let claude = crate::agents::get("claude").unwrap();
        let shell = crate::agents::get("shell").unwrap();
        let p = Path::new("/Users/x/.local/state/aaa-daemon/claude-hooks.json");
        assert_eq!(
            with_settings("claude --dangerously-skip-permissions".into(), claude, p),
            "claude --dangerously-skip-permissions --settings /Users/x/.local/state/aaa-daemon/claude-hooks.json"
        );
        assert_eq!(with_settings("exec zsh -l".into(), shell, p), "exec zsh -l");
        let sp = Path::new("/tmp/a b/x.json");
        assert!(with_settings("claude".into(), claude, sp).ends_with("--settings '/tmp/a b/x.json'"));
    }

    fn body(event: &str, extra: Value) -> Value {
        let mut v = json!({
            "session_id": "c0ffee",
            "transcript_path": "/Users/x/.claude/projects/-p/c0ffee.jsonl",
            "cwd": "/p",
            "hook_event_name": event,
        });
        if let Some(o) = extra.as_object() {
            for (k, val) in o {
                v[k] = val.clone();
            }
        }
        v
    }

    #[test]
    fn prompt_and_stop_drive_the_state_machine() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        let a = apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        assert!(a.dirty && !a.entered_waiting);
        assert_eq!(a.learned_id, Some(("/p".to_string(), "c0ffee".to_string())));
        {
            let m = sess.meta.lock().unwrap();
            assert!(m.hooked);
            assert_eq!(m.state, State::Running);
            assert_eq!(m.resume_id.as_deref(), Some("c0ffee"));
        }
        assert_eq!(
            sess.msgs.lock().unwrap().file.as_deref(),
            Some(Path::new("/Users/x/.claude/projects/-p/c0ffee.jsonl"))
        );
        let a = apply(&sess, "Stop", &body("Stop", json!({"last_assistant_message": "done"})), now);
        assert!(a.entered_waiting);
        assert!(a.learned_id.is_none(), "same id twice is not news");
        assert_eq!(sess.meta.lock().unwrap().state, State::Waiting);
        // a second Stop while already waiting is not another "entered"
        let a = apply(&sess, "Stop", &body("Stop", json!({})), now);
        assert!(!a.entered_waiting);
    }

    #[test]
    fn stop_failure_records_kind_and_prompt_clears_it() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        let a = apply(&sess, "StopFailure", &body("StopFailure", json!({"error_type": "rate_limit"})), now);
        assert!(a.entered_waiting);
        assert_eq!(sess.meta.lock().unwrap().error.as_deref(), Some("rate_limit"));
        apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        assert!(sess.meta.lock().unwrap().error.is_none());
    }

    #[test]
    fn ask_user_question_raises_asking_and_compaction_toggles() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        apply(&sess, "PreToolUse", &body("PreToolUse", json!({"tool_name": "Bash"})), now);
        assert!(!sess.meta.lock().unwrap().asking);
        let a = apply(&sess, "PreToolUse", &body("PreToolUse", json!({"tool_name": "AskUserQuestion"})), now);
        assert!(a.dirty && sess.meta.lock().unwrap().asking);
        apply(&sess, "PreCompact", &body("PreCompact", json!({"trigger": "auto"})), now);
        assert!(sess.meta.lock().unwrap().compacting);
        apply(&sess, "PostCompact", &body("PostCompact", json!({})), now);
        assert!(!sess.meta.lock().unwrap().compacting);
    }

    #[test]
    fn idle_notification_ends_the_turn_but_other_notifications_do_not() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        let a = apply(&sess, "Notification", &body("Notification", json!({"notification_type": "auth_success"})), now);
        assert!(!a.entered_waiting);
        let a = apply(&sess, "Notification", &body("Notification", json!({"notification_type": "idle_prompt"})), now);
        assert!(a.entered_waiting);
    }

    #[test]
    fn exited_sessions_ignore_late_events() {
        let sess = Session::for_test("claude", "/p");
        sess.meta.lock().unwrap().state = State::Exited;
        let a = apply(&sess, "Stop", &body("Stop", json!({})), Instant::now());
        assert_eq!(a, Applied::default());
        assert!(!sess.meta.lock().unwrap().hooked);
    }
}
