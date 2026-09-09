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
    /// v1.15：归档的项目（列表 / 看板默认藏起来）
    /// v1.9 会话日志：所有出现过的会话，含已退出、已删除
    pub history: std::sync::Mutex<crate::history::History>,
    /// v1.10 日历：按天的 haiku 摘要
    /// v1.3 账号 plan 配额：5h / 7d 来自最近一次 statusLine 的 rate_limits，
    /// 按模型窗口（Fable）来自 quota.rs 对 claude.ai usage 接口的轮询
    pub plan_usage: std::sync::Mutex<Option<Value>>,
    /// Last known readability of the project root, refreshed by the health
    /// watcher. Requests read this instead of probing: `is_dir()` lies under a
    /// TCC denial (stat passes, `opendir` does not), and an honest probe costs
    /// a thread and a deadline — not something to do per request.
    pub root_state: std::sync::atomic::AtomicU8,
    /// `PUT /config` 已受理、self-exec 在倒计时：期间拒绝新建会话和第二个
    /// 配置写——409 检查到 exec 之间开出来的 PTY 会被无声杀掉。
    pub restarting: std::sync::atomic::AtomicBool,
    /// v1.16：约好了「没有会话在跑时重启」（POST /restart {when_idle}），tick 里兑现
    pub restart_when_idle: std::sync::atomic::AtomicBool,
    /// 启动时本二进制的 mtime。/health 拿它和磁盘上现在的比：不一样 = 有新构建
    /// 还没跑起来，客户端据此提示「需重启」。
    pub exe_mtime_at_start: Option<std::time::SystemTime>,
}

/// 现在磁盘上 aaa-daemon 二进制的 mtime（None = 读不到，当作没变）
pub fn exe_mtime() -> Option<std::time::SystemTime> {
    std::env::current_exe().ok()?.metadata().ok()?.modified().ok()
}

impl App {
    pub fn root_state(&self) -> crate::rootcheck::RootState {
        crate::rootcheck::RootState::from_u8(self.root_state.load(std::sync::atomic::Ordering::Relaxed))
    }
    pub fn set_root_state(&self, s: crate::rootcheck::RootState) {
        self.root_state.store(s.as_u8(), std::sync::atomic::Ordering::Relaxed);
    }
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

impl App {
    /// 换上新的 plan 配额；三个窗口有变化才广播 `usage` 帧。
    /// `carry_scoped`：新值缺 `model_scoped` 时沿用旧值（statusLine 来源用）。
    pub fn set_plan_usage(&self, mut plan: Value, carry_scoped: bool) {
        let changed = {
            let mut cur = self.plan_usage.lock().unwrap();
            if carry_scoped {
                crate::quota::carry_model_scoped(&mut plan, cur.as_ref());
            }
            // 只比配额本身，不比 updated_at，免得每次 statusline 都广播
            let same = cur.as_ref().is_some_and(|c| crate::quota::same_windows(c, &plan));
            *cur = Some(plan.clone());
            !same
        };
        if changed {
            self.hub.usage(&plan);
        }
    }
}

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
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "bad_request", message: msg.into() }
    }
    pub fn agent_unknown(agent: &str) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code: "agent_unknown", message: format!("unknown agent: {agent}") }
    }
    pub fn ssd_unmounted_with(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "ssd_unmounted",
            message: message.into(),
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
            let Some((key, value)) = pair.split_once('=') else { continue };
            if key == "token" && !token.is_empty()
                && percent_encoding::percent_decode_str(value).decode_utf8_lossy() == token
            { return true; }
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
    let _ = addr; // kept for future per-peer policy; every route needs the token now
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
    // 重启窗口期（PUT /config 落盘到 exec 之间）内存里的 project_root 是旧值：
    // 此刻放行创建/上传会在刚迁走的旧根上 create_dir_all——把旧根整个重造在
    // 系统盘，正是 SSD 守卫要防的事（审查 P0）。统一 409，客户端稍候重试。
    if app.restarting.load(std::sync::atomic::Ordering::SeqCst) {
        return Err(ApiError::conflict("daemon 正在重启（配置刚修改），稍候重试"));
    }
    let state = app.root_state();
    if state.is_ok() {
        Ok(())
    } else {
        // carry the fix, not just the symptom: whoever hits this is often
        // holding a phone and cannot see the Mac's log
        Err(ApiError::ssd_unmounted_with(crate::rootcheck::advice(state, &app.cfg.project_root)))
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
        // v1.22：客户端唯一的兼容闸门（PROTOCOL「版本兼容」）。语义搬家时 +1；
        // 纯新增可选字段不动它。2 = /projects 带 status/title/session_id/updated_at、
        // 会话带 asking_seq/checklist——客户端据此决定「直接画」还是「挂降级横幅」
        "schema": crate::SCHEMA,
        // kept as "mounted" for wire compatibility: to a client it has always
        // meant "usable". root_state says *why* when it is not.
        "ssd_mounted": app.root_state().usable(),
        "root_state": app.root_state().code(),
        "project_root": app.cfg.project_root,
        "uptime_s": app.started.elapsed().as_secs(),
        // 二进制被重新构建过、进程还是旧的：设置页据此亮「需重启」
        "update_pending": app.exe_mtime_at_start.is_some() && exe_mtime() != app.exe_mtime_at_start,
        // v1.16：约好了空闲时重启，还在等会话都停下
        "restart_scheduled": app.restart_when_idle.load(std::sync::atomic::Ordering::SeqCst),
        "restarting": app.restarting.load(std::sync::atomic::Ordering::SeqCst),
    }))
}

#[derive(Deserialize, Default)]
struct RestartBody {
    #[serde(default)]
    force: bool,
    /// v1.16：不是现在——等到没有会话在跑（都 waiting / exited）再重启；期间 /health
    /// 报 restart_scheduled。有活会话也不需要 force：等到它们都停在输入框再收
    #[serde(default)]
    when_idle: bool,
}

/// 有存活会话时能不能重启：不 force 一律拒绝并把它们的名字报出来，
/// 让客户端有话可说（「先结束这 3 个」）。
pub fn restart_blockers(alive: &[String], force: bool) -> Result<(), String> {
    if alive.is_empty() || force {
        Ok(())
    } else {
        Err(format!(
            "有 {} 个存活会话（{}）。重启会终止它们，先结束或勾选强制",
            alive.len(),
            alive.join("、")
        ))
    }
}

/// 「空闲时重启」兑现：没有会话在跑时由 tick 调用，等价于 /restart {force}
pub async fn restart_now(app: SharedApp) -> ApiResult<Value> {
    if app
        .restarting
        .compare_exchange(false, true, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst)
        .is_err()
    {
        return Err(ApiError::conflict("daemon 已在重启"));
    }
    app.restart_when_idle.store(false, std::sync::atomic::Ordering::SeqCst);
    let alive: Vec<Arc<crate::pool::Session>> = app.pool.list().into_iter().filter(|s| s.state() != SState::Exited).collect();
    let titles: Vec<String> = alive.iter().map(|s| s.meta.lock().unwrap().title.clone()).collect();
    eprintln!("restart: idle window reached, restarting ({} session(s) to resume)", alive.len());
    terminate_for_restart(&app, &alive, |p| p.to_string()).await;
    crate::daemon::restart_self_after_ms(600);
    Ok(json!({"ok": true, "restarting": true, "killed": titles, "note": "daemon 将在 1 秒内重启，客户端会自动重连"}))
}

