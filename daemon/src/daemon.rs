//! CLI entry + daemon run loop.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::{App, SharedApp};
use crate::cache::CwdCache;
use crate::events::EventHub;
use crate::namer::Namer;
use crate::paths::Paths;
use crate::pool::{PoolCtx, SessionPool, State};

const USAGE: &str = "\
aaa-daemon <command>

commands:
  run                       前台运行 daemon
  service install           写 launchd plist 并 launchctl load
  service uninstall         launchctl unload 并删除 plist
  service status            查看服务状态
  perms status              macOS 权限体检 (只读)
  perms request-all         逐项触发授权弹窗 (弹窗出现在 Mac 屏幕上)
";

/// `messages_changed` 两帧之间的最小间隔（PROTOCOL「/events」：≥500ms）。
/// 尾随节拍是 250ms，所以这个节流必须显式做。
const MSG_EVT_MIN: Duration = Duration::from_millis(500);

pub fn main_entry() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match (cmd, args.get(1).map(String::as_str)) {
        ("run", _) => run(),
        ("service", Some("install")) => {
            let paths = Paths::from_env();
            if let Err(e) = crate::service::install(&paths) {
                eprintln!("service install failed: {e}");
                std::process::exit(1);
            }
        }
        ("service", Some("uninstall")) => {
            let paths = Paths::from_env();
            if let Err(e) = crate::service::uninstall(&paths) {
                eprintln!("service uninstall failed: {e}");
                std::process::exit(1);
            }
        }
        ("service", Some("status")) => {
            let paths = Paths::from_env();
            if let Err(e) = crate::service::status(&paths) {
                eprintln!("service status failed: {e}");
                std::process::exit(1);
            }
        }
        ("perms", Some("status")) => crate::perms::cli_status(),
        ("perms", Some("request-all")) => crate::perms::cli_request_all(),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}

/// 重启 daemon：
/// - launchd (macOS) 或 systemd (Linux) 托管环境下：KeepAlive / Restart=always 会在进程退出后
///   立即拉起全新的干净实例。直接 exit(0) 是最可靠的方式——在 macOS 多线程/Tokio 运行时中
///   直接调用 `execvp` 容易遭遇 malloc/GCD 内部锁死锁，且无法平滑重载刚被重新签名的二进制。
/// - 游离/终端手动运行环境下：使用 `posix_spawn` (`Command::spawn`) 拉起新进程后再退出当前进程。
pub fn restart_self_after_ms(ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        let is_supervised = std::env::var("XPC_SERVICE_NAME").is_ok()
            || std::env::var("INVOCATION_ID").is_ok();
        if is_supervised {
            eprintln!("restart: supervised exit, daemon manager will respawn clean instance");
            std::process::exit(0);
        }
        let exe = std::env::current_exe().unwrap_or_else(|_| "aaa-daemon".into());
        match std::process::Command::new(&exe).arg("run").spawn() {
            Ok(_) => {
                eprintln!("restart: spawned new instance, exiting current process");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("restart: spawn failed: {err}");
                std::process::exit(1);
            }
        }
    });
}

