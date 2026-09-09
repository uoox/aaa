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
//! All observational hooks are `async` (the agent never waits on us).
//! `Session.hooked` 在**开会话时**定：这次到底有没有把钩子装上（claude 写成了
//! `--settings` 片段、agy 写进了它的全局 hooks.json）。装上了，屏幕启发式就此让位；
//! 没装上（终端、写不出文件）照旧看屏幕。第一条事件到达时也会补设一次，
//! 那是给「老 daemon 起的会话重启后被接管」留的兜底。

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

/// 把 stdin 上那段 JSON 原样转给 daemon 的 `/hooks/$1`，自己**不打印任何字**
/// （claude 的 statusLine 就是拿它当状态栏命令：终端里一行不占）。
///
/// 一个脚本管两处：claude 的 statusLine（`aaa-hook.sh statusline`）和 agy 那几个
/// 命令型钩子（`aaa-hook.sh UserPromptSubmit`…）。两边此前各写了一遍同样的 curl。
/// `AAA_SESSION` 没设就立刻退出——用户自己在终端里跑 agent 时不该往 daemon 发东西。
pub fn hook_script(port: u16, token: &str) -> String {
    format!(
        "#!/bin/sh\n# AAA: forward a hook/status payload to the daemon. $1 = event name.\n\
         [ -n \"${{{env}:-}}\" ] || exit 0\n\
         curl -s -m 2 -X POST -H 'Authorization: Bearer {token}' -H \"X-AAA-Session: ${env}\" \\\n           -H 'Content-Type: application/json' --data-binary @- \\\n           \"http://127.0.0.1:{port}/api/v1/hooks/$1\" >/dev/null 2>&1\nexit 0\n",
        env = SESSION_ENV,
    )
}

pub fn hook_script_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.state_dir().join("aaa-hook.sh")
}

// ---- agy（Antigravity CLI）的 hooks ----
//
// agy 没有 `--settings`，钩子只有全局一份 `~/.gemini/config/hooks.json`，而且只认
// `type:"command"`。所以这里不像 claude 那样直接给 URL，而是写一个转发脚本，
// **事件名的映射放在 hooks.json 里**（agy 的 `PreInvocation` 打到 daemon 的
// `UserPromptSubmit`）——按 agent 分派的只能是事实来源，判定仍然只有一份。
//
// 那份文件是用户的：我们只写自己那个 `aaa` 键，别人的（比如 orca-status）原样保留；
// 解析不动就整个放弃，宁可这次没有钩子，也不能把人家的键盖没了。卸载时的清理见 `service.rs`。
// 脚本在 `AAA_SESSION` 没设时立刻退出：用户自己在终端里跑 agy 不该往 daemon 发东西。

/// agy 的一个钩子事件：它那边叫什么、打到 daemon 的哪个事件、要不要包一层工具名匹配器。
pub struct AgyEvent {
    pub agy: &'static str,
    pub daemon: &'static str,
    /// agy 的文件里带工具名的事件写成 `matcher` + 嵌套 `hooks`，其余是平铺的一串命令。
    /// 写死在表里而不是从事件名猜（此前按 `ends_with("ToolUse")` 认，agy 哪天加一个
    /// 别的带工具名的事件就会写成错的形状，而且没人看得出来）。
    pub matcher: bool,
}

/// agy 事件 → daemon 事件。`PreToolUse` 不接：daemon 那边它是 claude 专用的
/// AskUserQuestion 匹配器，agy 没有对应的结构化提问，接了只是噪声。
/// agy 一共只有 PreInvocation / PostInvocation / Stop / PreToolUse / PostToolUse 五个，
/// **没有会话启动事件**——收件箱在 agy 会话刚起来时靠每秒那一遍 tick 投喂，不靠事件。
pub const AGY_EVENTS: &[AgyEvent] = &[
    AgyEvent { agy: "PreInvocation", daemon: "UserPromptSubmit", matcher: false },
    AgyEvent { agy: "Stop", daemon: "Stop", matcher: false },
    AgyEvent { agy: "PostToolUse", daemon: "PostToolUse", matcher: true },
];

