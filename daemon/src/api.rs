//! REST + WS endpoints (PROTOCOL.md `/api/v1`).

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Path as UrlPath, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents;
use crate::cache::CwdCache;
use crate::config::Config;
use crate::events::EventHub;
use crate::namer::Namer;
use crate::paths::Paths;
use crate::pool::{SessionPool, SpawnSpec, State as SState};
use crate::registry::Registry;
use crate::statemachine::Question;
use crate::stores;

pub struct App {
    pub cfg: Config,
    pub paths: Paths,
    pub started: Instant,
    pub pool: SessionPool,
    pub hub: EventHub,
    /// Serializes cwd-cache saves (read-merge-write of `~/.cache/aaa-cwds.json`)
    /// and project deletion. Deliberately NOT held during store scans or haiku
    /// naming — a 60s LLM call must never block read-only requests.
    pub store_lock: std::sync::Mutex<()>,
    /// actual bound port (set after bind; cfg.port may be 0 = ephemeral)
    pub bound_port: std::sync::atomic::AtomicU16,
    /// v1.1 task inbox
    pub inbox: std::sync::Mutex<crate::inbox::Inbox>,
}

impl App {
    pub fn effective_port(&self) -> u16 {
        let p = self.bound_port.load(std::sync::atomic::Ordering::Relaxed);
        if p != 0 {
            p
        } else {
            self.cfg.port
        }
    }
}

pub type SharedApp = Arc<App>;

// ---------- errors ----------

pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    pub fn unauthorized() -> Self {
        Self { status: StatusCode::UNAUTHORIZED, code: "unauthorized", message: "missing or invalid token".into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, code: "not_found", message: msg.into() }
    }
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::CONFLICT, code: "conflict", message: msg.into() }
    }
    pub fn agent_unknown(agent: &str) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "agent_unknown", message: format!("unknown agent: {agent}") }
    }
    pub fn ssd_unmounted() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "ssd_unmounted",
            message: "project root is not mounted; refusing writes".into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, code: "internal", message: msg.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": {"code": self.code, "message": self.message}});
        (self.status, Json(body)).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

// ---------- auth ----------

/// Pure token check: `Authorization: Bearer <token>` header or `?token=` query.
pub fn check_auth(auth_header: Option<&str>, query: Option<&str>, token: &str) -> bool {
    if let Some(h) = auth_header {
        if let Some(t) = h.strip_prefix("Bearer ") {
            if !token.is_empty() && t.trim() == token {
                return true;
            }
        }
    }
    if let Some(q) = query {
        for pair in q.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                if !token.is_empty() && v == token {
                    return true;
                }
            }
        }
    }
    false
}

async fn auth_mw(
    State(app): State<SharedApp>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if path == "/api/v1/hooks/claude" {
        // localhost-only, token-free (Claude Code hook curl)
        if addr.ip().is_loopback() {
            return next.run(req).await;
        }
        return ApiError::unauthorized().into_response();
    }
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if check_auth(header, req.uri().query(), &app.cfg.token) {
        next.run(req).await
    } else {
        ApiError::unauthorized().into_response()
    }
}

// ---------- helpers ----------

fn ssd_guard(app: &App) -> ApiResult<()> {
    if app.cfg.project_root.is_dir() {
        Ok(())
    } else {
        Err(ApiError::ssd_unmounted())
    }
}

fn iso_from_epoch(secs: f64) -> String {
    DateTime::<Utc>::from_timestamp(secs as i64, 0)
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap())
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("task join: {e}")))
}

// ---------- handlers ----------

async fn health(State(app): State<SharedApp>) -> Json<Value> {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "ssd_mounted": app.cfg.project_root.is_dir(),
        "project_root": app.cfg.project_root,
        "uptime_s": app.started.elapsed().as_secs(),
    }))
}

