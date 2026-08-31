//! git checkpoint / diff / rollback tests. Everything runs in tempdirs with
//! throwaway repos; the project's real state (HEAD / index / worktree) is
//! asserted untouched at every step. Rollback is deletion-class code, so the
//! assertions here are deliberately paranoid.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aaa_daemon::checkpoint::{self, CkptState};

fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn checkpoints_never_touch_repo_state() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write(&proj.join("a.txt"), "v1");
    write(&proj.join("sub").join("b.txt"), "b1");

    // auto-init
    assert!(checkpoint::ensure_repo(&proj, false, 0).is_err(), "no repo, no init when disabled");
    checkpoint::ensure_repo(&proj, true, 0).unwrap();
    assert!(proj.join(".git").is_dir());
    checkpoint::ensure_repo(&proj, false, 0).unwrap(); // idempotent once repo exists

    // size guard: a repo-less dir over budget is skipped (no .git created)
    let big = dir.path().join("big");
    std::fs::create_dir_all(&big).unwrap();
    write(&big.join("blob.bin"), &"x".repeat(2 * 1024 * 1024)); // 2 MB
    assert!(
        checkpoint::ensure_repo(&big, true, 1).is_err(),
        "over 1MB budget must skip init"
    );
    assert!(!big.join(".git").exists(), "no repo left behind when skipped");
    // an existing repo is never blocked by the budget
    checkpoint::ensure_repo(&proj, true, 1).unwrap();

    let mut st = CkptState::default();
    let r = checkpoint::make_checkpoint(&proj, "s_test", &mut st, "start")
        .unwrap()
        .unwrap();
    assert_eq!(r, "refs/aaa-ckpt/s_test/0-start");
    assert_eq!(st.start_ref.as_deref(), Some("refs/aaa-ckpt/s_test/0-start"));

    // repo state untouched: unborn HEAD, no real index, nothing staged
    assert!(!proj.join(".git").join("index").exists(), ".git/index must not be created");
    let (head_ok, _) = git(&proj, &["rev-parse", "--verify", "HEAD"]);
    assert!(!head_ok, "HEAD must stay unborn");
    let (_, status) = git(&proj, &["status", "--porcelain"]);
    assert!(
        status.lines().all(|l| l.starts_with("??")),
        "worktree files must all still be untracked: {status}"
    );

    // auto with no changes: no new ref
    assert!(checkpoint::make_checkpoint(&proj, "s_test", &mut st, "auto")
        .unwrap()
        .is_none());
    // auto after a change: new ref with running counter
    write(&proj.join("a.txt"), "v2");
    let r2 = checkpoint::make_checkpoint(&proj, "s_test", &mut st, "auto")
        .unwrap()
        .unwrap();
    assert_eq!(r2, "refs/aaa-ckpt/s_test/1-auto");
    let (ok, refs) = git(&proj, &["show-ref"]);
    assert!(ok);
    assert!(refs.contains("refs/aaa-ckpt/s_test/0-start"));
    assert!(refs.contains("refs/aaa-ckpt/s_test/1-auto"));
    // still unborn, still untracked-only
    let (head_ok, _) = git(&proj, &["rev-parse", "--verify", "HEAD"]);
    assert!(!head_ok);
    assert!(!proj.join(".git").join("index").exists());
}

#[test]
fn checkpoints_on_existing_repo_leave_head_and_index_alone() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write(&proj.join("committed.txt"), "base");
    let (ok, _) = git(&proj, &["init", "-q"]);
    assert!(ok);
    git(&proj, &["-c", "user.name=t", "-c", "user.email=t@t", "add", "committed.txt"]);
    let (ok, _) = git(&proj, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base"]);
    assert!(ok);
    let (_, head_before) = git(&proj, &["rev-parse", "HEAD"]);
    let index_before = std::fs::read(proj.join(".git").join("index")).unwrap();

    write(&proj.join("wip.txt"), "uncommitted work");
    let mut st = CkptState::default();
    checkpoint::make_checkpoint(&proj, "s_x", &mut st, "start").unwrap().unwrap();

    let (_, head_after) = git(&proj, &["rev-parse", "HEAD"]);
    assert_eq!(head_before, head_after, "HEAD unchanged");
    let index_after = std::fs::read(proj.join(".git").join("index")).unwrap();
    assert_eq!(index_before, index_after, "real index byte-identical");
    // checkpoint commit is invisible to branches
    let (_, branch_log) = git(&proj, &["log", "--oneline"]);
    assert_eq!(branch_log.lines().count(), 1);
}