/// `POST /restart`：托管环境（launchd / systemd）下退出让服务管理器拉起干净实例，
/// 游离运行时 spawn 新进程再退出（v1.11.1 起；此前是多线程里 execvp，在 macOS 上会
/// 撞进 malloc/GCD 的内部锁卡死）。所有 PTY 都是本进程的子进程，重启 = 全部终止；
/// 所以有存活会话时要 `force`，并且先把它们正经 kill 掉（屏幕回放留着），不让它们
/// 在进程退出时无声消失。
/// 重启前的收尾（`/restart` 与 `PUT /config {force}` 共用）：记下活着的项目会话
/// （终端除外）到 `resume_after_restart.json`——起来后自动 resume（daemon.rs）——
/// 然后一起终止、并行等退出：以前是一个个 kill 再各等最多 3s，十个会话要半分钟；
/// 现在总共就是最慢那一个的时间（SIGTERM 2s 后补 SIGKILL，上限约 3s）。
/// `path_after` 把会话的 project_path 映射成重启后的路径（迁移根时换前缀）。
async fn terminate_for_restart(
    app: &SharedApp,
    alive: &[Arc<crate::pool::Session>],
    path_after: impl Fn(&str) -> String,
) {
    let to_resume: Vec<Value> = alive
        .iter()
        .filter_map(|s| {
            let m = s.meta.lock().unwrap();
            (m.agent != "shell").then(|| json!({"project_path": path_after(&m.project_path), "agent": m.agent}))
        })
        .collect();
    let _ = std::fs::write(
        app.paths.state_dir().join("resume_after_restart.json"),
        serde_json::to_vec(&to_resume).unwrap_or_default(),
    );
    for s in alive {
        s.meta.lock().unwrap().user_killed = true;
    }
    let waits: Vec<_> = alive
        .iter()
        .cloned()
        .map(|s| tokio::spawn(async move { kill_and_wait(&s, 25, 120).await }))
        .collect();
    for w in waits {
        let _ = w.await;
    }
}

async fn restart(
    State(app): State<SharedApp>,
    body: Option<Json<RestartBody>>,
) -> ApiResult<Json<Value>> {
    let (force, when_idle) = body.map(|b| (b.force, b.when_idle)).unwrap_or((false, false));
    if when_idle {
        let running = app.pool.all().iter().filter(|s| s.state() == SState::Running).count();
        if running > 0 {
            app.restart_when_idle.store(true, std::sync::atomic::Ordering::SeqCst);
            return Ok(Json(json!({"ok": true, "scheduled": true, "running": running, "note": "等这些会话都停在输入框再重启（/health 的 restart_scheduled）"})));
        }
        // 此刻就没有在跑的：直接走 force 路径（waiting 的会话收掉、起来后自动 resume）
        return restart_now(app).await.map(Json);
    }
    if app
        .restarting
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(ApiError::conflict("daemon 已在重启"));
    }
    let alive: Vec<Arc<crate::pool::Session>> = app
        .pool
        .list()
        .into_iter()
        .filter(|s| s.state() != SState::Exited)
        .collect();
    let titles: Vec<String> = alive.iter().map(|s| s.meta.lock().unwrap().title.clone()).collect();
    if let Err(msg) = restart_blockers(&titles, force) {
        app.restarting.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(ApiError::conflict(msg));
    }
    terminate_for_restart(&app, &alive, |p| p.to_string()).await;
    crate::daemon::restart_self_after_ms(600);
    Ok(Json(json!({
        "ok": true,
        "restarting": true,
        "killed": titles,
        "note": "daemon 将在 1 秒内重启，客户端会自动重连"
    })))
}

async fn projects_list(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    if app.restarting.load(std::sync::atomic::Ordering::SeqCst) {
        // 迁根 / 重启窗口里别扫：目录可能正在搬，cwd 缓存正在改写（gpt-6 审阅指出这条路没上锁）
        return Err(ApiError::conflict("daemon 正在重启，稍候重试"));
    }
    // v1.22：这一项目此刻的样子（代表会话 / 五态 / 标题 / 排序时间）由 daemon 算一次，
    // 两端只画（PROTOCOL「版本兼容」）。此前两端各自从 /sessions 推，推法还不一样：
    // mac 取 updated_at 最大的活会话，Android 先按 agent 过滤再按「待回复<执行中<其它」
    // 排——同一个项目在两台设备上会显示不同的标题和状态。
    let agg = crate::history::project_aggregate(app.pool.list().iter().filter_map(|sess| {
        let m = sess.meta.lock().unwrap();
        // 终端不代表项目（PROTOCOL「终端」：不进项目列表、不参与主会话判定）
        (m.agent != "shell").then(|| crate::history::SessSnap {
            path: stores::realpath(&m.project_path),
            id: sess.id.clone(),
            agent: m.agent.clone(),
            title: m.title.clone(),
            updated_at: m.updated_at.unwrap_or(m.created_at).to_rfc3339_opts(SecondsFormat::Secs, true),
            status: crate::history::status_of(&m),
        })
    }));
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
            // 注册表就是项目名册：根目录下没登记的目录（顺手 clone 的仓库、
            // 杂物文件夹）不算项目。经 daemon 建的项目/会话都会登记。
            .filter(|r| reg.get(&r.path).is_some())
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
                // 会话那边的 key 是 canonicalize 过的，项目行这边是 read_dir 给的原始路径：
                // 项目根只要经过一个软链接（/Volumes 下很常见），两边就对不上，整列项目
                // 会全部显示成「没有活会话」。所以查表前统一走一次 realpath。
                let a = agg.get(&stores::realpath(&r.path));
                let live = a.and_then(|a| a.live.as_ref());
                json!({
                    "path": r.path,
                    "name": r.name,
                    "mtime": iso_from_epoch(r.mtime),
                    "dir_size": r.dir_size,
                    "agent": agent,
                    "session_title": if title.is_empty() { Value::Null } else { Value::String(title.clone()) },
                    // v1.22：登记过的项目才能 resume / 删（见下面补的「没登记但有活会话」那几行）
                    "registered": true,
                    // ↓ v1.22：daemon 算好的「这一项目此刻的样子」，客户端只画
                    // 代表会话：活着的那个；都退出了就是最近退出的那个（点它 resume）
                    "session_id": live
                        .map(|l| l.id.clone())
                        .or_else(|| a.and_then(|a| a.latest.as_ref().map(|l| l.id.clone()))),
                    // 五态与看板、CLI 同一句话（history::status_of）；没有活会话 = paused
                    "status": live.map(|l| l.status).unwrap_or("paused"),
                    "title": project_title(live.map(|l| l.title.as_str()), &title, &r.name),
                    // 排序键：该项目最新一条非终端会话的 updated_at（含已退出）；一个都没有 → 目录 mtime
                    "updated_at": a
                        .and_then(|a| a.latest.as_ref().map(|l| l.updated_at.clone()))
                        .unwrap_or_else(|| iso_from_epoch(r.mtime)),
                })
            })
            .collect();
        if cache.dirty() {
            let _g = app2.store_lock.lock().unwrap();
            cache.save();
        }
        (out, agg)
    })
    .await?;
    let (rows, agg) = rows;
    // 在别处 `aaa open` 开出来的目录：注册表里没有，但此刻有活会话——也得给一行，
    // 否则它无处可点（PROTOCOL「GUI 列表口径」）。v1.22 起由 daemon 补：mac 侧此前
    // 自己补一遍、Android 压根没补，同一台 daemon 在两端显示的行数都不一样。
    let seen: std::collections::HashSet<String> = rows
        .iter()
        .filter_map(|r| r["path"].as_str())
        .map(stores::realpath)
        .collect();
    let extra: Vec<Value> = agg
        .iter()
        .filter(|(path, a)| a.live.is_some() && !seen.contains(*path))
        .map(|(path, a)| {
            let l = a.live.as_ref().expect("filtered above");
            json!({
                "path": path,
                "name": Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone()),
                "mtime": l.updated_at,
                "dir_size": 0,
                "agent": l.agent,
                "session_title": Value::Null,
                // 没登记 = 不能 resume / 删项目，客户端据此收起那些菜单项
                "registered": false,
                "session_id": l.id,
                "status": l.status,
                "title": if l.title.trim().is_empty() { Value::String(path.clone()) } else { Value::String(l.title.clone()) },
                "updated_at": a.latest.as_ref().map(|x| x.updated_at.clone()).unwrap_or_else(|| l.updated_at.clone()),
            })
        })
        .collect();
    let mut rows = rows;
    rows.extend(extra);
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
    match std::fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(ApiError::conflict(format!("already exists: {}", dir.display()))),
        Err(e) => return Err(ApiError::internal(format!("mkdir: {e}"))),
    }
    if let Some(agent) = &body.agent {
        let _reg_lock = crate::registry::lock();
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
        "agent": agent,
        "session_title": Value::Null,
    })))
}