async fn agents_list(State(app): State<SharedApp>) -> Json<Value> {
    let home = app.paths.home.clone();
    let list: Vec<Value> = agents::AGENTS
        .iter()
        .map(|a| {
            json!({
                "id": a.id,
                "label": a.label,
                "cmd": a.cmd,
                "resume_cmd": a.resume_cmd,
                "available": agents::which(agents::agent_bin(a), &home).is_some(),
            })
        })
        .collect();
    Json(json!(list))
}

async fn projects_list(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let app2 = Arc::clone(&app);
    let rows = blocking(move || {
        // No store_lock here: naming may call haiku (up to 60s) and the cache
        // save below merges instead of overwriting, so scans can run unlocked.
        let mut cache = CwdCache::load(&app2.paths.cwd_cache());
        let rows = stores::collect(&app2.paths, &mut cache, &app2.cfg.project_root);
        let reg = Registry::load(&app2.cfg.project_root);
        let namer = Namer::new(&app2.paths, app2.cfg.namer);
        let out: Vec<Value> = rows
            .iter()
            .map(|r| {
                // agent: registry first, else detected, else default (aaa logic)
                let mut agent = reg
                    .get(&r.path)
                    .filter(|a| agents::get(a).is_some())
                    .map(String::from);
                if agent.is_none() {
                    agent = r
                        .det_agent
                        .clone()
                        .filter(|a| agents::get(a).is_some());
                }
                let agent = agent.unwrap_or_else(|| "claude".to_string());
                let title = if r.det_path.is_empty() {
                    String::new()
                } else {
                    namer.name_for(
                        &mut cache,
                        r.det_agent.as_deref().unwrap_or(""),
                        Path::new(&r.det_path),
                    )
                };
                json!({
                    "path": r.path,
                    "name": r.name,
                    "mtime": iso_from_epoch(r.mtime),
                    "dir_size": r.dir_size,
                    "ctx_size": r.ctx_size.unwrap_or(0),
                    "agent": agent,
                    "session_title": if title.is_empty() { Value::Null } else { Value::String(title) },
                })
            })
            .collect();
        if cache.dirty() {
            let _g = app2.store_lock.lock().unwrap();
            cache.save();
        }
        out
    })
    .await?;
    Ok(Json(json!(rows)))
}

#[derive(Deserialize)]
struct CreateProject {
    name: Option<String>,
    agent: Option<String>,
}

async fn projects_create(
    State(app): State<SharedApp>,
    Json(body): Json<CreateProject>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let name = crate::slug::slugify(body.name.as_deref().unwrap_or(""));
    let name = if name.is_empty() { crate::slug::timestamp_name() } else { name };
    if let Some(agent) = &body.agent {
        if agents::get(agent).is_none() {
            return Err(ApiError::agent_unknown(agent));
        }
    }
    let dir = app.cfg.project_root.join(&name);
    if dir.exists() {
        return Err(ApiError::conflict(format!("already exists: {}", dir.display())));
    }
    std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(format!("mkdir: {e}")))?;
    if let Some(agent) = &body.agent {
        let mut reg = Registry::load(&app.cfg.project_root);
        reg.set(&dir.to_string_lossy(), agent)
            .map_err(|e| ApiError::internal(format!("registry: {e}")))?;
    }
    app.hub.projects_changed();
    // full project object (same shape as GET /projects rows); the client
    // uses `path` directly to open a session
    let agent = body.agent.clone().unwrap_or_else(|| "claude".to_string());
    let mtime = std::fs::metadata(&dir)
        .map(|m| stores::mtime_f(&m))
        .unwrap_or(0.0);
    Ok(Json(json!({
        "path": dir,
        "name": name,
        "mtime": iso_from_epoch(mtime),
        "dir_size": 0,
        "ctx_size": 0,
        "agent": agent,
        "session_title": Value::Null,
    })))
}

#[derive(Deserialize)]
struct DeleteProjects {
    paths: Vec<String>,
}