#[test]
fn diff_reports_added_modified_deleted_and_respects_gitignore() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write(&proj.join("keep.txt"), "same\n");
    write(&proj.join("mod.txt"), "old line\n");
    write(&proj.join("del.txt"), "bye\n");
    checkpoint::ensure_repo(&proj, true, 0).unwrap();
    let mut st = CkptState::default();
    let start = checkpoint::make_checkpoint(&proj, "s_d", &mut st, "start")
        .unwrap()
        .unwrap();

    write(&proj.join("mod.txt"), "new line\n");
    std::fs::remove_file(proj.join("del.txt")).unwrap();
    write(&proj.join("add.txt"), "fresh\n");
    write(&proj.join(".gitignore"), "secret.txt\n");
    write(&proj.join("secret.txt"), "ignored\n");
    write(&proj.join("big.txt"), &"x".repeat(100 * 1024));

    let files = checkpoint::diff(&proj, &start).unwrap();
    let by_path: std::collections::HashMap<&str, &aaa_daemon::checkpoint::DiffFile> =
        files.iter().map(|f| (f.path.as_str(), f)).collect();
    assert_eq!(by_path["mod.txt"].status, "modified");
    assert!(by_path["mod.txt"].additions >= 1 && by_path["mod.txt"].deletions >= 1);
    assert!(by_path["mod.txt"].patch.contains("+new line"));
    assert!(by_path["mod.txt"].patch.contains("-old line"));
    assert_eq!(by_path["del.txt"].status, "deleted");
    assert_eq!(by_path["add.txt"].status, "added");
    assert_eq!(by_path[".gitignore"].status, "added");
    assert!(!by_path.contains_key("secret.txt"), "gitignored files excluded");
    assert!(!by_path.contains_key("keep.txt"), "unchanged files excluded");
    let big = by_path["big.txt"];
    assert!(big.truncated, "64KB patch cap");
    assert!(big.patch.len() <= aaa_daemon::checkpoint::PATCH_CAP_BYTES);
}

#[test]
fn rollback_restores_worktree_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    write(&proj.join("a.txt"), "v1");
    write(&proj.join("sub").join("b.txt"), "b1");
    write(&proj.join(".gitignore"), "ignored.txt\n");
    write(&proj.join("ignored.txt"), "keep me\n");
    checkpoint::ensure_repo(&proj, true, 0).unwrap();
    let mut st = CkptState::default();
    let start = checkpoint::make_checkpoint(&proj, "s_r", &mut st, "start")
        .unwrap()
        .unwrap();

    // mutate everything
    write(&proj.join("a.txt"), "v2 CORRUPTED");
    std::fs::remove_file(proj.join("sub").join("b.txt")).unwrap();
    write(&proj.join("new.txt"), "added after start");
    write(&proj.join("deep").join("nested").join("c.txt"), "deep");
    write(&proj.join("ignored.txt"), "keep me\nplus appended\n");

    let (restored, deleted) = checkpoint::rollback(&proj, &start).unwrap();
    assert_eq!(restored, 3, "a.txt + sub/b.txt + .gitignore");
    assert_eq!(deleted, 2, "new.txt + deep/nested/c.txt");

    assert_eq!(read(&proj.join("a.txt")), "v1");
    assert_eq!(read(&proj.join("sub").join("b.txt")), "b1");
    assert!(!proj.join("new.txt").exists());
    assert!(!proj.join("deep").exists(), "empty dirs pruned");
    assert_eq!(
        read(&proj.join("ignored.txt")),
        "keep me\nplus appended\n",
        "gitignored files are never rolled back"
    );
    assert!(proj.join(".git").is_dir(), ".git untouched");
    assert!(!proj.join(".git").join("index").exists());
    let (head_ok, _) = git(&proj, &["rev-parse", "--verify", "HEAD"]);
    assert!(!head_ok, "HEAD still unborn after rollback");

    // diff is now empty again
    assert!(checkpoint::diff(&proj, &start).unwrap().is_empty());
}

