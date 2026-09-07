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
//! - `SessionStart`（startup / resume / clear）：TUI 就绪停在输入框 → `waiting`
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
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "StopFailure",
    "Notification",
    "PreCompact",
    "PostCompact",
    "SessionEnd",
];

pub const STATUSLINE_EVENT: &str = "statusline";

/// statusLine 命令：把 Claude Code 推来的状态 JSON 原样转给 daemon，自己不打印任何字——
/// 终端里不再占一行（claude-hud 那种状态栏就是这份 JSON 画出来的）。
pub fn statusline_script(port: u16, token: &str) -> String {
    format!(
        "#!/bin/sh\n# AAA: forward Claude Code status JSON to the daemon; print nothing.\n\
         curl -s -m 2 -X POST -H 'Authorization: Bearer {token}' -H \"X-AAA-Session: ${{{env}:-}}\" \\\n  -H 'Content-Type: application/json' --data-binary @- \\\n  http://127.0.0.1:{port}/api/v1/hooks/{ev} >/dev/null 2>&1\nexit 0\n",
        env = SESSION_ENV,
        ev = STATUSLINE_EVENT,
    )
}

pub fn statusline_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.state_dir().join("aaa-statusline.sh")
}

/// Settings fragment for `claude --settings <file>`.
pub fn settings_json(port: u16, token: &str) -> Value {
    settings_json_with(port, token, &statusline_path(&crate::paths::Paths::from_env()))
}

pub fn settings_json_with(port: u16, token: &str, statusline: &Path) -> Value {
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
    // Remote Control 与本应用重叠：daemon 起的会话一律关掉（覆盖用户 settings 里的开关）
    json!({
        "hooks": Value::Object(hooks),
        "remoteControlAtStartup": false,
        "statusLine": { "type": "command", "command": statusline.to_string_lossy(), "padding": 0 }
    })
}