async fn projects_delete(
    State(app): State<SharedApp>,
    Json(body): Json<DeleteProjects>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let app2 = Arc::clone(&app);
    let results = blocking(move || {
        // deletion stays fully serialized (no LLM work in here, cost is small)
        let _g = app2.store_lock.lock().unwrap();
        let root_canon = std::fs::canonicalize(&app2.cfg.project_root)
            .unwrap_or_else(|_| app2.cfg.project_root.clone());
        let mut cache = CwdCache::load(&app2.paths.cwd_cache());
        let mut reg = Registry::load(&app2.cfg.project_root);
        let mut results = Vec::new();
        for p in &body.paths {
            let pb = std::path::PathBuf::from(p);
            // SAFETY: only a real, direct child directory of the project root
            // may be deleted. canonicalize resolves `..` and symlinks; we trust
            // ONLY the resolved parent — a purely syntactic `pb.parent() == root`
            // check is bypassable (`<root>/..` has parent `<root>` yet resolves
            // to the root's parent, i.e. the whole volume) and lets a symlinked
            // child point purge/rm anywhere. When the path does not resolve we
            // fall back to a strict syntactic check that forbids any `..`.
            let target_dir = match std::fs::canonicalize(&pb) {
                Ok(canon) => {
                    if canon.parent() != Some(root_canon.as_path()) || canon == root_canon {
                        results.push(json!({"path": p, "ok": false, "purged": []}));
                        continue;
                    }
                    canon
                }
                Err(_) => {
                    // Nonexistent (already gone, or a typo): allow a purge-only
                    // pass for a syntactically valid direct child, never `..`.
                    let has_parent_dir = pb
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir));
                    if has_parent_dir || pb.parent() != Some(app2.cfg.project_root.as_path()) {
                        results.push(json!({"path": p, "ok": false, "purged": []}));
                        continue;
                    }
                    pb.clone()
                }
            };
            let target = target_dir.to_string_lossy().into_owned();
            let purged: Vec<Value> = stores::purge(&app2.paths, &mut cache, &target)
                .into_iter()
                .map(|(label, count)| json!({"agent_label": label, "count": count}))
                .collect();
            let _ = reg.unset(p);
            let _ = reg.unset(&target);
            let rm_ok = if target_dir.exists() {
                std::fs::remove_dir_all(&target_dir).is_ok()
            } else {
                true
            };
            results.push(json!({"path": p, "ok": rm_ok, "purged": purged}));
        }
        cache.save();
        results
    })
    .await?;
    app.hub.projects_changed();
    Ok(Json(json!({"results": results})))
}

#[derive(Deserialize)]
struct SetProjectAgent {
    path: String,
    agent: String,
}

async fn projects_agent(
    State(app): State<SharedApp>,
    Json(body): Json<SetProjectAgent>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    if agents::get(&body.agent).is_none() {
        return Err(ApiError::agent_unknown(&body.agent));
    }
    let mut reg = Registry::load(&app.cfg.project_root);
    reg.set(&body.path, &body.agent)
        .map_err(|e| ApiError::internal(format!("registry: {e}")))?;
    app.hub.projects_changed();
    Ok(Json(json!({"ok": true})))
}

