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
    });
    pool.restore_persisted();
    let inbox = crate::inbox::Inbox::load(&paths.state_dir());
    let pins = crate::pins::Pins::load(&paths.state_dir());
    let history = crate::history::History::load(&paths.state_dir());
    let days = crate::history::Days::load(&paths.state_dir());

    let app: SharedApp = Arc::new(App {
        cfg,
        paths,
        started: Instant::now(),
        pool,
        hub,
        store_lock: std::sync::Mutex::new(()),
        bound_port: std::sync::atomic::AtomicU16::new(0),
        inbox: std::sync::Mutex::new(inbox),
        pins: std::sync::Mutex::new(pins),
        history: std::sync::Mutex::new(history),
        days: std::sync::Mutex::new(days),
        plan_usage: std::sync::Mutex::new(None),
        root_state: std::sync::atomic::AtomicU8::new(root_state.as_u8()),
        restarting: std::sync::atomic::AtomicBool::new(false),
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
        // 日历摘要：起来 30s 后一次，之后每 5 分钟看哪些天的输入变了（history.rs）
        {
            let app = Arc::clone(&app);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                loop {
                    let app2 = Arc::clone(&app);
                    let _ = tokio::task::spawn_blocking(move || crate::history::refresh_days(&app2, 2)).await;
                    tokio::time::sleep(Duration::from_secs(300)).await;
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
                        let sessions = app2.pool.all();
                        for sess in &sessions {
                            // 其他会话已认领的存储文件（同目录并发不许抢）
                            let claimed: std::collections::HashSet<std::path::PathBuf> =
                                sessions
                                    .iter()
                                    .filter(|s| s.id != sess.id)
                                    .filter_map(|s| s.msgs.lock().unwrap().file.clone())
                                    .collect();
                            if let Some(last_seq) =
                                crate::messages::poll_session(&app2.paths, sess, &claimed)
                            {
                                app2.hub.messages_changed(&sess.id, last_seq);
                            }
                            // mirror「有问题在等回答」to the session object
                            // (structured: transcript AskUserQuestion without
                            // an answer, from this process's lifetime)
                            let asking = {
                                let meta = sess.meta.lock().unwrap();
                                if meta.state == State::Exited {
                                    false
                                } else {
                                    let since = meta
                                        .created_at
                                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                                    drop(meta);
                                    sess.msgs.lock().unwrap().pending_question(Some(&since)).is_some()
                                }
                            };
                            let mut meta = sess.meta.lock().unwrap();
                            // hooks 刚报了 AskUserQuestion、transcript 还没落盘：几秒内不压回
                            let hinted = meta
                                .asking_hint_inst
                                .map(|t| t.elapsed().as_secs() < 10)
                                .unwrap_or(false);
                            let asking = asking || (hinted && meta.state != State::Exited);
                            if meta.asking != asking {
                                meta.asking = asking;
                                drop(meta);
                                sess.mark_dirty();
                            }
                        }
                    })
                    .await;
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