/// Write (or refresh) the settings file; returns its path. Token is inside, so
/// the file is 0600. Rewritten only when the content changed.
pub fn ensure_settings(app: &App) -> std::io::Result<PathBuf> {
    let port = match app.bound_port.load(std::sync::atomic::Ordering::Relaxed) {
        0 => app.cfg.port,
        p => p,
    };
    let path = settings_path(&app.paths);
    let script = statusline_path(&app.paths);
    let body = serde_json::to_vec_pretty(&settings_json_with(port, &app.cfg.token, &script))?;
    let script_body = statusline_script(port, &app.cfg.token).into_bytes();
    let same = |p: &Path, want: &[u8]| std::fs::read(p).map(|cur| cur == want).unwrap_or(false);
    if same(&path, &body) && same(&script, &script_body) {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::paths::write_atomic(&script, &script_body)?;
    crate::paths::write_atomic(&path, &body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700));
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
    /// statusline 带来的 plan 配额（rate_limits），有就广播
    pub plan: Option<Value>,
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
        STATUSLINE_EVENT => {
            let usage = session_usage(body);
            if meta.usage != Some(usage.clone()) {
                meta.usage = Some(usage);
                out.dirty = true;
            }
            out.plan = plan_usage(body);
        }
        "SessionStart" => {
            // TUI 起来了、停在输入框：轮到你。compact 是一轮中途的整理，不算
            let source = body.get("source").and_then(Value::as_str).unwrap_or("");
            if source != "compact" {
                if meta.state == State::Running {
                    meta.state = State::Waiting;
                    meta.touch();
                }
                // 收件箱里排着的（比如 daemon 重启前发的待发送）这时喂进去
                out.entered_waiting = true;
                out.dirty = true;
            }
        }
        "UserPromptSubmit" => {
            if meta.state != State::Running {
                meta.state = State::Running;
                meta.needs_name = true;
                meta.touch();
            }
            meta.running_by_transcript = false;
            meta.permission = None;
            meta.error = None;
            meta.compacting = false;
            out.dirty = true;
        }
        "Stop" => {
            if meta.state == State::Running {
                meta.state = State::Waiting;
                meta.touch();
                out.entered_waiting = true;
            }
            meta.last_stop_at = Some(chrono::Utc::now());
            meta.running_by_transcript = false;
            meta.permission = None;
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
                meta.touch();
                out.entered_waiting = true;
            }
            meta.last_stop_at = Some(chrono::Utc::now());
            meta.running_by_transcript = false;
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
                meta.touch();
                out.entered_waiting = true;
                out.dirty = true;
            }
            if kind == "idle_prompt" {
                meta.last_stop_at = Some(chrono::Utc::now());
                meta.running_by_transcript = false;
            }
            // 兜底：权限对话框已经弹了 6 秒还没人答（PermissionRequest 没送到 / 老版本）、
            // 或 MCP 的 elicitation 表单在等——都记成待回复。elicitation 没法替答，卡片只
            // 提示去终端
            let msg = body.get("message").and_then(Value::as_str).unwrap_or("").to_string();
            match kind {
                "permission_prompt" if meta.permission.is_none() => {
                    meta.permission = Some(json!({"kind": "permission", "tool_name": "权限", "summary": msg, "tool_input": Value::Null, "tool_use_id": Value::Null,
                        "since": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}));
                    meta.asking = true;
                    meta.asking_hint_inst = Some(now);
                    out.dirty = true;
                }
                "elicitation_dialog" | "elicitation_url_dialog" => {
                    meta.permission = Some(json!({"kind": "elicitation", "tool_name": "对话框", "summary": msg, "tool_input": Value::Null, "tool_use_id": Value::Null,
                        "since": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}));
                    meta.asking = true;
                    meta.asking_hint_inst = Some(now);
                    out.dirty = true;
                }
                "elicitation_complete" | "elicitation_response" => {
                    if meta.permission.as_ref().is_some_and(|p| p["kind"] == "elicitation") {
                        meta.permission = None;
                        out.dirty = true;
                    }
                }
                _ => {}
            }
        }
        "PermissionRequest" => {
            // 权限对话框要弹了（Bash 授权、ExitPlanMode 批准…）：记下来 = 待回复。
            // 我们不在 hook 里决定（async，返回被忽略，对话框照常弹）；客户端按钮通过
            // POST /sessions/:id/permission 驱动 PTY 作答，终端里手动答也一样
            let tool = body.get("tool_name").and_then(Value::as_str).unwrap_or("").to_string();
            let input = body.get("tool_input").cloned().unwrap_or(Value::Null);
            meta.permission = Some(json!({
                "kind": "permission",
                "tool_name": tool,
                "summary": permission_summary(&tool, &input),
                "tool_input": input,
                "tool_use_id": body.get("tool_use_id").cloned().unwrap_or(Value::Null),
                "since": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            }));
            meta.asking_hint_inst = Some(now);
            if !meta.asking {
                meta.asking = true;
            }
            out.dirty = true;
        }
        "PostToolUse" => {
            // 工具跑完了：对话框肯定没了（允许了）；拒绝的路径靠 Stop / UserPromptSubmit 清
            if meta.permission.take().is_some() {
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

/// 权限对话框的一行摘要：Bash 给命令，文件类给路径，ExitPlanMode 给「批准计划」，其它给工具名
pub fn permission_summary(tool: &str, input: &Value) -> String {
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let cap = |t: &str| -> String { let t: String = t.chars().take(160).collect(); t.replace('\n', " ") };
    match tool {
        "Bash" => cap(s("command")),
        "Edit" | "Write" | "Read" | "NotebookEdit" | "MultiEdit" => cap(s("file_path")),
        "ExitPlanMode" => "批准计划并退出计划模式".to_string(),
        "WebFetch" => cap(s("url")),
        "Agent" | "Task" => cap(s("description")),
        _ => String::new(),
    }
}

/// 本会话用量：statusLine JSON 里与这一个会话有关的部分，压成客户端直接能画的形状。
pub fn session_usage(body: &Value) -> Value {
    let cw = body.get("context_window");
    let pct = cw
        .and_then(|c| c.get("used_percentage"))
        .and_then(Value::as_f64)
        .or_else(|| {
            // 老版本没有百分比：用 current_usage 各项之和 / 窗口大小
            let size = cw?.get("context_window_size")?.as_f64()?;
            let cu = cw?.get("current_usage")?;
            let sum: f64 = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
                .iter()
                .filter_map(|k| cu.get(*k).and_then(Value::as_f64))
                .sum();
            (size > 0.0).then(|| (sum / size * 100.0).round())
        });
    let cost = body.get("cost");
    let g = |v: Option<&Value>, k: &str| v.and_then(|c| c.get(k)).cloned().unwrap_or(Value::Null);
    // 最近一次 API 调用的输入构成（= 现在上下文里的东西）：新读的 / 新写进缓存的 / 从缓存读的。
    // 命中率 = 缓存读 ÷ 三者之和；有 cache_read 就说明这段对话的缓存还活着
    let cu = cw.and_then(|c| c.get("current_usage"));
    let n = |k: &str| cu.and_then(|c| c.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
    let (fresh, created, read) = (n("input_tokens"), n("cache_creation_input_tokens"), n("cache_read_input_tokens"));
    let total = fresh + created + read;
    // 一位小数：整数四舍五入下 99.6% 显示成 100%，用户以为「全命中了不用 compact」——
    // 其实命中率只说上一次调用的输入构成，跟要不要 compact（看 context_pct）无关
    let cache_hit_pct = if cu.is_some() && total > 0.0 { json!((read / total * 1000.0).round() / 10.0) } else { Value::Null };
    json!({
        "model": g(body.get("model"), "display_name"),
        "model_id": g(body.get("model"), "id"),
        "context_pct": pct,
        "context_window_size": g(cw, "context_window_size"),
        "input_tokens": g(cw, "total_input_tokens"),
        "output_tokens": g(cw, "total_output_tokens"),
        // v1.8：提示缓存（current_usage 里的三项 + 命中率）；老 Claude Code 没有 current_usage 就全 null
        "cache_read_tokens": g(cu, "cache_read_input_tokens"),
        "cache_creation_tokens": g(cu, "cache_creation_input_tokens"),
        "fresh_input_tokens": g(cu, "input_tokens"),
        "cache_hit_pct": cache_hit_pct,
        "cost_usd": g(cost, "total_cost_usd"),
        "duration_ms": g(cost, "total_duration_ms"),
        "lines_added": g(cost, "total_lines_added"),
        "lines_removed": g(cost, "total_lines_removed"),
        "effort": body.get("effort").and_then(|e| e.get("level").cloned().or_else(|| e.as_str().map(|s| json!(s)))).unwrap_or(Value::Null),
    })
}

/// 账号级 plan 配额（与会话无关）：`rate_limits` 原样透传加时间戳；没有就 None。
pub fn plan_usage(body: &Value) -> Option<Value> {
    let rl = body.get("rate_limits")?;
    if rl.is_null() {
        return None;
    }
    Some(json!({
        "five_hour": rl.get("five_hour").cloned().unwrap_or(Value::Null),
        "seven_day": rl.get("seven_day").cloned().unwrap_or(Value::Null),
        "model_scoped": rl.get("model_scoped").cloned().unwrap_or(Value::Null),
        "updated_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    }))
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
        assert_eq!(v["remoteControlAtStartup"], false);
        assert!(hooks["Stop"][0].get("matcher").is_none());
    }

    #[test]
    fn statusline_is_forwarded_and_summarised() {
        let v = settings_json_with(2730, "tk", Path::new("/s/aaa-statusline.sh"));
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["command"], "/s/aaa-statusline.sh");
        let sh = statusline_script(2730, "tk");
        assert!(sh.starts_with("#!/bin/sh\n"));
        assert!(sh.contains("Bearer tk") && sh.contains("X-AAA-Session: ${AAA_SESSION:-}") && sh.contains("/api/v1/hooks/statusline"));
        let body = json!({
            "model": {"id": "claude-fable-5-1", "display_name": "Fable 5.1"},
            "context_window": {"context_window_size": 200000, "total_input_tokens": 1200, "total_output_tokens": 300,
                "current_usage": {"input_tokens": 20000, "cache_creation_input_tokens": 10000, "cache_read_input_tokens": 30000}},
            "cost": {"total_cost_usd": 1.25, "total_duration_ms": 5000, "total_lines_added": 10, "total_lines_removed": 2},
            "rate_limits": {"five_hour": {"used_percentage": 32, "resets_at": 1700000000}, "seven_day": {"used_percentage": 61, "resets_at": 1700400000}},
            "effort": {"level": "high"}
        });
        let u = session_usage(&body);
        assert_eq!(u["model"], "Fable 5.1");
        assert_eq!(u["context_pct"], 30.0, "60k of 200k");
        assert_eq!(u["cache_read_tokens"], 30000);
        assert_eq!(u["cache_creation_tokens"], 10000);
        assert_eq!(u["cache_hit_pct"], 50.0, "30k read of 60k");
        assert_eq!(u["cost_usd"], 1.25);
        assert_eq!(u["effort"], "high");
        let with_pct = json!({"context_window": {"used_percentage": 42.5}});
        assert_eq!(session_usage(&with_pct)["context_pct"], 42.5, "native percentage wins");
        assert!(session_usage(&with_pct)["cache_hit_pct"].is_null(), "没有 current_usage 就不算");
        let p = plan_usage(&body).unwrap();
        assert_eq!(p["five_hour"]["used_percentage"], 32);
        assert_eq!(p["seven_day"]["used_percentage"], 61);
        assert!(plan_usage(&json!({"model": {}})).is_none());
        // through apply(): usage lands on the session, plan comes back out
        let sess = Session::for_test("claude", "/p");
        let a = apply(&sess, STATUSLINE_EVENT, &body, Instant::now());
        assert!(a.dirty && a.plan.is_some());
        assert_eq!(sess.meta.lock().unwrap().usage.as_ref().unwrap()["model"], "Fable 5.1");
        let a = apply(&sess, STATUSLINE_EVENT, &body, Instant::now());
        assert!(!a.dirty, "same usage twice is not a change");
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

    /// v1.16：PermissionRequest → 待回复 + permission 摘要；PostToolUse / UserPromptSubmit 清掉
    #[test]
    fn permission_request_marks_asking_until_the_tool_runs_or_a_new_prompt() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        let a = apply(&sess, "PermissionRequest", &body("PermissionRequest", json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build\n&& ls"}, "tool_use_id": "tu1"})), now);
        assert!(a.dirty);
        {
            let m = sess.meta.lock().unwrap();
            assert!(m.asking);
            let p = m.permission.as_ref().unwrap();
            assert_eq!(p["tool_name"], "Bash");
            assert_eq!(p["summary"], "rm -rf build && ls");
            assert_eq!(p["tool_use_id"], "tu1");
        }
        apply(&sess, "PostToolUse", &body("PostToolUse", json!({"tool_name": "Bash"})), now);
        assert!(sess.meta.lock().unwrap().permission.is_none(), "工具跑完 = 对话框没了");
        apply(&sess, "PermissionRequest", &body("PermissionRequest", json!({"tool_name": "ExitPlanMode", "tool_input": {}})), now);
        assert_eq!(sess.meta.lock().unwrap().permission.as_ref().unwrap()["summary"], "批准计划并退出计划模式");
        apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        assert!(sess.meta.lock().unwrap().permission.is_none(), "用户又发言了 = 拒绝路径走完了");
    }

    #[test]
    fn notification_fallbacks_mark_prompts_and_elicitation() {
        let sess = Session::for_test("claude", "/p");
        let now = Instant::now();
        apply(&sess, "Notification", &body("Notification", json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"})), now);
        {
            let m = sess.meta.lock().unwrap();
            assert!(m.asking);
            assert_eq!(m.permission.as_ref().unwrap()["kind"], "permission");
        }
        apply(&sess, "PostToolUse", &body("PostToolUse", json!({})), now);
        apply(&sess, "Notification", &body("Notification", json!({"notification_type": "elicitation_dialog", "message": "server asks for input"})), now);
        assert_eq!(sess.meta.lock().unwrap().permission.as_ref().unwrap()["kind"], "elicitation");
        apply(&sess, "Notification", &body("Notification", json!({"notification_type": "elicitation_response"})), now);
        assert!(sess.meta.lock().unwrap().permission.is_none());
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
    fn session_start_means_waiting_at_the_prompt() {
        let now = Instant::now();
        let sess = Session::for_test("claude", "/p");
        assert_eq!(sess.meta.lock().unwrap().state, State::Running);
        // resume 出来的会话：起来就停在输入框，是 waiting，并且要喂一次收件箱
        let a = apply(&sess, "SessionStart", &body("SessionStart", json!({"source": "resume"})), now);
        assert!(a.entered_waiting);
        assert_eq!(sess.meta.lock().unwrap().state, State::Waiting);
        // 一轮中途的 compact 重启不算：正在跑就还是跑
        apply(&sess, "UserPromptSubmit", &body("UserPromptSubmit", json!({})), now);
        let a = apply(&sess, "SessionStart", &body("SessionStart", json!({"source": "compact"})), now);
        assert!(!a.entered_waiting);
        assert_eq!(sess.meta.lock().unwrap().state, State::Running);
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