async fn sessions_list(State(app): State<SharedApp>) -> Json<Value> {
    let list: Vec<Value> = app.pool.list().iter().map(|s| s.to_json()).collect();
    Json(json!(list))
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct CreateSession {
    project_path: String,
    agent: String,
    #[serde(default)]
    resume: bool,
    /// v1.1: disable inbox auto-feed for this session
    #[serde(default = "default_true")]
    feed_inbox: bool,
}

async fn sessions_create(
    State(app): State<SharedApp>,
    Json(body): Json<CreateSession>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let agent = agents::get(&body.agent).ok_or_else(|| ApiError::agent_unknown(&body.agent))?;
    let dir = std::path::PathBuf::from(&body.project_path);
    if !dir.is_absolute() {
        return Err(ApiError::not_found("project_path must be absolute"));
    }
    if !dir.is_dir() {
        // create only under the project root (SSD guard already passed);
        // reject `..` components — starts_with is purely component-wise
        let no_parent_refs = dir
            .components()
            .all(|c| !matches!(c, std::path::Component::ParentDir));
        if no_parent_refs && dir.starts_with(&app.cfg.project_root) {
            std::fs::create_dir_all(&dir)
                .map_err(|e| ApiError::internal(format!("mkdir: {e}")))?;
        } else {
            return Err(ApiError::not_found(format!("no such directory: {}", dir.display())));
        }
    }
    let canon = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let canon_str = canon.to_string_lossy().into_owned();

    // resume: port of aaa launch_agent_in (find most recent session for cwd)
    let mut cmd = agent.cmd.to_string();
    let mut resume_id = None;
    if body.resume && agent.resume_cmd.is_some() {
        let app2 = Arc::clone(&app);
        let target = canon_str.clone();
        let agent_id = agent.id;
        let sid = blocking(move || {
            let mut cache = CwdCache::load(&app2.paths.cwd_cache());
            let sid = stores::find(&app2.paths, &mut cache, agent_id, &target);
            if cache.dirty() {
                let _g = app2.store_lock.lock().unwrap();
                cache.save();
            }
            sid
        })
        .await?;
        if !sid.is_empty() {
            if let Some(rc) = agents::build_resume_cmd(agent, &sid) {
                cmd = rc;
                resume_id = Some(sid);
            }
        }
    }

    let project_name = canon
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| canon_str.clone());
    let spec = SpawnSpec {
        project_path: canon_str.clone(),
        project_name: project_name.clone(),
        agent: agent.id.to_string(),
        title: project_name,
        cmd,
        resume_id,
        feed_inbox: body.feed_inbox,
    };
    let sess = app.pool.spawn(spec).map_err(ApiError::internal)?;

    // v1.1: start checkpoint for agent sessions (async; never blocks the API)
    if agent.id != "shell" && app.cfg.checkpoint.enabled {
        let auto_init = app.cfg.checkpoint.auto_init_git;
        let max_mb = app.cfg.checkpoint.auto_init_max_mb;
        let sess2 = Arc::clone(&sess);
        let dir = canon_str;
        tokio::task::spawn_blocking(move || {
            let dirp = Path::new(&dir);
            if crate::checkpoint::ensure_repo(dirp, auto_init, max_mb).is_ok() {
                let mut st = sess2.ckpt.lock().unwrap();
                if let Ok(Some(r)) =
                    crate::checkpoint::make_checkpoint(dirp, &sess2.id, &mut st, "start")
                {
                    sess2.meta.lock().unwrap().ckpt_start_ref = Some(r);
                    sess2.mark_dirty();
                }
            }
        });
    }
    Ok(Json(sess.to_json()))
}

fn get_session(app: &App, id: &str) -> ApiResult<Arc<crate::pool::Session>> {
    app.pool
        .get(id)
        .ok_or_else(|| ApiError::not_found(format!("no such session: {id}")))
}

/// Kill a live session and poll until the reader thread marks it exited.
/// Returns whether it actually exited within `tries * step_ms`.
async fn kill_and_wait(sess: &Arc<crate::pool::Session>, tries: u32, step_ms: u64) -> bool {
    sess.kill().await;
    for _ in 0..tries {
        if sess.state() == SState::Exited {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(step_ms)).await;
    }
    sess.state() == SState::Exited
}

#[derive(Deserialize)]
struct InputBody {
    text: String,
    #[serde(default)]
    enter: bool,
}