// ---------- API-level coverage (router + synthetic session, no daemon) ----------

const TOKEN: &str = "aaa_tk_router_test";

fn build_app(root: &Path, home: &Path) -> aaa_daemon::api::SharedApp {
    use aaa_daemon::config::{CheckpointConfig, Config, WatchdogConfig};
    let cfg = Config {
        port: 0,
        token: TOKEN.to_string(),
        project_root: root.to_path_buf(),
        namer: false,
        ntfy: None,
        checkpoint: CheckpointConfig::default(),
        watchdog: WatchdogConfig::default(),
    };
    let paths = aaa_daemon::paths::Paths::new(home);
    let hub = aaa_daemon::events::EventHub::new();
    let pool = aaa_daemon::pool::SessionPool::new(aaa_daemon::pool::PoolCtx {
        hub: hub.clone(),
        sessions_dir: paths.sessions_dir(),
        ntfy: None,
        ckpt_cfg: cfg.checkpoint.clone(),
    });
    let inbox = aaa_daemon::inbox::Inbox::load(&paths.state_dir());
    Arc::new(aaa_daemon::api::App {
        cfg,
        paths,
        started: std::time::Instant::now(),
        pool,
        hub,
        store_lock: std::sync::Mutex::new(()),
        bound_port: std::sync::atomic::AtomicU16::new(0),
        inbox: std::sync::Mutex::new(inbox),
        root_state: std::sync::atomic::AtomicU8::new(aaa_daemon::rootcheck::RootState::Ok.as_u8()),
        restarting: std::sync::atomic::AtomicBool::new(false),
    })
}

/// A synthetic exited claude session pointing at `proj`.
fn synthetic_session(app: &aaa_daemon::api::SharedApp, proj: &Path, start_ref: Option<String>) -> String {
    use aaa_daemon::pool::{Meta, Session, State};
    let now = chrono::Utc::now();
    let meta = Meta {
        title: "t".into(),
        custom_title: false,
        project_path: proj.to_string_lossy().into_owned(),
        project_name: "proj".into(),
        agent: "claude".into(),
        state: State::Exited,
        question: None,
        rows: 40,
        cols: 120,
        pid: None,
        exit_code: Some(0),
        resume_id: None,
        created_at: now,
        last_output_at: now,
        preview: String::new(),
        feed_inbox: true,
        ckpt_start_ref: start_ref.clone(),
        last_output_inst: None,
        hook_waiting: false,
        needs_name: false,
        inbox_fed: false,
        stalled_notified: false,
        last_notified_question: None,
        last_notify_at: None,
    };
    let (tx, _) = tokio::sync::broadcast::channel(8);
    let id = "s_router01".to_string();
    let sess = Arc::new(Session {
        id: id.clone(),
        meta: std::sync::Mutex::new(meta),
        parser: std::sync::Mutex::new(None),
        out_tx: tx,
        live: std::sync::Mutex::new(None),
        dirty: std::sync::atomic::AtomicBool::new(false),
        msgs: std::sync::Mutex::new(aaa_daemon::messages::MsgStore::for_agent("claude")),
        ckpt: std::sync::Mutex::new(CkptState { start_ref, ..Default::default() }),
    });
    app.pool.map.lock().unwrap().insert(id.clone(), sess);
    id
}