/// 我们在 agy 的 hooks.json 里占的键
pub const AGY_KEY: &str = "aaa";

pub fn agy_hooks_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.home.join(".gemini").join("config").join("hooks.json")
}

pub fn agy_settings_path(paths: &crate::paths::Paths) -> PathBuf {
    paths.home.join(".gemini").join("antigravity-cli").join("settings.json")
}

/// 接管 agy 的 statusLine（模型、token、缓存命中就从这儿来；它推的 JSON 与
/// Claude Code 同一个形状）。**只在那一格空着、或者本来就是我们的时候才写**——
/// 用户装了别的状态栏（agy-hud 那类）就别抢，宁可没有用量也不动人家的东西。
/// 返回是否真的接管了。
pub fn ensure_agy_statusline(paths: &crate::paths::Paths, script: &Path) -> std::io::Result<bool> {
    let path = agy_settings_path(paths);
    let want = format!("{} {STATUSLINE_EVENT}", crate::agents::shell_quote(&script.to_string_lossy()));
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(false) };
    let Some(mut root) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_object().cloned())
    else {
        return Ok(false); // 解析不动就别碰，和 hooks.json 一个道理
    };
    match root.get("statusLine").and_then(|v| v.get("command")).and_then(Value::as_str) {
        Some(cur) if cur == want => return Ok(true), // 已经是我们的
        Some(_) => return Ok(false),                 // 别人的，不抢
        None => {}
    }
    root.insert("statusLine".into(), json!({"type": "command", "command": want}));
    crate::paths::write_atomic(&path, &serde_json::to_vec_pretty(&Value::Object(root))?)?;
    Ok(true)
}

/// 卸载时把 statusLine 那一格还回去（只还我们自己放的）
pub fn remove_agy_statusline(paths: &crate::paths::Paths) -> std::io::Result<bool> {
    let path = agy_settings_path(paths);
    let want_prefix = hook_script_path(paths).to_string_lossy().into_owned();
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(false) };
    let Some(mut root) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_object().cloned())
    else {
        return Ok(false);
    };
    let ours = root
        .get("statusLine")
        .and_then(|v| v.get("command"))
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(&want_prefix));
    if !ours {
        return Ok(false);
    }
    root.remove("statusLine");
    crate::paths::write_atomic(&path, &serde_json::to_vec_pretty(&Value::Object(root))?)?;
    Ok(true)
}

/// 我们那一段的内容（`hooks.json` 里 `aaa` 键的值）
pub fn agy_hooks_entry(script: &Path) -> Value {
    let cmd = |daemon_event: &str| {
        json!({
            "type": "command",
            "command": format!("{} {daemon_event}", crate::agents::shell_quote(&script.to_string_lossy())),
            "timeout": 10
        })
    };
    let mut m = serde_json::Map::new();
    for ev in AGY_EVENTS {
        let v = if ev.matcher {
            json!([{ "matcher": "*", "hooks": [cmd(ev.daemon)] }])
        } else {
            json!([cmd(ev.daemon)])
        };
        m.insert(ev.agy.to_string(), v);
    }
    Value::Object(m)
}