async fn session_input(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<InputBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    if sess.state() == SState::Exited {
        return Err(ApiError::conflict("session already exited"));
    }
    // Text and the submitting Return go in as two writes with a beat between
    // them. A TUI agent (Claude Code, Codex) reads its prompt through an input
    // widget: when a whole line arrives in one read together with the \r, the
    // widget takes the text but swallows the Return, leaving the message
    // sitting unsent in the box — exactly what the phone composer produces.
    // A plain shell is line-buffered and does not care either way.
    let text = body.text;
    if !text.is_empty() {
        sess.write_input(text.as_bytes())
            .map_err(|e| ApiError::internal(format!("pty write: {e}")))?;
    }
    if body.enter {
        if !text.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        sess.write_input(b"\r")
            .map_err(|e| ApiError::internal(format!("pty write: {e}")))?;
    }
    Ok(Json(json!({"ok": true})))
}

async fn session_kill(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    sess.kill().await;
    Ok(Json(json!({"ok": true})))
}

async fn session_delete(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    if sess.state() != SState::Exited {
        // best effort: the record is removed either way
        kill_and_wait(&sess, 25, 120).await;
    }
    app.pool.remove(&id);
    sess.remove_persisted(&app.pool.ctx);
    app.hub.session_removed(&id);
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct RenameBody {
    title: String,
}

async fn session_rename(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<RenameBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    {
        let mut meta = sess.meta.lock().unwrap();
        meta.title = body.title;
        meta.custom_title = true;
        meta.needs_name = false;
    }
    sess.mark_dirty();
    if sess.state() == SState::Exited {
        sess.persist(&app.pool.ctx);
    }
    Ok(Json(sess.to_json()))
}

async fn session_ports(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let pid = sess.meta.lock().unwrap().pid;
    let (Some(pid), true) = (pid, sess.state() != SState::Exited) else {
        return Ok(Json(json!([])));
    };
    let entries = blocking(move || crate::ports::listening_ports(pid)).await?;
    Ok(Json(json!(entries)))
}

async fn mac_permissions() -> ApiResult<Json<Value>> {
    let statuses = blocking(crate::perms::status_all).await?;
    Ok(Json(json!(statuses)))
}

#[derive(Deserialize)]
struct PermRequest {
    ids: Vec<String>,
}

async fn mac_permissions_request(
    Json(body): Json<PermRequest>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let (triggered, opened) = crate::perms::request(&body.ids);
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"triggered": triggered, "opened_settings": opened})),
    ))
}

async fn pair_handler(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let port = app.effective_port();
    let token = app.cfg.token.clone();
    let payload = blocking(move || crate::pair::build_payload(port, &token)).await?;
    Ok(Json(json!({"payload": payload})))
}

async fn hooks_claude(
    State(app): State<SharedApp>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let event = body
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let cwd = body.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
    let message = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
    if cwd.is_empty() {
        return Json(json!({"ok": false}));
    }
    let cwd_canon = stores::realpath(cwd);
    let mut matched = 0;
    for sess in app.pool.all() {
        let is_match = {
            let meta = sess.meta.lock().unwrap();
            meta.agent == "claude"
                && meta.state != SState::Exited
                && (meta.project_path == cwd || meta.project_path == cwd_canon)
        };
        if !is_match {
            continue;
        }
        matched += 1;
        match event {
            "Notification" => {
                let question = (!message.is_empty()).then(|| Question {
                    text: message.to_string(),
                    options: vec![],
                });
                {
                    let mut meta = sess.meta.lock().unwrap();
                    meta.state = SState::Waiting;
                    meta.hook_waiting = true;
                    meta.question = question.clone();
                    meta.needs_name = true;
                }
                sess.mark_dirty();
                // inbox auto-feed + deduplicated waiting push (v1.1)
                crate::waiting::on_waiting(&app, Arc::clone(&sess), question).await;
            }
            "Stop" | "SubagentStop" => {
                let mut meta = sess.meta.lock().unwrap();
                // reset the silence timer; heuristics take over from here
                meta.last_output_at = Utc::now();
                meta.last_output_inst = Some(Instant::now());
                meta.state = SState::Running;
                meta.hook_waiting = false;
                meta.question = None;
                meta.needs_name = true;
                drop(meta);
                sess.mark_dirty();
            }
            _ => {}
        }
    }
    Json(json!({"ok": true, "matched": matched}))
}

