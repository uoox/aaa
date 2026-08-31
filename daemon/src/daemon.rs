//! CLI entry + daemon run loop.

use std::net::SocketAddr;
use std::path::Path;
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
  install-claude-hooks      向 ~/.claude/settings.json 合并 Notification/Stop hook
  perms status              macOS 权限体检 (只读)
  perms request-all         逐项触发授权弹窗 (弹窗出现在 Mac 屏幕上)
";

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
        ("install-claude-hooks", _) => {
            let paths = Paths::from_env();
            match crate::config::load_or_create(&paths.config_path()) {
                Ok(cfg) => match crate::claude_hooks::install(&paths, cfg.port) {
                    Ok(msg) => println!("{msg}"),
                    Err(e) => {
                        eprintln!("install-claude-hooks failed: {e}");
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("config error: {e}");
                    std::process::exit(1);
                }
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

/// Apply-by-restart for the config API: `exec` keeps the PID, so a
/// launchd-supervised daemon stays supervised, and an orphan process (no
/// launchd) keeps running too — either way `run()` re-reads the config.
/// Callers guarantee no live sessions (exec tears down every PTY).
pub fn restart_self_after_ms(ms: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        use std::os::unix::process::CommandExt;
        let exe = std::env::current_exe().unwrap_or_else(|_| "aaa-daemon".into());
        let err = std::process::Command::new(exe).arg("run").exec();
        eprintln!("re-exec failed: {err}");
        std::process::exit(1); // launchd KeepAlive 会拉起来；游离进程只能到此为止
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
        ntfy: cfg.ntfy.clone(),
        ckpt_cfg: cfg.checkpoint.clone(),
    });
    pool.restore_persisted();
    let inbox = crate::inbox::Inbox::load(&paths.state_dir());

    let app: SharedApp = Arc::new(App {
        cfg,
        paths,
        started: Instant::now(),
        pool,
        hub,
        store_lock: std::sync::Mutex::new(()),
        bound_port: std::sync::atomic::AtomicU16::new(0),
        inbox: std::sync::Mutex::new(inbox),
        root_state: std::sync::atomic::AtomicU8::new(root_state.as_u8()),
        restarting: std::sync::atomic::AtomicBool::new(false),
    });

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async move {
        // state machine tick (1s); waiting transitions feed inbox / dedup-ntfy
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(1));
                loop {
                    iv.tick().await;
                    let entered = app.pool.tick_states();
                    for (sess, question) in entered {
                        crate::waiting::on_waiting(&app, sess, question).await;
                    }
                }
            });
        }
        // v1.1 message stream tail (1s; the cadence itself is the >=500ms
        // throttle for messages_changed)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(1));
                loop {
                    iv.tick().await;
                    let app2 = Arc::clone(&app);
                    let _ = tokio::task::spawn_blocking(move || {
                        for sess in app2.pool.all() {
                            if let Some(last_seq) =
                                crate::messages::poll_session(&app2.paths, &sess)
                            {
                                app2.hub.messages_changed(&sess.id, last_seq);
                            }
                        }
                    })
                    .await;
                }
            });
        }
        // v1.1 auto checkpoints (60s scan; per-session interval from config)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(60));
                loop {
                    iv.tick().await;
                    if !app.cfg.checkpoint.enabled {
                        continue;
                    }
                    let interval = app.cfg.checkpoint.interval_minutes;
                    for sess in app.pool.all() {
                        let (agent, project_path, has_start, alive) = {
                            let meta = sess.meta.lock().unwrap();
                            let alive = sess.live.lock().unwrap().is_some();
                            (
                                meta.agent.clone(),
                                meta.project_path.clone(),
                                meta.ckpt_start_ref.is_some(),
                                alive,
                            )
                        };
                        if agent == "shell" || !has_start || !alive {
                            continue;
                        }
                        let due = {
                            let st = sess.ckpt.lock().unwrap();
                            crate::checkpoint::auto_due(st.last_at, interval, Instant::now())
                        };
                        if !due {
                            continue;
                        }
                        let sess2 = Arc::clone(&sess);
                        let _ = tokio::task::spawn_blocking(move || {
                            let mut st = sess2.ckpt.lock().unwrap();
                            let _ = crate::checkpoint::make_checkpoint(
                                std::path::Path::new(&project_path),
                                &sess2.id,
                                &mut st,
                                "auto",
                            );
                        })
                        .await;
                    }
                }
            });
        }
        // v1.1 watchdog (30s)
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                let mut iv = tokio::time::interval(Duration::from_secs(30));
                loop {
                    iv.tick().await;
                    let cfg = &app.cfg.watchdog;
                    for sess in app.pool.all() {
                        let (state, agent, silence_s, already, title) = {
                            let meta = sess.meta.lock().unwrap();
                            (
                                meta.state,
                                meta.agent.clone(),
                                meta.last_output_inst
                                    .map(|t| t.elapsed().as_secs())
                                    .unwrap_or(0),
                                meta.stalled_notified,
                                meta.title.clone(),
                            )
                        };
                        let alive = sess.live.lock().unwrap().is_some();
                        if !SessionPool::watchdog_due(
                            state,
                            &agent,
                            alive,
                            silence_s,
                            cfg.stall_minutes,
                            already,
                        ) {
                            continue;
                        }
                        sess.meta.lock().unwrap().stalled_notified = true;
                        app.hub.session_stalled(&sess.id, silence_s);
                        if let Some(ntfy) = app.cfg.ntfy.clone() {
                            let body = format!("{} 分钟无输出", silence_s / 60);
                            let title = format!("疑似卡死: {title}");
                            tokio::task::spawn_blocking(move || {
                                crate::ntfy::push_blocking(
                                    &ntfy,
                                    &title,
                                    &body,
                                    crate::ntfy::PRIO_HIGH,
                                );
                            });
                        }
                        if cfg.auto_kill {
                            sess.kill().await;
                        }
                    }
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

        let addr = SocketAddr::from(([0, 0, 0, 0], app.cfg.port));
        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("bind {addr}: {e}");
                std::process::exit(1);
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
            && matches!(meta.agent.as_str(), "claude" | "reasonix")
    });
    let Some(sess) = candidate else {
        // clear the flag for agents we can't name so we don't rescan forever
        for s in app.pool.all() {
            let mut meta = s.meta.lock().unwrap();
            if meta.needs_name && !matches!(meta.agent.as_str(), "claude" | "reasonix") {
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
            "reasonix" => {
                let st = crate::stores::rnx_stat(&app2.paths, &cwd);
                if st.meta.is_empty() {
                    String::new()
                } else {
                    namer.reasonix_name(Path::new(&st.meta))
                }
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