async fn call(
    app: &aaa_daemon::api::SharedApp,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    use tower::util::ServiceExt;
    let router = aaa_daemon::api::router(Arc::clone(app));
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {TOKEN}"));
    if body.is_some() {
        builder = builder.header("Content-Type", "application/json");
    }
    let mut req = builder
        .body(match &body {
            Some(v) => axum::body::Body::from(v.to_string()),
            None => axum::body::Body::empty(),
        })
        .unwrap();
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from(
            ([127, 0, 0, 1], 33333),
        )));
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn diff_and_rollback_via_api() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let home = dir.path().join("home");
    let proj = root.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    write(&proj.join("main.rs"), "fn main() {}\n");

    checkpoint::ensure_repo(&proj, true, 0).unwrap();
    let mut st = CkptState::default();
    let start = checkpoint::make_checkpoint(&proj, "s_router01", &mut st, "start")
        .unwrap()
        .unwrap();

    let app = build_app(&root, &home);
    let proj_canon = PathBuf::from(aaa_daemon::stores::realpath(&proj.to_string_lossy()));
    let sid = synthetic_session(&app, &proj_canon, Some(start));

    // agent corrupts the file and adds junk
    write(&proj.join("main.rs"), "fn main() { panic!(); }\n");
    write(&proj.join("junk.tmp"), "junk");

    let (code, diff) = call(&app, "GET", &format!("/api/v1/sessions/{sid}/diff"), None).await;
    assert_eq!(code, 200);
    assert_eq!(diff["supported"], true);
    assert!(diff["base"].as_str().unwrap().starts_with("refs/aaa-ckpt/"));
    let files = diff["files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{diff}");
    assert!(files.iter().any(|f| f["path"] == "main.rs" && f["status"] == "modified"));
    assert!(files.iter().any(|f| f["path"] == "junk.tmp" && f["status"] == "added"));

    // confirm is mandatory
    let (code, e) = call(
        &app,
        "POST",
        &format!("/api/v1/sessions/{sid}/rollback"),
        Some(serde_json::json!({"confirm": false})),
    )
    .await;
    assert_eq!(code, 409);
    assert_eq!(e["error"]["code"], "conflict");

    let (code, rb) = call(
        &app,
        "POST",
        &format!("/api/v1/sessions/{sid}/rollback"),
        Some(serde_json::json!({"confirm": true})),
    )
    .await;
    assert_eq!(code, 200, "{rb}");
    assert_eq!(rb["ok"], true);
    assert_eq!(read(&proj.join("main.rs")), "fn main() {}\n");
    assert!(!proj.join("junk.tmp").exists());

    let (_, diff2) = call(&app, "GET", &format!("/api/v1/sessions/{sid}/diff"), None).await;
    assert!(diff2["files"].as_array().unwrap().is_empty());

    // no-checkpoint session reports unsupported
    let sid2 = synthetic_session(&app, &proj_canon, None);
    let (code, d3) = call(&app, "GET", &format!("/api/v1/sessions/{sid2}/diff"), None).await;
    assert_eq!(code, 200);
    assert_eq!(d3["supported"], false);
}