// ---------- v1.1: messages / checkpoint / inbox / upload ----------

#[derive(Deserialize)]
struct MsgQuery {
    #[serde(default)]
    after: u64,
    limit: Option<usize>,
}

async fn session_messages(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    axum::extract::Query(q): axum::extract::Query<MsgQuery>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let store = sess.msgs.lock().unwrap();
    let limit = q.limit.unwrap_or(200).min(1000);
    Ok(Json(json!({
        "supported": store.supported,
        "source": store.source,
        "last_seq": store.last_seq(),
        "messages": store.slice(q.after, limit),
    })))
}

async fn session_diff(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let (agent, project_path, start_ref) = {
        let meta = sess.meta.lock().unwrap();
        (meta.agent.clone(), meta.project_path.clone(), meta.ckpt_start_ref.clone())
    };
    let unsupported = Json(json!({"supported": false, "base": Value::Null, "files": []}));
    let Some(start_ref) = start_ref else { return Ok(unsupported) };
    if agent == "shell"
        || !app.cfg.checkpoint.enabled
        || !Path::new(&project_path).is_dir()
        || !crate::checkpoint::has_repo(Path::new(&project_path))
    {
        return Ok(unsupported);
    }
    let base = start_ref.clone();
    let files = blocking(move || crate::checkpoint::diff(Path::new(&project_path), &start_ref))
        .await?
        .map_err(ApiError::internal)?;
    Ok(Json(json!({"supported": true, "base": base, "files": files})))
}

#[derive(Deserialize)]
struct RollbackBody {
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    force: bool,
}

async fn session_rollback(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<RollbackBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    if !body.confirm {
        return Err(ApiError::conflict("rollback requires confirm:true"));
    }
    let (project_path, start_ref) = {
        let meta = sess.meta.lock().unwrap();
        (meta.project_path.clone(), meta.ckpt_start_ref.clone())
    };
    let Some(start_ref) = start_ref else {
        return Err(ApiError::conflict("session has no start checkpoint"));
    };
    if !Path::new(&project_path).is_dir() {
        return if app.cfg.project_root.is_dir() {
            Err(ApiError::not_found(format!("project dir missing: {project_path}")))
        } else {
            Err(ApiError::ssd_unmounted())
        };
    }
    if sess.state() != SState::Exited {
        if !body.force {
            return Err(ApiError::conflict(
                "session is still alive; pass force:true to kill it first",
            ));
        }
        if !kill_and_wait(&sess, 40, 150).await {
            return Err(ApiError::internal("could not stop the session"));
        }
    }
    let base = start_ref.clone();
    let (restored, deleted) =
        blocking(move || crate::checkpoint::rollback(Path::new(&project_path), &start_ref))
            .await?
            .map_err(ApiError::internal)?;
    Ok(Json(json!({
        "ok": true,
        "base": base,
        "restored_files": restored,
        "deleted_files": deleted,
    })))
}

#[derive(Deserialize)]
struct InboxQuery {
    path: String,
}

/// Inbox entries are keyed by the canonical project path so that the
/// auto-feed lookup (which uses the session's canonicalized project_path)
/// always matches, regardless of how the client spelled the path.
fn inbox_key(path: &str) -> String {
    stores::realpath(path)
}

async fn inbox_list(
    State(app): State<SharedApp>,
    axum::extract::Query(q): axum::extract::Query<InboxQuery>,
) -> ApiResult<Json<Value>> {
    let inbox = app.inbox.lock().unwrap();
    Ok(Json(json!(inbox.list(&inbox_key(&q.path)))))
}

#[derive(Deserialize)]
struct InboxAdd {
    path: String,
    text: String,
}