/// 把 `aaa` 那一段装进（或刷新）用户的 `hooks.json`，别人的键原样保留。
///
/// **装不上就退回屏幕差分**（全库只在这里说一次）：`Err` 的意思是「这个会话没有事件源」，
/// 调用方必须据此把 `Session.hooked` 置 false，而不是按 agent 名猜。猜错的后果是
/// 会话永远停在「在跑」——没有 Stop 事件，也没有屏幕差分接手。
pub fn ensure_agy_hooks(app: &App) -> std::io::Result<()> {
    let port = match app.bound_port.load(std::sync::atomic::Ordering::Relaxed) {
        0 => app.cfg.port,
        p => p,
    };
    let script = write_hook_script(&app.paths, port, &app.cfg.token)?;
    let path = agy_hooks_path(&app.paths);
    // 两个 daemon 线程同时开 agy 会话时的 read-modify-write：`write_atomic` 的 tmp 名字
    // 是固定的，不加锁两边会互相盖掉
    static AGY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = AGY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // **文件不存在**才当空表。读得出来却解析不动（用户手改坏了一个逗号）时必须报错退出——
    // 否则我们会拿 `{"aaa":…}` 盖掉人家所有的钩子。这是唯一一处 AAA 写别人的文件，
    // 出错方向只能选「什么都不做」。
    let mut root = match std::fs::read_to_string(&path) {
        Ok(text) if text.trim().is_empty() => serde_json::Map::new(),
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{} 不是一个 JSON 对象，不敢覆盖", path.display()),
                )
            })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(e),
    };
    let want = agy_hooks_entry(&script);
    if root.get(AGY_KEY) != Some(&want) {
        root.insert(AGY_KEY.to_string(), want);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::paths::write_atomic(&path, &serde_json::to_vec_pretty(&Value::Object(root))?)?;
    }
    // statusLine 那一格空着就顺手接过来。**在钩子那一步的短路之外**：钩子早就装好了
    // 的机器（升级上来的）也得有机会接管，拿不到用量不算失败，钩子才是关键。
    let _ = ensure_agy_statusline(&app.paths, &script);
    Ok(())
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
        "statusLine": { "type": "command", "command": format!("{} {STATUSLINE_EVENT}", crate::agents::shell_quote(&statusline.to_string_lossy())), "padding": 0 }
    })
}

/// 写（或刷新）那个转发脚本，返回它的路径。claude 的 statusLine 与 agy 的钩子共用它。
fn write_hook_script(paths: &crate::paths::Paths, port: u16, token: &str) -> std::io::Result<PathBuf> {
    let script = hook_script_path(paths);
    let body = hook_script(port, token).into_bytes();
    if std::fs::read(&script).map(|cur| cur != body).unwrap_or(true) {
        if let Some(dir) = script.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::paths::write_atomic(&script, &body)?;
    }
    // 权限每次都设：内容没变但模式被谁放宽过时，token 就一直躺在一个可读的文件里
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700));
    }
    Ok(script)
}

/// 把我们塞进 agy 全局 `hooks.json` 的那一段摘掉（`aaa-daemon service uninstall` 调）。
/// 不摘的话卸载之后那份配置里会留下一条指向已删脚本的命令，agy 每次事件都去跑它。
/// 文件不在、解析不动、里面本来就没有我们那一段：都当没事发生。
pub fn remove_agy_hooks(paths: &crate::paths::Paths) -> std::io::Result<bool> {
    let path = agy_hooks_path(paths);
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(false) };
    let Some(mut root) = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_object().cloned())
    else {
        return Ok(false);
    };
    if root.remove(AGY_KEY).is_none() {
        return Ok(false);
    }
    crate::paths::write_atomic(&path, &serde_json::to_vec_pretty(&Value::Object(root))?)?;
    Ok(true)
}