fn run() {
    let paths = Paths::from_env();
    let cfg = match crate::config::load_or_create(&paths.config_path()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = std::fs::create_dir_all(paths.sessions_dir()) {
        eprintln!("cannot create state dir: {e}");
        std::process::exit(1);
    }
    // Never touch the root before proving it can be read without blocking:
    // under launchd an unreadable external volume parks `opendir` forever on a
    // consent dialog, and startup would never reach the listener.
    let root_state = crate::rootcheck::check(&cfg.project_root);
    if !root_state.is_ok() {
        eprintln!("{}", crate::rootcheck::advice(root_state, &cfg.project_root));
    }
    // startup cleanup (only when the root is usable; never mkdir the root)
    if root_state.is_ok() {
        crate::slug::cleanup_empty_timestamped(&cfg.project_root);
    }

    // Resolve the PATH agents get before anything can ask for a session, so
    // the login-shell probe never shows up as spawn latency.
    let agent_path = crate::agents::agent_path(&paths.home);
    println!("agent PATH: {agent_path}");

    let hub = EventHub::new();
    let pool = SessionPool::new(PoolCtx {
        hub: hub.clone(),
        sessions_dir: paths.sessions_dir(),
    });
    pool.restore_persisted();
    let inbox = crate::inbox::Inbox::load(&paths.state_dir());
    let history = crate::history::History::load(&paths.state_dir());

    let app: SharedApp = Arc::new(App {
        cfg,
        paths,
        started: Instant::now(),
        pool,
        hub,
        store_lock: std::sync::Mutex::new(()),
        bound_port: std::sync::atomic::AtomicU16::new(0),
        inbox: std::sync::Mutex::new(inbox),
        history: std::sync::Mutex::new(history),
        plan_usage: std::sync::Mutex::new(None),
        root_state: std::sync::atomic::AtomicU8::new(root_state.as_u8()),
        restarting: std::sync::atomic::AtomicBool::new(false),
        restart_when_idle: std::sync::atomic::AtomicBool::new(false),
        exe_mtime_at_start: crate::api::exe_mtime(),
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        // state machine tick (1s); a session that just finished its turn
        // gets the queued inbox (if the structured gate allows)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(1));
                loop {
                    iv.tick().await;
                    {
                        let app2 = Arc::clone(&app);
                        let _ = tokio::task::spawn_blocking(move || {
                            for sess in app2.pool.all() {
                                crate::trust::on_tick(&app2, &sess);
                            }
                        })
                        .await;
                    }
                    // 状态翻转由 tick_states 处理；收件箱投喂每秒对所有会话重试一遍
                    // （空着 + 有条目 + 门槛放行就喂）——信任对话框刚被接受、
                    // 表单刚答完这类没有状态翻转的时刻也能把排着的话发出去
                    let _ = app.pool.tick_states();
                    // v1.16：约好的「空闲时重启」——没有会话在跑就走 /restart {force} 那条路
                    if app.restart_when_idle.load(std::sync::atomic::Ordering::SeqCst)
                        && !app.restarting.load(std::sync::atomic::Ordering::SeqCst)
                        && app.pool.all().iter().all(|s| s.state() != State::Running)
                    {
                        let app2 = Arc::clone(&app);
                        tokio::spawn(async move { crate::api::restart_now(app2).await });
                    }
                    {
                        let app2 = Arc::clone(&app);
                        let _ = tokio::task::spawn_blocking(move || {
                            for sess in app2.pool.all() {
                                crate::feed::on_waiting(&app2, sess);
                            }
                            // 会话日志跟着池子走：标题 / 状态 / 清单变了就更新，落盘只在有变化时
                            crate::history::sync(&app2);
                        })
                        .await;
                    }
                }
            });
        }
        // plan 配额轮询（quota.rs）：statusLine 给不了按模型的窗口，从 claude.ai 的
        // usage 接口拿；没登录 / 令牌过期就跳过这一轮，沿用旧值
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(crate::quota::POLL_INTERVAL);
                let mut last_err: Option<String> = None;
                let mut ticks: u64 = 0;
                // 被 claude.ai 限流（429）后歇 5 分钟再问；成功一次恢复每分钟
                let mut backoff_ticks: u64 = 0;
                loop {
                    iv.tick().await;
                    if backoff_ticks > 0 {
                        backoff_ticks -= 1;
                        continue;
                    }
                    // 没有活着的 claude 会话时数字基本不动（别的设备在用除外），
                    // 降到每 5 分钟问一次
                    let any_live = app.pool.all().iter().any(|s| {
                        s.live.lock().unwrap().is_some() && s.meta.lock().unwrap().agent != "shell"
                    });
                    ticks += 1;
                    if !any_live && ticks % crate::quota::IDLE_POLL_EVERY != 1 {
                        continue;
                    }
                    let home = app.paths.home.clone();
                    let res = tokio::task::spawn_blocking(move || {
                        let token = crate::quota::access_token(&home)?;
                        Some(crate::quota::fetch(&token).map(|body| crate::quota::parse_usage(&body)))
                    })
                    .await;
                    match res {
                        Ok(Some(Ok(Some(mut plan)))) => {
                            plan["updated_at"] = serde_json::Value::String(
                                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                            );
                            app.set_plan_usage(plan, false);
                            if last_err.take().is_some() {
                                println!("quota: usage poll recovered");
                            }
                        }
                        Ok(Some(Ok(None))) => {}
                        Ok(Some(Err(e))) => {
                            if e == "HTTP 429" {
                                backoff_ticks = crate::quota::RATE_LIMIT_BACKOFF_TICKS;
                            }
                            // 同一个错只报一次，网断了不刷屏
                            if last_err.as_deref() != Some(e.as_str()) {
                                eprintln!("quota: usage poll failed: {e}");
                                last_err = Some(e);
                            }
                        }
                        Ok(None) | Err(_) => {}
                    }
                }
            });
        }
        // v1.16.2 补清单：池子里没有清单的会话（功能之前的、resume 进来的）起来 90s 后补一轮，
        // 之后每小时补 20 条（summary::backfill）
        if app.cfg.namer {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(90)).await;
                loop {
                    let app2 = Arc::clone(&app);
                    let _ = tokio::task::spawn_blocking(move || crate::summary::backfill(&app2, 20)).await;
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
            });
        }
        // v1.1 消息流尾随。
        //
        // **节拍 250ms**（2026-09-08 从 1s 改下来）：一条消息落进 transcript 到客户端
        // 看见它，以前最坏要等满一秒——那是这条链路上唯一能压的延迟。transcript 本身
        // 只在一条消息**写完**时才落盘（量过：条目成簇出现，簇间静默 30s 到 10 分钟），
        // 所以流式是拿不到的，但那一秒是白等的。
        //
        // 活会话每拍都看；**已退出的每 4 拍看一次**——它们的 transcript 不会再长，
        // 池子里两百多条大都是这种。总 I/O 因此和以前一个量级。
        //
        // `messages_changed` 的 ≥500ms 节流以前是「节拍本身」，现在得显式做（PROTOCOL
        // 「/events」）：窗口内攒着，下一拍补发，一次都不丢。
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_millis(250));
                // 会话 id → (上次发帧的时刻, 攒着还没发的 last_seq)
                let mut evt: std::collections::HashMap<String, (std::time::Instant, Option<u64>)> =
                    std::collections::HashMap::new();
                let mut tick: u64 = 0;
                loop {
                    iv.tick().await;
                    tick = tick.wrapping_add(1);
                    let slow_tick = tick.is_multiple_of(4);
                    let app2 = Arc::clone(&app);
                    let fresh = tokio::task::spawn_blocking(move || {
                        let sessions = app2.pool.all();
                        // 「已被别的会话认领的 transcript」整轮只算一次。以前它在循环
                        // **里面**重建，每个会话都要把其余所有会话的 msgs 锁挨个锁一遍
                        // ——216 个会话就是每秒四万多次加锁，锁的还正是 GET /messages
                        // 要拿的那把。带上自己那份无害：`claimed` 只在「还没认到文件」
                        // 和「兜底文件找升级」两条路上读，后者本来就会跳过自己当前的文件。
                        let claimed: std::collections::HashSet<std::path::PathBuf> = sessions
                            .iter()
                            .filter_map(|s| s.msgs.lock().unwrap().file.clone())
                            .collect();
                        let mut fresh: Vec<(String, u64)> = Vec::new();
                        for sess in &sessions {
                            let exited = sess.state() == State::Exited;
                            if exited && !slow_tick {
                                continue;
                            }
                            if let Some(last_seq) =
                                crate::messages::poll_session(&app2.paths, sess, &claimed)
                            {
                                fresh.push((sess.id.clone(), last_seq));
                            }
                            // mirror「有问题在等回答」to the session object
                            // (structured: transcript AskUserQuestion without
                            // an answer, from this process's lifetime)
                            // v1.22：不只判「有没有」，把**是哪一条**（seq）也带出来——
                            // 客户端据它画表单卡片，不再自己从消息流倒着找一遍
                            let seq = {
                                let meta = sess.meta.lock().unwrap();
                                if meta.state == State::Exited {
                                    None
                                } else {
                                    let since = meta
                                        .created_at
                                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                                    drop(meta);
                                    sess.msgs.lock().unwrap().pending_question(Some(&since)).map(|m| m.seq)
                                }
                            };
                            let asking = seq.is_some();
                            // v1.22：Claude Code 自己排着的待发送消息 + 信任对话框弹着时
                            // daemon 替用户收下的那几条，合成一份镜到会话对象上。客户端
                            // 因此不再维护第二套「待发送」，发送一律走 /input
                            let queued = {
                                let mut q = sess.msgs.lock().unwrap().queued.clone();
                                let path = sess.meta.lock().unwrap().project_path.clone();
                                q.extend(app2.inbox.lock().unwrap().list(&crate::stores::realpath(&path)).into_iter().map(
                                    |e| crate::messages::QueuedMsg { ts: e.created_at, text: e.text },
                                ));
                                q
                            };
                            let mut meta = sess.meta.lock().unwrap();
                            // hooks 刚报了 AskUserQuestion、transcript 还没落盘：几秒内不压回
                            let hinted = meta
                                .asking_hint_inst
                                .map(|t| t.elapsed().as_secs() < 10)
                                .unwrap_or(false);
                            // v1.16：权限对话框在等 = 待回复（hook 记的，跟 transcript 无关）
                            let asking = (asking || (hinted && meta.state != State::Exited) || meta.permission.is_some())
                                && meta.state != State::Exited;
                            // 会话退出后没有可答的了；权限对话框 / hook 抢跑撑起的 asking
                            // 没有对应的 transcript 条目，seq 就是 None
                            let seq = if asking { seq } else { None };
                            if meta.asking != asking || meta.asking_seq != seq || meta.queued != queued {
                                meta.asking = asking;
                                meta.asking_seq = seq;
                                meta.queued = queued;
                                drop(meta);
                                sess.mark_dirty();
                            } else {
                                drop(meta);
                            }
                            crate::messages::mirror_background(sess);
                        }
                        fresh
                    })
                    .await;
                    // ≥500ms 一帧的显式节流：窗口内的攒进 pending，下一拍补发
                    let now = std::time::Instant::now();
                    for (id, seq) in fresh.unwrap_or_default() {
                        let e = evt.entry(id).or_insert((now - MSG_EVT_MIN, None));
                        e.1 = Some(e.1.map_or(seq, |p: u64| p.max(seq)));
                    }
                    evt.retain(|id, (last, pending)| {
                        if now.duration_since(*last) >= MSG_EVT_MIN {
                            if let Some(seq) = *pending {
                                app.hub.messages_changed(id, seq);
                                *last = now;
                                *pending = None;
                            }
                        }
                        // 会话没了就不再占位；没有待发帧且很久没动的条目也清掉
                        pending.is_some() || now.duration_since(*last) < Duration::from_secs(60)
                    });
                }
            });
        }
        // dirty-session flush (>= 250ms throttle for /events)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_millis(250));
                loop {
                    iv.tick().await;
                    app.pool.flush_dirty();
                }
            });
        }
        // SSD health watcher (5s, emits health event on change)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut last = app.root_state();
                let mut iv = tokio::time::interval(Duration::from_secs(5));
                loop {
                    iv.tick().await;
                    // off the reactor: the probe blocks a thread by design
                    let root = app.cfg.project_root.clone();
                    let now = tokio::task::spawn_blocking(move || crate::rootcheck::check(&root))
                        .await
                        .unwrap_or(crate::rootcheck::RootState::Denied);
                    if now != last {
                        app.set_root_state(now);
                        // a denial recovers the moment the grant lands, so
                        // keep saying so rather than only complaining once
                        eprintln!("{}", crate::rootcheck::advice(now, &app.cfg.project_root));
                        last = now;
                        app.hub.health(now.usable());
                    }
                }
            });
        }
        // AI session naming loop (5s; one session per pass)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(5));
                loop {
                    iv.tick().await;
                    name_one_session(&app).await;
                }
            });
        }

        // 端口被占着就重试 5s：重启时上一个实例可能还没退干净（游离模式下新实例
        // 是老实例 spawn 出来的，那一刻老的还握着监听套接字），一次 bind 失败就
        // exit(1) 会让「重启」变成「没有 daemon」。
        let addr = SocketAddr::from(([0, 0, 0, 0], app.cfg.port));
        let listener = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => break l,
                    Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && std::time::Instant::now() < deadline => {
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    }
                    Err(e) => {
                        eprintln!("bind {addr}: {e}");
                        std::process::exit(1);
                    }
                }
            }
        };
        let local = listener.local_addr().expect("local_addr");
        app.bound_port
            .store(local.port(), std::sync::atomic::Ordering::Relaxed);
        // publish the actual bound port (port=0 -> ephemeral; used by tests)
        let _ = std::fs::write(app.paths.port_file(), local.port().to_string());
        eprintln!(
            "aaa-daemon v{} listening on {local} (root: {})",
            env!("CARGO_PKG_VERSION"),
            app.cfg.project_root.display()
        );
        // 上一次 /restart 记下的会话：起来后自动 resume（走 POST /sessions，与客户端同一条路）
        {
            let file = app.paths.state_dir().join("resume_after_restart.json");
            if let Ok(body) = std::fs::read(&file) {
                let _ = std::fs::remove_file(&file);
                let port = local.port();
                let token = app.cfg.token.clone();
                let list: Vec<serde_json::Value> = serde_json::from_slice(&body).unwrap_or_default();
                if !list.is_empty() {
                    eprintln!("restart: resuming {} session(s)", list.len());
                    tokio::task::spawn_blocking(move || {
                        std::thread::sleep(Duration::from_millis(1500));
                        for item in list {
                            let body = serde_json::json!({
                                "project_path": item["project_path"], "agent": item["agent"], "resume": true
                            });
                            let r = ureq::post(&format!("http://127.0.0.1:{port}/api/v1/sessions"))
                                .set("Authorization", &format!("Bearer {token}"))
                                .timeout(Duration::from_secs(20))
                                .send_json(body);
                            if let Err(e) = r {
                                eprintln!("restart: resume {} failed: {e}", item["project_path"]);
                            }
                            std::thread::sleep(Duration::from_millis(400));
                        }
                    });
                }
            }
        }
        let router = crate::api::router(Arc::clone(&app));
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("axum serve");
    });
}