async fn inbox_add(
    State(app): State<SharedApp>,
    Json(body): Json<InboxAdd>,
) -> ApiResult<Json<Value>> {
    if !body.path.starts_with('/') {
        return Err(ApiError::not_found("path must be absolute"));
    }
    if body.text.trim().is_empty() {
        return Err(ApiError::conflict("text must not be empty"));
    }
    let key = inbox_key(&body.path);
    let entry = {
        let mut inbox = app.inbox.lock().unwrap();
        inbox.add(&key, body.text.trim())
    };
    app.hub.inbox_changed(&key);
    Ok(Json(serde_json::to_value(entry).unwrap_or(Value::Null)))
}

async fn inbox_delete(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let hit = {
        let mut inbox = app.inbox.lock().unwrap();
        inbox.remove(&id)
    };
    match hit {
        Some(path) => {
            app.hub.inbox_changed(&path);
            Ok(Json(json!({"ok": true})))
        }
        None => Err(ApiError::not_found(format!("no such inbox entry: {id}"))),
    }
}

pub const UPLOAD_LIMIT: usize = 50 * 1024 * 1024;

#[derive(Deserialize)]
struct UploadQuery {
    path: String,
    name: String,
}

async fn project_upload(
    State(app): State<SharedApp>,
    axum::extract::Query(q): axum::extract::Query<UploadQuery>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let dir = std::path::PathBuf::from(&q.path);
    let clean = dir.is_absolute()
        && dir
            .components()
            .all(|c| !matches!(c, std::path::Component::ParentDir));
    if !clean || !dir.is_dir() {
        return Err(ApiError::not_found(format!("no such project dir: {}", q.path)));
    }
    // uploads only ever land inside the project root
    let root_canon = std::fs::canonicalize(&app.cfg.project_root)
        .unwrap_or_else(|_| app.cfg.project_root.clone());
    let dir_canon = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    if !dir_canon.starts_with(&root_canon) || dir_canon == root_canon {
        return Err(ApiError::not_found(format!(
            "upload target must be a project under {}",
            app.cfg.project_root.display()
        )));
    }
    let dir = dir_canon;
    // keep only the file-name portion, slugified (dots survive slugify)
    let base = Path::new(&q.name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let mut name = crate::slug::slugify(base);
    if name.is_empty() {
        name = "file".to_string();
    }
    let inbox_dir = dir.join("_inbox");
    std::fs::create_dir_all(&inbox_dir)
        .map_err(|e| ApiError::internal(format!("mkdir _inbox: {e}")))?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    // anti-overwrite: -1, -2… before the extension
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let mut target = inbox_dir.join(format!("{ts}-{stem}{ext}"));
    let mut n = 0;
    while target.exists() {
        n += 1;
        target = inbox_dir.join(format!("{ts}-{stem}-{n}{ext}"));
    }
    let tmp = inbox_dir.join(format!(
        ".{}.part",
        target.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&tmp, &body).map_err(|e| ApiError::internal(format!("write: {e}")))?;
    std::fs::rename(&tmp, &target).map_err(|e| ApiError::internal(format!("rename: {e}")))?;
    Ok(Json(json!({"saved_path": target})))
}

// ---------- WS ----------

async fn ws_attach(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(sess) = app.pool.get(&id) else {
        return ApiError::not_found(format!("no such session: {id}")).into_response();
    };
    ws.on_upgrade(move |socket| attach_loop(app, sess, socket))
}

async fn attach_loop(app: SharedApp, sess: Arc<crate::pool::Session>, mut socket: WebSocket) {
    let (rows, cols) = {
        let meta = sess.meta.lock().unwrap();
        (meta.rows, meta.cols)
    };
    let hello = json!({"t": "hello", "session": sess.to_json(), "rows": rows, "cols": cols});
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }
    let (replay, mut rx) = sess.attach_snapshot(&app.pool.ctx);
    if socket.send(Message::Binary(replay.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                // Bytes chunk shared with every other attached client
                Ok(bytes) => {
                    if socket.send(Message::Binary(bytes)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // resync with a fresh full redraw
                    let replay = sess.build_replay(&app.pool.ctx);
                    if socket.send(Message::Binary(replay.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(data))) => {
                    let _ = sess.write_input(&data);
                }
                Some(Ok(Message::Text(text))) => {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        if v.get("t").and_then(|t| t.as_str()) == Some("resize") {
                            let cols = v.get("cols").and_then(|c| c.as_u64()).unwrap_or(0) as u16;
                            let rows = v.get("rows").and_then(|r| r.as_u64()).unwrap_or(0) as u16;
                            if cols >= 20 && rows >= 5 && cols <= 1000 && rows <= 500 {
                                sess.resize(cols, rows);
                            }
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}

async fn ws_events(State(app): State<SharedApp>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| events_loop(app, socket))
}

async fn events_loop(app: SharedApp, mut socket: WebSocket) {
    let mut rx = app.hub.subscribe();
    let sessions: Vec<Value> = app.pool.list().iter().map(|s| s.to_json()).collect();
    let snapshot = json!({"t": "snapshot", "sessions": sessions});
    if socket.send(Message::Text(snapshot.to_string().into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                // pre-serialized frame shared with every other subscriber
                Ok(text) => {
                    if socket.send(Message::Text(text)).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let sessions: Vec<Value> = app.pool.list().iter().map(|s| s.to_json()).collect();
                    let snap = json!({"t": "snapshot", "sessions": sessions});
                    if socket.send(Message::Text(snap.to_string().into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
}

// ---------- router ----------

pub fn router(app: SharedApp) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/agents", get(agents_list))
        .route("/api/v1/projects", get(projects_list).post(projects_create))
        .route("/api/v1/projects/delete", post(projects_delete))
        .route("/api/v1/projects/agent", post(projects_agent))
        .route("/api/v1/sessions", get(sessions_list).post(sessions_create))
        .route("/api/v1/sessions/{id}", delete(session_delete))
        .route("/api/v1/sessions/{id}/input", post(session_input))
        .route("/api/v1/sessions/{id}/kill", post(session_kill))
        .route("/api/v1/sessions/{id}/rename", post(session_rename))
        .route("/api/v1/sessions/{id}/ports", get(session_ports))
        .route("/api/v1/sessions/{id}/messages", get(session_messages))
        .route("/api/v1/sessions/{id}/diff", get(session_diff))
        .route("/api/v1/sessions/{id}/rollback", post(session_rollback))
        .route("/api/v1/inbox", get(inbox_list).post(inbox_add))
        .route("/api/v1/inbox/{id}", delete(inbox_delete))
        .route(
            "/api/v1/projects/upload",
            post(project_upload)
                .layer(axum::extract::DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/api/v1/sessions/{id}/attach", get(ws_attach))
        .route("/api/v1/events", get(ws_events))
        .route("/api/v1/mac/permissions", get(mac_permissions))
        .route("/api/v1/mac/permissions/request", post(mac_permissions_request))
        .route("/api/v1/pair", get(pair_handler))
        .route("/api/v1/hooks/claude", post(hooks_claude))
        .layer(middleware::from_fn_with_state(Arc::clone(&app), auth_mw))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_check() {
        let tok = "aaa_tk_deadbeef";
        assert!(check_auth(Some("Bearer aaa_tk_deadbeef"), None, tok));
        assert!(check_auth(None, Some("token=aaa_tk_deadbeef"), tok));
        assert!(check_auth(None, Some("x=1&token=aaa_tk_deadbeef&y=2"), tok));
        assert!(!check_auth(Some("Bearer wrong"), None, tok));
        assert!(!check_auth(Some("aaa_tk_deadbeef"), None, tok), "must use Bearer scheme");
        assert!(!check_auth(None, Some("token=wrong"), tok));
        assert!(!check_auth(None, None, tok));
        assert!(!check_auth(Some("Bearer "), None, ""), "empty token never matches");
    }
}