#[tokio::test]
async fn messages_and_inbox_via_api() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let app = build_app(&root, &home);
    let proj = root.join("p");
    std::fs::create_dir_all(&proj).unwrap();
    let sid = synthetic_session(&app, &proj, None);

    // prefill the message store as the tailer would
    {
        let sess = app.pool.get(&sid).unwrap();
        let mut store = sess.msgs.lock().unwrap();
        let lines = format!(
            "{}\n{}\n{}\n",
            serde_json::json!({"type":"user","message":{"role":"user","content":"问题一"},"timestamp":"T1"}),
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"回答一"}]},"timestamp":"T2"}),
            serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},"timestamp":"T3"}),
        );
        aaa_daemon::messages::ingest(&mut store, lines.as_bytes());
    }
    let (code, m) = call(&app, "GET", &format!("/api/v1/sessions/{sid}/messages"), None).await;
    assert_eq!(code, 200);
    assert_eq!(m["supported"], true);
    assert_eq!(m["source"], "claude");
    assert_eq!(m["last_seq"], 3);
    assert_eq!(m["messages"].as_array().unwrap().len(), 3);
    assert_eq!(m["messages"][2]["tool"]["name"], "Bash");
    // incremental pull
    let (_, m2) = call(&app, "GET", &format!("/api/v1/sessions/{sid}/messages?after=2"), None).await;
    assert_eq!(m2["messages"].as_array().unwrap().len(), 1);
    assert_eq!(m2["messages"][0]["seq"], 3);
    // unsupported agent shape
    let (_, list) = call(&app, "GET", "/api/v1/sessions", None).await;
    assert!(list.is_array());

    // ---- inbox CRUD over the API ----
    let proj_s = proj.to_string_lossy().into_owned();
    let (code, e1) = call(
        &app,
        "POST",
        "/api/v1/inbox",
        Some(serde_json::json!({"path": proj_s, "text": "任务A"})),
    )
    .await;
    assert_eq!(code, 200);
    let id1 = e1["id"].as_str().unwrap().to_string();
    assert!(id1.starts_with("in_"));
    let (_, _e2) = call(
        &app,
        "POST",
        "/api/v1/inbox",
        Some(serde_json::json!({"path": proj_s, "text": "任务B"})),
    )
    .await;
    let (code, lst) = call(
        &app,
        "GET",
        &format!("/api/v1/inbox?path={}", urlencode(&proj_s)),
        None,
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(lst.as_array().unwrap().len(), 2);
    assert_eq!(lst[0]["text"], "任务A");
    let (code, _) = call(&app, "DELETE", &format!("/api/v1/inbox/{id1}"), None).await;
    assert_eq!(code, 200);
    let (_, lst2) = call(
        &app,
        "GET",
        &format!("/api/v1/inbox?path={}", urlencode(&proj_s)),
        None,
    )
    .await;
    assert_eq!(lst2.as_array().unwrap().len(), 1);
    assert_eq!(lst2[0]["text"], "任务B");
    let (code, _) = call(&app, "DELETE", "/api/v1/inbox/in_missing", None).await;
    assert_eq!(code, 404);
    // empty text rejected
    let (code, _) = call(
        &app,
        "POST",
        "/api/v1/inbox",
        Some(serde_json::json!({"path": proj_s, "text": "  "})),
    )
    .await;
    assert_eq!(code, 409);
}

/// Regression: `projects/delete` must never escape the project root. A path
/// containing `..` (which resolves above the root) or a symlink pointing
/// outside must be refused, and the outside target must survive untouched.
#[tokio::test]
async fn projects_delete_refuses_traversal_and_symlink_escape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let home = dir.path().join("home");
    let proj = root.join("keep");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    // A sibling directory OUTSIDE the root that must never be deleted.
    let outside = dir.path().join("precious");
    std::fs::create_dir_all(&outside).unwrap();
    write(&outside.join("data.txt"), "do not delete me");
    let app = build_app(&root, &home);

    // 1) `<root>/..` resolves to the root's parent (the whole tempdir).
    let (code, v) = call(
        &app,
        "POST",
        "/api/v1/projects/delete",
        Some(serde_json::json!({"paths": [format!("{}/..", root.display())]})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(v["results"][0]["ok"], false, "`..` escape must be refused");
    assert!(root.is_dir(), "root itself must survive");
    assert!(outside.is_dir(), "root's parent must survive");

    // 2) a symlink under the root pointing outside.
    #[cfg(unix)]
    {
        let link = root.join("escape");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let (code, v) = call(
            &app,
            "POST",
            "/api/v1/projects/delete",
            Some(serde_json::json!({"paths": [link.to_string_lossy()]})),
        )
        .await;
        assert_eq!(code, 200);
        assert_eq!(v["results"][0]["ok"], false, "symlink escape must be refused");
        assert!(outside.join("data.txt").is_file(), "symlink target untouched");
    }

    // 3) a genuine direct child still deletes.
    let (code, v) = call(
        &app,
        "POST",
        "/api/v1/projects/delete",
        Some(serde_json::json!({"paths": [proj.to_string_lossy()]})),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(v["results"][0]["ok"], true, "real child must delete");
    assert!(!proj.exists());
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