/// Write (or refresh) the settings file; returns its path. Token is inside, so
/// the file is 0600. Rewritten only when the content changed.
pub fn ensure_settings(app: &App) -> std::io::Result<PathBuf> {
    let port = match app.bound_port.load(std::sync::atomic::Ordering::Relaxed) {
        0 => app.cfg.port,
        p => p,
    };
    let path = settings_path(&app.paths);
    let script = write_hook_script(&app.paths, port, &app.cfg.token)?;
    let body = serde_json::to_vec_pretty(&settings_json_with(port, &app.cfg.token, &script))?;
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
/// 返回 `(命令行, 装上了没有)`。**两件事只判一次**：此前调用方还要自己再写一遍
/// `agent.id == "claude"` 来决定 `hooked`，两处一旦不一致就会出现「自称装了钩子、
/// 命令行里却没有 --settings」的会话。
pub fn with_settings(cmd: String, agent: &crate::agents::AgentDef, path: &Path) -> (String, bool) {
    if agent.id == "claude" {
        (format!("{cmd} --settings {}", crate::agents::shell_quote(&path.to_string_lossy())), true)
    } else {
        (cmd, false)
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
    // **明写着窗口是 0** = 这个 agent 这一帧不知道窗口有多大（agy 就是这样，它同时把
    // used_percentage 也填 0）。那时百分比只能是「不知道」，不能当真的 0% 画出来。
    // 字段整个没有则不算数：老 Claude Code 只给百分比不给大小，那个百分比是真的。
    let window_says_unknown = cw
        .and_then(|c| c.get("context_window_size"))
        .and_then(Value::as_f64)
        .is_some_and(|s| s <= 0.0);
    let pct = cw
        .filter(|_| !window_says_unknown)
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
            m.agent != "shell" && m.state != State::Exited && m.project_path == cwd
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
        let v = settings_json_with(2730, "aaa_tk_x", Path::new("/tmp/statusline"));
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

    /// agy 的钩子写在用户全局那份 `hooks.json` 里：只许动我们自己那个键，
    /// 而且事件名的映射（agy 的 PreInvocation → daemon 的 UserPromptSubmit）在这一层做完，
    /// daemon 那边仍然只有一套判定。
    #[test]
    fn agy_hooks_map_events_and_leave_other_keys_alone() {
        let e = agy_hooks_entry(Path::new("/s/aaa-hook.sh"));
        assert_eq!(e["PreInvocation"][0]["type"], "command");
        assert_eq!(e["PreInvocation"][0]["command"], "/s/aaa-hook.sh UserPromptSubmit");
        assert_eq!(e["Stop"][0]["command"], "/s/aaa-hook.sh Stop");
        // 带工具名的事件是 matcher + 嵌套 hooks（agy 自己的文件就是这个形状）
        assert_eq!(e["PostToolUse"][0]["matcher"], "*");
        assert_eq!(e["PostToolUse"][0]["hooks"][0]["command"], "/s/aaa-hook.sh PostToolUse");
        // PreToolUse 不接：daemon 那边它是 claude 专用的 AskUserQuestion 匹配器
        assert!(e.get("PreToolUse").is_none());

        // statusLine：空着才接，别人的不抢，我们自己的幂等
        let dir1 = tempfile::tempdir().unwrap();
        let paths1 = crate::paths::Paths::new(dir1.path());
        let sp = agy_settings_path(&paths1);
        std::fs::create_dir_all(sp.parent().unwrap()).unwrap();
        let script1 = hook_script_path(&paths1);
        let read_cmd = |p: &Path| -> Option<String> {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()?;
            v.get("statusLine")?.get("command")?.as_str().map(str::to_string)
        };

        std::fs::write(&sp, r#"{"model":"x"}"#).unwrap();
        assert!(ensure_agy_statusline(&paths1, &script1).unwrap(), "空着就接");
        assert!(read_cmd(&sp).unwrap().ends_with(" statusline"));
        assert!(ensure_agy_statusline(&paths1, &script1).unwrap(), "已经是我们的：幂等");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&sp).unwrap()).unwrap();
        assert_eq!(v["model"], "x", "别的设置不许丢");

        std::fs::write(&sp, r#"{"statusLine":{"type":"command","command":"agy-hud.js"}}"#).unwrap();
        assert!(!ensure_agy_statusline(&paths1, &script1).unwrap(), "别人的状态栏不抢");
        assert_eq!(read_cmd(&sp).unwrap(), "agy-hud.js");
        assert!(!remove_agy_statusline(&paths1).unwrap(), "也不许替别人还回去");
        assert_eq!(read_cmd(&sp).unwrap(), "agy-hud.js");

        // 卸载：只还我们自己放的那一格
        std::fs::write(&sp, r#"{"model":"x"}"#).unwrap();
        ensure_agy_statusline(&paths1, &script1).unwrap();
        assert!(remove_agy_statusline(&paths1).unwrap());
        assert!(read_cmd(&sp).is_none());
        assert_eq!(
            serde_json::from_str::<Value>(&std::fs::read_to_string(&sp).unwrap()).unwrap()["model"],
            "x"
        );

        // 坏 JSON 不能当空文件：那会拿 {"aaa":…} 盖掉人家所有的钩子
        let dir0 = tempfile::tempdir().unwrap();
        let paths0 = crate::paths::Paths::new(dir0.path());
        let bad = agy_hooks_path(&paths0);
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::fs::write(&bad, "{\"orca-status\": {,,,}").unwrap();
        // 摘不掉也不许写坏：remove 对坏文件是个空操作
        assert!(!remove_agy_hooks(&paths0).unwrap());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "{\"orca-status\": {,,,}", "原样没动");

        // 摘掉我们那一段，别人的留着
        let good = agy_hooks_path(&paths0);
        std::fs::write(&good, r#"{"orca-status":{"Stop":[]},"aaa":{"Stop":[]}}"#).unwrap();
        assert!(remove_agy_hooks(&paths0).unwrap());
        let left: serde_json::Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(&good).unwrap()).unwrap();
        assert_eq!(left.keys().collect::<Vec<_>>(), vec!["orca-status"]);
        assert!(!remove_agy_hooks(&paths0).unwrap(), "没有我们那一段时是空操作");

        // 合并进已有文件：别人的键原样留着
        let dir = tempfile::tempdir().unwrap();
        let paths = crate::paths::Paths::new(dir.path());
        let path = agy_hooks_path(&paths);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"orca-status":{"Stop":[{"type":"command","command":"theirs"}]}}"#).unwrap();
        let mut root: serde_json::Map<String, Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        root.insert(AGY_KEY.to_string(), agy_hooks_entry(Path::new("/s/aaa-hook.sh")));
        assert_eq!(root["orca-status"]["Stop"][0]["command"], "theirs");
        assert!(root.contains_key("aaa") && root.len() == 2);
    }

    #[test]
    fn statusline_is_forwarded_and_summarised() {
        // statusLine 与 agy 的钩子共用同一个转发脚本，事件名当参数传
        let v = settings_json_with(2730, "tk", Path::new("/s/aaa-hook.sh"));
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["command"], "/s/aaa-hook.sh statusline");
        let sh = hook_script(2730, "tk");
        assert!(sh.starts_with("#!/bin/sh\n"));
        assert!(sh.contains("Bearer tk") && sh.contains("X-AAA-Session: $AAA_SESSION"));
        assert!(sh.contains("/api/v1/hooks/$1"), "事件名由调用方给");
        assert!(sh.contains("[ -n \"${AAA_SESSION:-}\" ] || exit 0"), "不是 AAA 起的会话不发");
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
        // agy 那一帧：窗口明写着 0，百分比也是 0——那是「不知道」，不是「用了 0%」
        let unknown = json!({"context_window": {"used_percentage": 0, "context_window_size": 0,
            "current_usage": {"input_tokens": 6088, "cache_read_input_tokens": 8119}}});
        assert!(session_usage(&unknown)["context_pct"].is_null(), "窗口是 0 时百分比不算数");
        assert_eq!(session_usage(&unknown)["cache_read_tokens"], 8119, "token 数照给");
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
            (
                "claude --dangerously-skip-permissions --settings /Users/x/.local/state/aaa-daemon/claude-hooks.json"
                    .to_string(),
                true
            )
        );
        // 「命令行有没有 --settings」与「算不算 hooked」是同一件事，只判一次
        assert_eq!(with_settings("exec zsh -l".into(), shell, p), ("exec zsh -l".to_string(), false));
        let sp = Path::new("/tmp/a b/x.json");
        let (cmd, hooked) = with_settings("claude".into(), claude, sp);
        assert!(cmd.ends_with("--settings '/tmp/a b/x.json'") && hooked);
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