/// v1.26 恢复（2026-09-08 删过一次，理由是「给一个常量做服务发现」）：agent 表
/// 加回 agy 之后不再是常量，而「这台机器装没装」也只有 daemon 知道——客户端硬
/// 编码一份表就会给没装的 agent 开会话，然后拿到一屏 command not found。
/// `shell` 不在这张表里：终端是面板，不是 agent（PROTOCOL「终端」）。
async fn agents_list(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let home = app.paths.home.clone();
    let list = blocking(move || {
        agents::AGENTS
            .iter()
            .filter(|a| a.id != "shell")
            .map(|a| {
                json!({
                    "id": a.id,
                    "label": a.label,
                    "available": agents::which(agents::agent_bin(a), &home).is_some(),
                })
            })
            .collect::<Vec<Value>>()
    })
    .await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

/// 会话日志：所有出现过的会话（含已退出、已删除），最新在前。
///
/// 2026-09-08 差点被当成死路由删掉：两端确实只取 `/history/dashboard`。但看板**不收终端**，
/// 而历史账本是收的——删项目之后「记录还在吗」只有这里能看见（smoke 就这么验的）。
/// 它是这个账本唯一的读出口，留着。
async fn history_list(
    State(app): State<SharedApp>,
    axum::extract::Query(q): axum::extract::Query<HistoryQuery>,
) -> ApiResult<Json<Value>> {
    let list = app.history.lock().unwrap().list(q.limit.unwrap_or(200).min(crate::history::KEEP));
    Ok(Json(json!({"entries": list})))
}

/// v1.16.2：给没有清单的会话补清单（后台跑，立刻返回）
async fn history_backfill(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let missing = app
        .pool
        .all()
        .iter()
        .filter(|s| {
            let m = s.meta.lock().unwrap();
            m.agent == "claude" && m.summary.trim().is_empty()
        })
        .count();
    let app2 = Arc::clone(&app);
    tokio::task::spawn_blocking(move || crate::summary::backfill(&app2, 500));
    Ok(Json(json!({"ok": true, "started": true, "missing": missing})))
}

/// 看板：所有会话的进度（daemon 一次算好，两端只画）
async fn history_dashboard(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let entries = app.history.lock().unwrap().list(crate::history::KEEP);
    // 池子里的会话此刻的状态；不在池子里的一律 paused（不能点开）
    let mut live = std::collections::HashMap::new();
    for s in app.pool.list() {
        let m = s.meta.lock().unwrap();
        let status = crate::history::status_of(&m);
        let updated_at = m.updated_at.unwrap_or(m.created_at).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        live.insert(
            s.id.clone(),
            crate::history::LiveStatus { status, updated_at, permission: m.permission.clone() },
        );
    }
    let d = crate::history::dashboard(&entries, &live);
    Ok(Json(serde_json::to_value(d).unwrap_or_else(|_| json!({}))))
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
    let deleted_paths: Vec<String> = body.paths.iter().map(|p| stores::realpath(p)).collect();
    // 「删除项目 = 目录 + 全部会话」（手机上的删除入口一直是这么写的）：先把这些目录下
    // 的会话结束并摘出池子，再删目录。否则目录没了、PTY 还活着——cwd 成了幽灵，会话在
    // /sessions 里赖着，mac 侧栏还会为「有会话但没登记」的目录补一行，点进去无处可去。
    // 终端也一起收：它的 cwd 同样跟着目录消失。
    let victims: Vec<Arc<crate::pool::Session>> = app
        .pool
        .all()
        .into_iter()
        .filter(|s| {
            let path = s.meta.lock().unwrap().project_path.clone();
            deleted_paths.contains(&stores::realpath(&path))
        })
        .collect();
    let killed: Vec<String> = victims
        .iter()
        .map(|s| {
            let m = s.meta.lock().unwrap();
            if m.title.is_empty() { s.id.clone() } else { m.title.clone() }
        })
        .collect();
    for s in &victims {
        // 用户自己动的手：随后的 exited 不该弹通知
        s.meta.lock().unwrap().user_killed = true;
    }
    // 一起终止、并行等退出（与 /restart 同一套：总耗时 = 最慢那一个）
    let waits: Vec<_> = victims
        .iter()
        .cloned()
        .map(|s| tokio::spawn(async move { kill_and_wait(&s, 25, 120).await }))
        .collect();
    for w in waits {
        let _ = w.await;
    }
    for s in &victims {
        app.pool.remove(&s.id);
        s.remove_persisted(&app.pool.ctx);
        {
            // 日志里留着：先把最后一版同步进去，再盖删除戳
            let mut h = app.history.lock().unwrap();
            h.upsert(crate::history::entry_from(s));
            h.mark_deleted(&s.id);
            h.save_if_dirty();
        }
        app.hub.session_removed(&s.id);
    }
    let results = blocking(move || {
        // deletion stays fully serialized (no LLM work in here, cost is small)
        let _g = app2.store_lock.lock().unwrap();
        let root_canon = std::fs::canonicalize(&app2.cfg.project_root)
            .unwrap_or_else(|_| app2.cfg.project_root.clone());
        let mut cache = CwdCache::load(&app2.paths.cwd_cache());
        let _reg_lock = crate::registry::lock();
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
    for p in &deleted_paths {
        let mut h = app.history.lock().unwrap();
        h.mark_project_deleted(p);
        h.save_if_dirty();
    }
    app.hub.projects_changed();
    Ok(Json(json!({"results": results, "killed": killed})))
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
    /// 显式要求开新会话（默认幂等：同项目+同 agent 已有存活会话就返回它。
    /// 实测事故：双击「没反应」再点一次 → 两个进程 resume 同一个对话）
    #[serde(default)]
    fresh: bool,
}

/// 项目行的标题回退链：活会话的标题 → agent 存储读出的 `session_title` → 目录名。
/// 这条链此前三端各写一遍（PROTOCOL「一件事只算一次」）。
///
/// 活会话的标题**还是目录名**时让位给存储里那个（v1.34）：namer 还没轮到这个会话时
/// 它就是目录名，而存储那边已经算好了。两个都是目录名的话结果不变。
fn project_title(live: Option<&str>, store: &str, dir_name: &str) -> String {
    let store = store.trim();
    let live = live.map(str::trim).filter(|t| !t.is_empty() && !(*t == dir_name && !store.is_empty()));
    match live {
        Some(t) => t.to_string(),
        None if store.is_empty() => dir_name.to_string(),
        None => store.to_string(),
    }
}

/// 一条可继承标题的旧记录（池子里的会话、或会话日志里的一行）。
struct TitleCand {
    agent: String,
    title: String,
    custom_title: bool,
    resume_id: Option<String>,
    project_path: String,
    /// ISO-8601 UTC，秒精度——两个来源同一种写法，直接按字符串比新旧
    ts: String,
}

/// 从旧记录里挑一个标题：同一段对话（`resume_id`）优先，其次同一目录、同一 agent，
/// 同一档里取最新的一条。用户自己改的名只跟着同一段对话走——同目录另一段对话的
/// 自定义标题按到这一段头上，namer 就再也不会纠正它了。
fn pick_title(
    cands: &[TitleCand],
    resume_id: Option<&str>,
    project_path: &str,
    agent: &str,
) -> Option<(String, bool)> {
    let mut best: Option<(u8, &TitleCand)> = None;
    for c in cands {
        if c.agent != agent || c.title.trim().is_empty() {
            continue;
        }
        let rank = if resume_id.is_some() && c.resume_id.as_deref() == resume_id {
            2
        } else if c.project_path == project_path {
            1
        } else {
            continue;
        };
        if best.map(|(r, b)| (rank, &c.ts) > (r, &b.ts)).unwrap_or(true) {
            best = Some((rank, c));
        }
    }
    best.map(|(rank, c)| (c.title.trim().to_string(), rank == 2 && c.custom_title))
}

/// resume 出来的会话接着上一段的标题（v1.34）：池子里已退出的记录（重启后由
/// `restore_persisted` 读回来）加会话日志，就是「上一段」的全部去处。
fn carried_title(
    app: &SharedApp,
    resume_id: Option<&str>,
    project_path: &str,
    agent: &str,
) -> Option<(String, bool)> {
    let iso = |t: chrono::DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let mut cands: Vec<TitleCand> = app
        .pool
        .all()
        .iter()
        .map(|s| {
            let m = s.meta.lock().unwrap();
            TitleCand {
                agent: m.agent.clone(),
                title: m.title.clone(),
                custom_title: m.custom_title,
                resume_id: m.resume_id.clone(),
                project_path: m.project_path.clone(),
                ts: iso(m.updated_at.unwrap_or(m.created_at)),
            }
        })
        .collect();
    cands.extend(app.history.lock().unwrap().list(crate::history::KEEP).into_iter().map(|e| {
        TitleCand {
            agent: e.agent,
            title: e.title,
            custom_title: false,
            resume_id: None,
            project_path: e.project_path,
            ts: e.created_at,
        }
    }));
    pick_title(&cands, resume_id, project_path, agent)
}

async fn sessions_create(
    State(app): State<SharedApp>,
    Json(body): Json<CreateSession>,
) -> ApiResult<Json<Value>> {
    if app.restarting.load(std::sync::atomic::Ordering::SeqCst) {
        // 409 检查到 exec 之间开出来的 PTY 会被无声杀掉，宁可让客户端重试
        return Err(ApiError::conflict("daemon 正在重启（配置刚修改），稍候重试"));
    }
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
        // **两边都得是同一种写法**：项目根装载时已 canonicalize，客户端给的路径却可能
        // 走软链接（`/var` vs `/private/var`），裸 `starts_with` 会把根底下的目录判成外人。
        // 目录还不存在，只能拿它最深的那个存在的祖先去解。
        let mut probe = dir.as_path();
        let under_root = loop {
            match std::fs::canonicalize(probe) {
                Ok(real) => break real.starts_with(&app.cfg.project_root),
                Err(_) => match probe.parent() {
                    Some(p) => probe = p,
                    None => break false,
                },
            }
        };
        if no_parent_refs && under_root {
            std::fs::create_dir_all(&dir)
                .map_err(|e| ApiError::internal(format!("mkdir: {e}")))?;
        } else {
            return Err(ApiError::not_found(format!("no such directory: {}", dir.display())));
        }
    }
    let canon = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let canon_str = canon.to_string_lossy().into_owned();

    // 幂等：同项目 + 同 agent 已有存活会话时直接返回它，不再孵第二个进程
    // （两个进程 resume 同一对话还会互抢存储）。要并行开第二个用 fresh:true。
    if !body.fresh {
        if let Some(existing) = app.pool.all().into_iter().find(|s| {
            let meta = s.meta.lock().unwrap();
            meta.project_path == canon_str
                && meta.agent == body.agent
                && meta.state != SState::Exited
        }) {
            return Ok(Json(existing.to_json()));
        }
    }

    // 名册维护：开会话的目录必须在注册表里（项目列表以注册表为准）。
    // 已登记的不动——用户手动设过的 agent 不被这次会话的选择覆盖。
    // 登记失败就不开会话：一个跑着会话却不在名册里的项目治理不了。
    // 终端（shell）例外：它是常驻工具，在项目根或任意目录开都不算建项目。
    if agent.id != "shell" {
        let app2 = Arc::clone(&app);
        let target = canon_str.clone();
        let agent_id = agent.id;
        blocking(move || {
            let _reg_lock = crate::registry::lock();
            let mut reg = Registry::load(&app2.cfg.project_root);
            if reg.get(&target).is_none() {
                reg.set(&target, agent_id)
            } else {
                Ok(())
            }
        })
        .await?
        .map_err(|e| ApiError::internal(format!("项目登记失败: {e}")))?;
    }

    // resume: port of aaa launch_agent_in (find most recent session for cwd)
    let mut cmd = agents::spawn_cmd(agent);
    let mut resume_id = None;
    if body.resume && agent.resume_cmd.is_some() {
        let app2 = Arc::clone(&app);
        let target = canon_str.clone();
        let agent_id = agent.id;
        let sid = blocking(move || {
            let mut cache = CwdCache::load(&app2.paths.cwd_cache());
            let mut sid = stores::find(&app2.paths, &mut cache, &target, agent_id);
            if cache.dirty() {
                let _g = app2.store_lock.lock().unwrap();
                cache.save();
            }
            let _reg_lock = crate::registry::lock();
            let mut reg = Registry::load(&app2.cfg.project_root);
            if sid.is_empty() {
                // 项目根迁移后 agent 存储按旧 cwd 查不到会话；注册表第三列
                // 记的对话 id 是兜底（仅当登记的 agent 就是本次要开的）。
                // 先验证：坏 id 当场清掉并开新会话，不要每次 resume 都拿同一个
                // 已被 GC 的 id 去撞墙。
                if reg.get(&target) == Some(agent_id) {
                    let cand = reg.get_id(&target).unwrap_or_default().to_string();
                    if stores::id_exists(&app2.paths, agent_id, &cand) {
                        sid = cand;
                    } else {
                        let _ = reg.clear_id(&target);
                    }
                }
            } else {
                // 顺手把最新对话 id 写回注册表：迁移时就不依赖再扫一遍存储
                let _ = reg.set_id(&target, agent_id, &sid);
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
    // hooks（事件源，见 `hooks::ensure_agy_hooks` 的文档）：claude 走 `--settings` 片段，
    // agy 走它全局那份 `hooks.json` 里我们自己的一段。装不上照样开会话，只是 hooked=false。
    let (cmd, mut hooked) = match crate::hooks::ensure_settings(&app) {
        Ok(p) => crate::hooks::with_settings(cmd, agent, &p),
        Err(e) => {
            eprintln!("hooks settings unavailable ({e}); session runs without hooks");
            (cmd, false)
        }
    };
    if agent.id == "agy" {
        match crate::hooks::ensure_agy_hooks(&app) {
            Ok(()) => hooked = true,
            Err(e) => eprintln!("agy hooks unavailable ({e}); session falls back to screen diffing"),
        }
    }
    // 标题接着上一段（v1.34）：新会话的标题先用目录名占着，等 namer 起名。平时看不
    // 出来（新会话本来也没标题），但重启会把所有活会话 resume 一遍——那几十秒里整列
    // 项目都显示成文件夹名，赶上会话在这窗口里被收掉，错的那版还会落进会话日志。
    let (title, custom_title) = carried_title(&app, resume_id.as_deref(), &canon_str, agent.id)
        .unwrap_or_else(|| (project_name.clone(), false));
    let spec = SpawnSpec {
        project_path: canon_str.clone(),
        project_name: project_name.clone(),
        agent: agent.id.to_string(),
        title,
        custom_title,
        cmd,
        resume_id,
        feed_inbox: body.feed_inbox,
        hooked,
    };
    let sess = app.pool.spawn(spec).map_err(ApiError::internal)?;
    // 进度清单跟着对话走：resume 出来的新会话把同一对话上一份清单带过来（先找池子里
    // 已退出的同对话记录，再找会话日志），不用等第一轮跑完才重新有
    let resume_id = sess.meta.lock().unwrap().resume_id.clone();
    if let Some(rid) = resume_id {
        let from_pool = app.pool.all().into_iter().find_map(|s| {
            if s.id == sess.id {
                return None;
            }
            let m = s.meta.lock().unwrap();
            (m.resume_id.as_deref() == Some(rid.as_str()) && !m.summary.is_empty()).then(|| m.summary.clone())
        });
        let carried = from_pool.or_else(|| {
            app.history
                .lock()
                .unwrap()
                .list(crate::history::KEEP)
                .into_iter()
                .find(|e| e.project_path == canon_str && !e.summary.is_empty())
                .map(|e| e.summary)
        });
        if let Some(summary) = carried {
            sess.meta.lock().unwrap().summary = summary;
            sess.mark_dirty();
        }
    }

    Ok(Json(sess.to_json()))
}

/// Claude Code hook 回调（hooks.rs）。永远快速 200：hooks 是 async 的，Claude 不等我们，
/// 但也不该看到错误提示。找不到归属会话的事件丢弃。
async fn hook_event(
    State(app): State<SharedApp>,
    UrlPath(event): UrlPath<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Json<Value> {
    let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let sid = headers
        .get(crate::hooks::SESSION_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string);
    let Some(sess) = crate::hooks::resolve(&app, sid.as_deref(), &v) else {
        return Json(json!({}));
    };
    let applied = crate::hooks::apply(&sess, &event, &v, Instant::now());
    if applied.dirty {
        sess.mark_dirty();
    }
    if let Some(plan) = applied.plan {
        // statusLine 从不带按模型窗口；那是 quota.rs 轮询来的，别让它被清掉
        app.set_plan_usage(plan, true);
    }
    if let Some((dir, id)) = applied.learned_id {
        // 注册表第三列：迁移后 resume 的兜底，现在从事件里直接拿到
        let root = app.cfg.project_root.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _reg_lock = crate::registry::lock();
            let mut reg = Registry::load(&root);
            let _ = reg.set_id(&dir, "claude", &id);
        })
        .await;
    }
    if applied.entered_waiting {
        let app2 = Arc::clone(&app);
        let sess2 = Arc::clone(&sess);
        let _ = tokio::task::spawn_blocking(move || crate::feed::on_waiting(&app2, sess2)).await;
    }
    // 一轮回复结束：让 haiku 写一句这轮做了什么（summary.rs）。不等它——要跑几秒到一分钟
    if event == "Stop" {
        let app2 = Arc::clone(&app);
        tokio::task::spawn_blocking(move || crate::summary::on_turn_done(&app2, sess));
    }
    Json(json!({}))
}

/// 账号 plan 配额（statusLine 的 rate_limits）：还没有任何会话转来时 `plan` 为 null
async fn usage_get(State(app): State<SharedApp>) -> Json<Value> {
    let plan = app.plan_usage.lock().unwrap().clone();
    Json(json!({ "plan": plan }))
}

/// 会话里发布过的 Artifact（报告 / 原型 / 图的链接），来自 transcript 的 Artifact 工具调用
async fn session_artifacts(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let list = sess.msgs.lock().unwrap().artifacts.clone();
    Ok(Json(json!({ "artifacts": list })))
}

/// v1.17 详情屏：一次给齐「消息流里翻不出来」的四样东西——子代理、后台任务、上传、技能。
/// 前三样从 transcript 解析出来的台账里取（messages.rs），上传直接看项目的 `_inbox/`。
/// 手机的详情屏和 mac 的详情栏画的都是它，所以口径只有一份。
async fn session_detail(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let (project_path, created_at) = {
        let m = sess.meta.lock().unwrap();
        (m.project_path.clone(), m.created_at)
    };
    // 锁序约定：meta 与 msgs 不同时持有
    let since = created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let (subagents, background, skills) = {
        let ms = sess.msgs.lock().unwrap();
        (ms.subagents.clone(), ms.background_tasks(Some(&since)), ms.skills.clone())
    };
    let uploads = blocking(move || list_uploads(&project_path)).await?;
    Ok(Json(json!({
        "subagents": subagents,
        "background_tasks": background,
        "uploads": uploads,
        "skills": skills,
    })))
}

/// 项目 `_inbox/`（`POST /projects/upload` 的落点）里的文件，新的在前，最多 200 个。
/// 目录不存在（从没传过东西）= 空列表，不是错误。
fn list_uploads(project_path: &str) -> Vec<Value> {
    let dir = Path::new(project_path).join("_inbox");
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<(std::time::SystemTime, Value)> = Vec::new();
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        // 上传是「先写 .part 再 rename」，半截的不该露出来
        if name.starts_with('.') {
            continue;
        }
        let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        out.push((
            mtime,
            json!({
                "name": name,
                "path": e.path().to_string_lossy(),
                "size": meta.len(),
                "ts": iso_from_epoch(mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as f64),
            }),
        ));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.truncate(200);
    out.into_iter().map(|(_, v)| v).collect()
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

/// 手机 composer 发来的文本怎么写进 PTY。单行原样；**多行**在 TUI 开了 bracketed
/// paste（DECSET 2004，Claude Code 一直开着）时包成一次粘贴，行尾归一成 CR——
/// 否则每个换行都是一次 Return，消息在第一行就被提交了。TUI 没开 2004 时（纯 shell）
/// 只做 CR 归一：那本来就是一行一条命令。
pub fn encode_input(text: &str, bracketed: bool) -> Vec<u8> {
    let multi = text.contains('\n') || text.contains('\r');
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    if multi && bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(body.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        body.into_bytes()
    }
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
    let bracketed = {
        let guard = sess.parser.lock().unwrap();
        guard.as_ref().is_some_and(|p| p.screen().bracketed_paste())
    };
    let text = body.text;
    // 新项目第一屏是信任对话框：这时写进去的字会被对话框吞掉（甚至替用户按下高亮的
    // 「No, exit」）。整句消息改进收件箱，tick 里的投喂在对话框被接受后立刻送达。
    if body.enter && !text.trim().is_empty() && crate::feed::trust_dialog_up(&sess) {
        let (project_path, agent) = {
            let m = sess.meta.lock().unwrap();
            (m.project_path.clone(), m.agent.clone())
        };
        if agent == "claude" {
            {
                let mut inbox = app.inbox.lock().unwrap();
                inbox.add(&project_path, text.trim());
            }
            app.hub.inbox_changed(&project_path);
            return Ok(Json(json!({"ok": true, "queued": true})));
        }
    }
    if !text.is_empty() {
        sess.write_input(&encode_input(&text, bracketed))
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

#[derive(Deserialize)]
struct AnswerBody {
    answers: Vec<crate::answer::Answer>,
}

/// Answer the AskUserQuestion form the session is waiting on. The client
/// sends choices (option indexes / free text per question); the daemon
/// turns them into the dialog's keystrokes and confirms the dialog closed.
async fn session_answer(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<AnswerBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    if sess.state() == SState::Exited {
        return Err(ApiError::conflict("session already exited"));
    }
    let (agent, since) = {
        let meta = sess.meta.lock().unwrap();
        (
            meta.agent.clone(),
            meta.created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        )
    };
    if agent != "claude" {
        return Err(ApiError::conflict("only claude sessions carry structured questions"));
    }
    let spec = {
        let store = sess.msgs.lock().unwrap();
        store
            .pending_question(Some(&since))
            .and_then(|m| m.question.clone())
            .ok_or_else(|| ApiError::conflict("no question is waiting for an answer"))?
    };
    crate::answer::validate(&spec, &body.answers).map_err(ApiError::bad_request)?;
    let steps = crate::answer::plan(&spec, &body.answers);
    crate::answer::drive(&sess, steps).await.map_err(ApiError::conflict)?;
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

#[derive(Deserialize)]
struct KillAll {
    /// 默认只收在跑的；false = 连停在输入框等你说话的一起收
    #[serde(default = "default_true")]
    running_only: bool,
}

/// 紧急制动：一把收掉所有还活着的**项目**会话（终端不收——它不烧配额，
/// 而且十有八九是你自己开着在用）。会话留在池子里，屏幕能回放，跟一个个按 ✕ 完全一样。
///
/// 与 `/restart` 的区别：那个是把 daemon 重启（起来还会自动 resume），这个只是刹车。
async fn sessions_kill_all(
    State(app): State<SharedApp>,
    body: Option<Json<KillAll>>,
) -> ApiResult<Json<Value>> {
    let running_only = body.map(|b| b.running_only).unwrap_or(true);
    let victims: Vec<Arc<crate::pool::Session>> = app
        .pool
        .list()
        .into_iter()
        .filter(|s| {
            let m = s.meta.lock().unwrap();
            m.agent != "shell"
                && m.state != SState::Exited
                && (!running_only || m.state == SState::Running)
        })
        .collect();
    let killed: Vec<String> = victims.iter().map(|s| s.meta.lock().unwrap().title.clone()).collect();
    // 并行收，总耗时 = 最慢那一个（与 /restart 同一套）
    let waits: Vec<_> = victims
        .iter()
        .cloned()
        .map(|s| tokio::spawn(async move { kill_and_wait(&s, 25, 120).await }))
        .collect();
    for w in waits {
        let _ = w.await;
    }
    Ok(Json(json!({"ok": true, "count": killed.len(), "killed": killed})))
}

/// 把池子里已经退出的会话记录一次清干净。**只动记录**：项目目录、agent 存储、
/// 会话日志里的条目都不碰（日志那边照旧盖 `deleted_at`）。
/// 存在的理由：几十个会话跑上几天，池子里几百条已退出的记录，看板每次都要连它们一起
/// 反序列化和排版；此前想清只能一条条 `DELETE /sessions/:id`，或者去删整个项目目录。
async fn sessions_clean_exited(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    let gone: Vec<Arc<crate::pool::Session>> = app
        .pool
        .list()
        .into_iter()
        .filter(|s| s.state() == SState::Exited)
        .collect();
    for sess in &gone {
        app.pool.remove(&sess.id);
        sess.remove_persisted(&app.pool.ctx);
        let mut h = app.history.lock().unwrap();
        h.upsert(crate::history::entry_from(sess));
        h.mark_deleted(&sess.id);
        app.hub.session_removed(&sess.id);
    }
    if !gone.is_empty() {
        app.history.lock().unwrap().save_if_dirty();
    }
    Ok(Json(json!({"ok": true, "removed": gone.len()})))
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
    {
        // 日志里留着：先把最后一版同步进去，再盖删除戳
        let mut h = app.history.lock().unwrap();
        h.upsert(crate::history::entry_from(&sess));
        h.mark_deleted(&id);
        h.save_if_dirty();
    }
    app.hub.session_removed(&id);
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct PermissionBody {
    /// allow | deny
    behavior: String,
}

/// v1.16：替用户答权限对话框。allow = 按对话框的第 1 项（Yes），deny = Esc（No，回到输入框
/// 让用户说要怎么改）。答完等对话框消失（PostToolUse / Stop 会清掉 meta.permission）；
/// 2 秒还没消失就报冲突，让用户去终端里看
async fn session_permission(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<PermissionBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    if sess.state() == SState::Exited {
        return Err(ApiError::conflict("session already exited"));
    }
    if sess.meta.lock().unwrap().permission.is_none() {
        return Err(ApiError::conflict("没有权限对话框在等（可能已经在终端里答过了）"));
    }
    let steps = crate::answer::permission_steps(&body.behavior).map_err(ApiError::bad_request)?;
    for (bytes, pause) in steps {
        sess.write_input(&bytes).map_err(|e| ApiError::internal(e.to_string()))?;
        tokio::time::sleep(std::time::Duration::from_millis(pause)).await;
    }
    for _ in 0..20 {
        if sess.meta.lock().unwrap().permission.is_none() {
            return Ok(Json(json!({"ok": true})));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(ApiError::conflict("对话框还在：去终端里看一眼"))
}

#[derive(Deserialize)]
struct ChecklistBody {
    /// 清单项的文字（看板卡片里那一行）
    text: String,
    done: bool,
    /// v1.22：这是清单里的第几项（与 `history::parse_checklist` / 会话对象上的
    /// `checklist` 同下标）。给了就只翻这一条，文字只用来核对；不给（老客户端）
    /// 退回「只翻第一条匹配的」。以前每一条同文的都跟着翻——清单里出现两条一样
    /// 的话，点一条勾掉两条。
    #[serde(default)]
    index: Option<usize>,
}

/// v1.16：看板上直接勾 / 取消勾。改 summary 里那一行，并记进 checklist_overrides：
/// haiku 下一轮重写清单时按它盖回去，手工勾的不会被重写冲掉
async fn session_checklist(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<ChecklistBody>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::bad_request("text 不能为空"));
    }
    {
        let mut meta = sess.meta.lock().unwrap();
        let next = crate::summary::set_item_at(&meta.summary, body.index, &text, body.done);
        if next == meta.summary && !meta.summary.contains(&text) {
            return Err(ApiError::not_found("清单里没有这一项"));
        }
        meta.summary = next;
        meta.checklist_overrides.insert(text.clone(), body.done);
    }
    sess.mark_dirty();
    if sess.state() == SState::Exited {
        sess.persist(&app.pool.ctx);
    }
    {
        let mut h = app.history.lock().unwrap();
        h.upsert(crate::history::entry_from(&sess));
        h.save_if_dirty();
    }
    Ok(Json(json!({"ok": true, "text": text, "done": body.done})))
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
        meta.touch();
    }
    sess.mark_dirty();
    if sess.state() == SState::Exited {
        sess.persist(&app.pool.ctx);
    }
    Ok(Json(sess.to_json()))
}

/// 屏幕文本（见 pool::screen_text）。没起 parser 的会话（刚建 / 已退出未回放）给空。
async fn session_screen(
    State(app): State<SharedApp>,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<Json<Value>> {
    let sess = get_session(&app, &id)?;
    let mut guard = sess.parser.lock().unwrap();
    let Some(parser) = guard.as_mut() else {
        return Ok(Json(json!({"text": "", "alternate_screen": false})));
    };
    let text = crate::pool::screen_text(parser, SCREEN_TEXT_SCROLLBACK);
    let alt = parser.screen().alternate_screen();
    Ok(Json(json!({"text": text, "alternate_screen": alt})))
}

/// `/screen` 带多少行回滚：手机复制/抓链接够用，又不至于一次抠出几万行。
const SCREEN_TEXT_SCROLLBACK: usize = 500;

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

// ── 配置读写（写盘 + 自我重启，不做热更新） ─────────────────────────────────

async fn config_get(State(app): State<SharedApp>) -> ApiResult<Json<Value>> {
    // 以磁盘为准：可能已有改动写入、等待重启生效
    let disk = crate::config::load_or_create(&app.paths.config_path())
        .map_err(|e| ApiError::internal(format!("读配置: {e}")))?;
    Ok(Json(json!({
        "port": disk.port,
        "token": disk.token,
        "project_root": disk.project_root,
    })))
}

#[derive(Deserialize)]
struct ConfigPut {
    port: Option<u16>,
    token: Option<String>,
    project_root: Option<String>,
    /// project_root 变化时是否迁移（整根移动 + 会话 / 日志 / Claude 存储全部改指向）
    #[serde(default)]
    migrate: bool,
    /// 有存活会话时不再 409：像 `/restart {force}` 一样先正经结束它们，重启后按
    /// **新**路径自动 resume（v1.12）。没有它，迁移前得手动把每个会话关掉。
    #[serde(default)]
    force: bool,
}

async fn config_put(
    State(app): State<SharedApp>,
    Json(body): Json<ConfigPut>,
) -> ApiResult<Json<Value>> {
    // 单飞：第二个并发配置写直接拒绝（两次迁移抢同一个根、两个 exec 排队
    // 都是灾难）。失败路径统统把标志放回去。
    if app
        .restarting
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err(ApiError::conflict("已有一次配置修改在进行（daemon 即将重启）"));
    }
    let unlock = || app.restarting.store(false, std::sync::atomic::Ordering::SeqCst);
    // 重启会杀掉所有 PTY：有存活会话时默认拒绝，绝不悄悄断人家的 agent；
    // `force` 才像 /restart 一样先正经结束、重启后自动 resume
    let live: Vec<Arc<crate::pool::Session>> = app
        .pool
        .list()
        .into_iter()
        .filter(|s| s.state() != SState::Exited)
        .collect();
    let titles: Vec<String> = live.iter().map(|s| s.meta.lock().unwrap().title.clone()).collect();
    let mut restart_anyway = false;
    let result: ApiResult<(Option<crate::migrate::Report>, Config)> = async {
        if !live.is_empty() && !body.force {
            return Err(ApiError::conflict(format!(
                "有 {} 个存活会话（{}）。改配置需要重启 daemon，先终止它们（或带 force）",
                live.len(),
                titles.join("、")
            )));
        }
        let cfg_path = app.paths.config_path();
        let mut cfg = crate::config::load_or_create(&cfg_path)
            .map_err(|e| ApiError::internal(format!("读配置: {e}")))?;
        let mut migrated: Option<crate::migrate::Report> = None;
        if let Some(root) = &body.project_root {
            let new_root = std::path::PathBuf::from(root);
            if !new_root.is_absolute() {
                return Err(ApiError::conflict("project_root 必须是绝对路径"));
            }
            if new_root != cfg.project_root {
                if body.migrate {
                    let old_root = cfg.project_root.clone();
                    // 注定失败的（跨卷 / 目标非空 / 嵌套 / 拼错）在收会话**之前**就拒绝，
                    // 别把人家跑着的会话杀了才发现搬不动（gpt-6 审阅指出）
                    crate::migrate::preflight(&old_root, &new_root).map_err(ApiError::conflict)?;
                    // 先把活着的会话结束掉（PTY 的 cwd 马上要搬走），resume 清单按新路径记
                    if !live.is_empty() {
                        let (o, n) = (old_root.clone(), new_root.clone());
                        terminate_for_restart(&app, &live, move |p| {
                            crate::migrate::reroot(p, &o, &n).unwrap_or_else(|| p.to_string())
                        })
                        .await;
                    }
                    let app2 = Arc::clone(&app);
                    let (o, n) = (old_root.clone(), new_root.clone());
                    let rep = match blocking(move || {
                        // 与 purge / collect 同一把锁：cwd 缓存和 Claude 目录都在动
                        let _g = app2.store_lock.lock().unwrap();
                        crate::migrate::migrate_root(&app2.paths, &o, &n)
                    })
                    .await?
                    {
                        Ok(rep) => rep,
                        Err(e) => {
                            // 过了 preflight 还失败（rename 本身出错）：会话已经收了，
                            // 不能白杀——resume 清单改回旧路径，照常重启让它们回来
                            if !live.is_empty() {
                                terminate_for_restart(&app, &live, |p| p.to_string()).await;
                            }
                            restart_anyway = true;
                            return Err(ApiError::conflict(format!("{e}（会话已结束，daemon 将重启并按原路径 resume）")));
                        }
                    };
                    // 内存里的会话也改指向：重启前若再 persist，别把旧路径写回去
                    for s in app.pool.all() {
                        let mut m = s.meta.lock().unwrap();
                        if let Some(np) = crate::migrate::reroot(&m.project_path, &old_root, &new_root) {
                            m.project_path = np;
                        }
                    }
                    migrated = Some(rep);
                } else if !new_root.is_dir() {
                    return Err(ApiError::conflict(format!(
                        "目录不存在：{}（或选择迁移现有项目）",
                        new_root.display()
                    )));
                }
                cfg.project_root = new_root;
            }
        }
        if let Some(p) = body.port {
            if p == 0 {
                return Err(ApiError::conflict("端口不能为 0"));
            }
            cfg.port = p;
        }
        if let Some(t) = &body.token {
            if t.trim().is_empty() {
                return Err(ApiError::conflict("token 不能为空"));
            }
            cfg.token = t.trim().to_string();
        }
        crate::config::write_config(&cfg_path, &cfg)
            .map_err(|e| ApiError::internal(format!("写配置: {e}")))?;
        if migrated.is_none() && !live.is_empty() {
            // 没迁移只改端口 / token 也带了 force：照 /restart 的规矩收会话
            terminate_for_restart(&app, &live, |p| p.to_string()).await;
        }
        Ok((migrated, cfg))
    }
    .await;
    let (migrated, cfg) = match result {
        Ok(v) => v,
        Err(e) => {
            if restart_anyway {
                // 会话已收、清单已按旧路径重写：标志保持 true，起来后自动 resume
                crate::daemon::restart_self_after_ms(600);
            } else {
                unlock();
            }
            return Err(e);
        }
    };
    // 标志保持 true 直到进程退出：sessions_create 在此期间被拒
    crate::daemon::restart_self_after_ms(600);
    Ok(Json(json!({
        "ok": true,
        "migrated": migrated.is_some(),
        "report": migrated,
        "killed": titles,
        "restarting": true,
        "port": cfg.port,
        "note": "daemon 将在 1 秒内自动重启，客户端会自动重连"
    })))
}

// ---------- v1.1: messages / inbox / upload ----------

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

// ---------- 收件箱：项目的任务清单，Claude 空下来时 daemon 自动喂 ----------
//
// 2026-09-10 一度当成僵尸路由删掉（两端确实都不调），装回来了：它是收件箱**唯一**的
// 通用写入口——另一个写入者只有「信任对话框挡着屏幕时把整句话收下」那一条岔路。
// 删了它 `feed.rs` 就只剩那个角落能触发，smoke 里那条自动投喂的用例也没法再写。
// 真正待定的是整个「收件箱 + 自动投喂」要不要留：两端都没有入口，用户加不进任务。

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
    // 会话此刻就空着：立刻喂，不等下一轮结束（feed 读 ~/.claude.json，放阻塞线程）
    let app2 = Arc::clone(&app);
    let key2 = key.clone();
    let _ = tokio::task::spawn_blocking(move || crate::feed::on_added(&app2, &key2)).await;
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

// ---- v1.30 目录浏览（消息流 / 终端之外的第三种视图）----

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

/// 客户端给的路径 → 项目根底下一个真实存在的路径。出根 / 不存在一律 404，
/// 且**不回显解析细节**：这是唯一一处按客户端给的字符串去碰文件系统的读接口。
fn under_root(app: &App, path: &str) -> ApiResult<std::path::PathBuf> {
    crate::files::resolve(&app.cfg.project_root, path)
        .ok_or_else(|| ApiError::not_found(format!("no such path under the project root: {path}")))
}

async fn files_list(
    State(app): State<SharedApp>,
    axum::extract::Query(q): axum::extract::Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let dir = under_root(&app, &q.path)?;
    let listed = {
        let dir = dir.clone();
        blocking(move || crate::files::list_dir(&dir)).await?
    };
    let entries = listed.map_err(|e| ApiError::not_found(format!("no such directory: {e}")))?;
    Ok(Json(json!({
        "path": dir.to_string_lossy(),
        "parent": crate::files::parent_of(&app.cfg.project_root, &dir),
        "truncated": entries.len() >= crate::files::MAX_ENTRIES,
        "entries": entries,
    })))
}

async fn files_read(
    State(app): State<SharedApp>,
    axum::extract::Query(q): axum::extract::Query<PathQuery>,
) -> ApiResult<Json<Value>> {
    ssd_guard(&app)?;
    let file = under_root(&app, &q.path)?;
    let body = blocking(move || crate::files::read_file(&file))
        .await?
        .map_err(|e| ApiError::not_found(format!("cannot read: {e}")))?;
    Ok(Json(serde_json::to_value(body).unwrap_or_default()))
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
    // v1.23：这条连接自己占一个尺寸位。PTY 多大按**所有连着的客户端里最小的那个**算，
    // 不再是「最后 resize 的说了算」——手机在后台连着时一帧 resize 就把桌面那扇窗户按成
    // 手机宽，桌面上正在看的输出当场重排。断开时这一位要还回去（下面每条 break 都走 detach）。
    let attach_id = sess.attach_size_slot();
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
                                sess.attach_resize(attach_id, cols, rows);
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
    sess.detach(attach_id);
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
    // 心跳：overlay 网络（tailscale 等）下 TCP 半开连接可以静默数分钟，
    // 客户端收不到任何帧也不自知（审查 P1）。20s 一个 Ping——tungstenite
    // 自动回 Pong，客户端也把收到的 Ping 当「链路活着」重置读超时。
    let mut hb = tokio::time::interval(std::time::Duration::from_secs(20));
    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
            _ = hb.tick() => {
                if socket.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
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
        .route("/api/v1/history", get(history_list))
        .route("/api/v1/history/backfill", post(history_backfill))
        .route("/api/v1/sessions/{id}/permission", post(session_permission))
        .route("/api/v1/sessions/{id}/checklist", post(session_checklist))
        .route("/api/v1/history/dashboard", get(history_dashboard))
        .route("/api/v1/sessions", get(sessions_list).post(sessions_create))
        .route("/api/v1/sessions/kill_all", post(sessions_kill_all))
        .route("/api/v1/sessions/clean_exited", post(sessions_clean_exited))
        .route("/api/v1/sessions/{id}", delete(session_delete))
        .route("/api/v1/sessions/{id}/input", post(session_input))
        .route("/api/v1/sessions/{id}/answer", post(session_answer))
        .route("/api/v1/sessions/{id}/kill", post(session_kill))
        .route("/api/v1/sessions/{id}/rename", post(session_rename))
        .route("/api/v1/sessions/{id}/screen", get(session_screen))
        .route("/api/v1/sessions/{id}/messages", get(session_messages))
        .route(
            "/api/v1/projects/upload",
            post(project_upload)
                .layer(axum::extract::DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/api/v1/files", get(files_list))
        .route("/api/v1/files/read", get(files_read))
        .route("/api/v1/inbox", get(inbox_list).post(inbox_add))
        .route("/api/v1/inbox/{id}", delete(inbox_delete))
        .route("/api/v1/hooks/{event}", post(hook_event))
        .route("/api/v1/usage", get(usage_get))
        .route("/api/v1/sessions/{id}/artifacts", get(session_artifacts))
        .route("/api/v1/sessions/{id}/detail", get(session_detail))
        .route("/api/v1/sessions/{id}/attach", get(ws_attach))
        .route("/api/v1/events", get(ws_events))
        .route("/api/v1/mac/permissions", get(mac_permissions))
        .route("/api/v1/mac/permissions/request", post(mac_permissions_request))
        .route("/api/v1/pair", get(pair_handler))
        .route("/api/v1/config", get(config_get).put(config_put))
        .route("/api/v1/restart", post(restart))
        .layer(middleware::from_fn_with_state(Arc::clone(&app), auth_mw))
        .with_state(app)
}

#[cfg(test)]
mod tests {
    #[test]
    fn restart_refuses_live_sessions_unless_forced() {
        assert!(restart_blockers(&[], false).is_ok());
        let two = ["改登录页".to_string(), "aaa-ui".to_string()];
        let err = restart_blockers(&two, false).unwrap_err();
        assert!(err.contains("2 个") && err.contains("改登录页、aaa-ui"));
        assert!(restart_blockers(&two, true).is_ok());
    }

    #[test]
    fn multiline_input_is_a_bracketed_paste_only_when_the_tui_asked() {
        assert_eq!(encode_input("hi", true), b"hi".to_vec());
        assert_eq!(encode_input("a\nb", true), b"\x1b[200~a\rb\x1b[201~".to_vec());
        assert_eq!(encode_input("a\r\nb\n", true), b"\x1b[200~a\rb\r\x1b[201~".to_vec());
        // 没开 2004 的 shell：不包，只归一换行
        assert_eq!(encode_input("a\nb", false), b"a\rb".to_vec());
    }

    use super::*;

    #[test]
    fn project_row_title_falls_back_through_the_chain() {
        assert_eq!(project_title(Some("改登录页"), "旧名字", "proj"), "改登录页");
        // 活会话还叫目录名（namer 还没轮到它）：存储里算好的那个更像话
        assert_eq!(project_title(Some("proj"), "旧名字", "proj"), "旧名字");
        // 两个都只有目录名：结果不变
        assert_eq!(project_title(Some("proj"), "", "proj"), "proj");
        // 没有活会话
        assert_eq!(project_title(None, "旧名字", "proj"), "旧名字");
        assert_eq!(project_title(None, "", "proj"), "proj");
        assert_eq!(project_title(Some("   "), "", "proj"), "proj");
    }

    fn cand(agent: &str, title: &str, path: &str, rid: Option<&str>, ts: &str, custom: bool) -> TitleCand {
        TitleCand {
            agent: agent.into(),
            title: title.into(),
            custom_title: custom,
            resume_id: rid.map(String::from),
            project_path: path.into(),
            ts: ts.into(),
        }
    }

    /// 重启会把活会话整批 resume 一遍；新会话若拿目录名当标题，整列项目都变成
    /// 文件夹名，要等 namer 一个一个补回来。上一段的标题就在旧记录里。
    #[test]
    fn resumed_session_inherits_the_previous_title() {
        let c = vec![
            cand("claude", "改登录页", "/p/a", Some("conv1"), "2026-09-10T07:00:00Z", false),
            cand("claude", "另一段", "/p/a", Some("conv2"), "2026-09-10T09:00:00Z", false),
            cand("claude", "别的项目", "/p/b", Some("conv3"), "2026-09-10T10:00:00Z", false),
            cand("agy", "别的 agent", "/p/a", Some("conv1"), "2026-09-10T11:00:00Z", false),
        ];
        // 同一段对话优先，哪怕同目录有更新的另一段
        assert_eq!(pick_title(&c, Some("conv1"), "/p/a", "claude"), Some(("改登录页".into(), false)));
        // 认不出对话（首次重启、老记录没有 resume_id）：同目录最新的那条
        assert_eq!(pick_title(&c, None, "/p/a", "claude"), Some(("另一段".into(), false)));
        // 没有旧记录 → 交给调用方用目录名兜底
        assert_eq!(pick_title(&c, None, "/p/c", "claude"), None);
        // 空标题的旧记录不算数
        let empty = vec![cand("claude", "   ", "/p/a", Some("conv1"), "2026-09-10T07:00:00Z", false)];
        assert_eq!(pick_title(&empty, Some("conv1"), "/p/a", "claude"), None);
    }

    /// 用户自己改的名只跟着同一段对话走：同目录另一段对话的自定义标题按到这一段
    /// 头上，namer 就再也不会纠正它了。
    #[test]
    fn custom_title_follows_only_the_same_conversation() {
        let c = vec![cand("claude", "我起的名", "/p/a", Some("conv1"), "2026-09-10T07:00:00Z", true)];
        assert_eq!(pick_title(&c, Some("conv1"), "/p/a", "claude"), Some(("我起的名".into(), true)));
        assert_eq!(pick_title(&c, None, "/p/a", "claude"), Some(("我起的名".into(), false)));
    }

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
