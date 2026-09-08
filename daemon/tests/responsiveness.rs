//! Lock-granularity regression test: `GET /projects` must stay responsive
//! while the store lock is held elsewhere (haiku naming used to hold it for
//! up to 60s, queueing every read-only request behind the LLM call).
//!
//! The handler now scans and names WITHOUT the lock and only takes it around
//! the cwd-cache save (a file write, microseconds). Holding the lock for
//! seconds here therefore must not delay the request.
//!
//! 2026-09-08 顺带量过另一条被外部评审点名的路径——`Session.meta: Mutex<Meta>` 太粗，
//! 「PTY 输出、hook 事件、状态轮询都在抢它，高频刷屏时 API 请求会被间歇阻塞」。在真机上
//! 实测（**184 个会话**、其中一个正在跑并持续输出）：`GET /sessions`（要逐个锁 184 把 meta）
//! p50 **2.0ms** / p95 12.4ms，`GET /projects` p50 5.6ms，`GET /health` p50 0.2ms。
//! 结论：这个量级上不成立，`last_output_at` 没有必要拆成 Atomic——那要改一圈读写点，
//! 换来的是量不出来的收益。**要动它先拿数说话。**

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TOKEN: &str = "aaa_tk_lock_test";

fn build_app(root: &Path, home: &Path) -> aaa_daemon::api::SharedApp {
    use aaa_daemon::config::Config;
    let cfg = Config {
        port: 0,
        token: TOKEN.to_string(),
        project_root: root.to_path_buf(),
        namer: true, // naming enabled, as in production
        auto_trust: true,
    };
    let paths = aaa_daemon::paths::Paths::new(home);
    let hub = aaa_daemon::events::EventHub::new();
    let pool = aaa_daemon::pool::SessionPool::new(aaa_daemon::pool::PoolCtx {
        hub: hub.clone(),
        sessions_dir: paths.sessions_dir(),
    });
    let inbox = aaa_daemon::inbox::Inbox::load(&paths.state_dir());
    let pins = aaa_daemon::pins::Pins::load(&paths.state_dir());
    let history = aaa_daemon::history::History::load(&paths.state_dir());
    Arc::new(aaa_daemon::api::App {
        cfg,
        paths,
        started: Instant::now(),
        pool,
        hub,
        store_lock: std::sync::Mutex::new(()),
        bound_port: std::sync::atomic::AtomicU16::new(0),
        pins: std::sync::Mutex::new(pins),
        history: std::sync::Mutex::new(history),
        inbox: std::sync::Mutex::new(inbox),
        plan_usage: std::sync::Mutex::new(None),
        root_state: std::sync::atomic::AtomicU8::new(aaa_daemon::rootcheck::RootState::Ok.as_u8()),
        restarting: std::sync::atomic::AtomicBool::new(false),
        restart_when_idle: std::sync::atomic::AtomicBool::new(false),
        exe_mtime_at_start: None,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn projects_list_is_not_blocked_by_a_busy_store_lock() {
    use tower::util::ServiceExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let home = dir.path().join("home");
    std::fs::create_dir_all(root.join("myproj")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    // 名册以注册表为准：未登记的目录不会出现在 /projects 里
    std::fs::write(
        root.join(".aaa-agents"),
        format!("{}\tclaude\n", root.join("myproj").display()),
    )
    .unwrap();
    let app = build_app(&root, &home);

    // Simulate a long store operation elsewhere by parking the lock for 4s.
    let app2 = Arc::clone(&app);
    let holder = std::thread::spawn(move || {
        let _g = app2.store_lock.lock().unwrap();
        std::thread::sleep(Duration::from_secs(4));
    });
    // make sure the holder actually has the lock before we measure
    std::thread::sleep(Duration::from_millis(200));

    let router = aaa_daemon::api::router(Arc::clone(&app));
    let mut req = axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/projects")
        .header("Authorization", format!("Bearer {TOKEN}"))
        .body(axum::body::Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(
            ([127, 0, 0, 1], 33333),
        )));

    let started = Instant::now();
    let resp = router.oneshot(req).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(resp.status().as_u16(), 200);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1, "project row served");
    assert_eq!(v[0]["name"], "myproj");
    assert!(
        elapsed < Duration::from_secs(2),
        "GET /projects must not queue behind the store lock (took {elapsed:?})"
    );
    holder.join().unwrap();
}