/// Pick one session that needs (re)naming and run the aaa naming chain on the
/// newest stored session file for its cwd. Claude / Reasonix only, skipped
/// for user-renamed sessions.
async fn name_one_session(app: &SharedApp) {
    let candidate = app.pool.list().into_iter().find(|s| {
        let meta = s.meta.lock().unwrap();
        meta.needs_name
            && !meta.custom_title
            && meta.state != State::Running
            && meta.agent == "claude"
    });
    let Some(sess) = candidate else {
        // clear the flag for agents we can't name so we don't rescan forever
        for s in app.pool.all() {
            let mut meta = s.meta.lock().unwrap();
            if meta.needs_name && meta.agent != "claude" {
                meta.needs_name = false;
            }
        }
        return;
    };
    let (agent, cwd) = {
        let mut meta = sess.meta.lock().unwrap();
        meta.needs_name = false;
        (meta.agent.clone(), meta.project_path.clone())
    };
    let app2 = Arc::clone(app);
    let title = tokio::task::spawn_blocking(move || {
        // Unlocked on purpose: claude_name may run haiku for up to 60s, and
        // holding store_lock across that starved every /projects request.
        // The merge-on-save cache makes the unlocked window safe.
        let mut cache = CwdCache::load(&app2.paths.cwd_cache());
        let namer = Namer::new(&app2.paths, app2.cfg.namer);
        let name = match agent.as_str() {
            "claude" => {
                let mut best: Option<(f64, std::path::PathBuf)> = None;
                for r in crate::stores::claude_sessions(&app2.paths, &mut cache) {
                    if r.cwd == cwd && best.as_ref().map(|b| r.mtime > b.0).unwrap_or(true) {
                        best = Some((r.mtime, r.path));
                    }
                }
                best.map(|(_, p)| namer.claude_name(&mut cache, &p))
                    .unwrap_or_default()
            }
            _ => String::new(),
        };
        if cache.dirty() {
            let _g = app2.store_lock.lock().unwrap();
            cache.save();
        }
        name
    })
    .await
    .unwrap_or_default();
    if title.is_empty() {
        return;
    }
    let changed = {
        let mut meta = sess.meta.lock().unwrap();
        if meta.custom_title || meta.title == title {
            false
        } else {
            meta.title = title;
            true
        }
    };
    if changed {
        sess.mark_dirty();
        if sess.state() == State::Exited {
            sess.persist(&app.pool.ctx);
        }
    }
}
