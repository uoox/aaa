//! End-to-end smoke test: boots the real daemon binary on an ephemeral port
//! with AAA_HOME + AAA_DAEMON_CONFIG pointing at tempdirs, then drives the
//! REST + WS API: auth, project create, shell session spawn, WS attach + echo,
//! resize, kill -> exited, restart -> exited session still listed + replay.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;

const TOKEN: &str = "aaa_tk_smoketest0000000000000000dead";

struct DaemonGuard {
    child: Child,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Env {
    _dir: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    config: PathBuf,
}

fn setup_env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let root = dir.path().join("ssd-root");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        format!(
            "port = 0\ntoken = \"{TOKEN}\"\nproject_root = \"{}\"\nnamer = false\n",
            root.display()
        ),
    )
    .unwrap();
    Env { _dir: dir, home, root, config }
}

fn spawn_daemon(env: &Env) -> DaemonGuard {
    let port_file = env
        .home
        .join(".local")
        .join("state")
        .join("aaa-daemon")
        .join("daemon.port");
    let _ = std::fs::remove_file(&port_file);
    let child = Command::new(env!("CARGO_BIN_EXE_aaa-daemon"))
        .arg("run")
        .env("AAA_HOME", &env.home)
        .env("AAA_DAEMON_CONFIG", &env.config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn daemon");
    DaemonGuard { child }
}

fn wait_port(env: &Env) -> u16 {
    let port_file = env
        .home
        .join(".local")
        .join("state")
        .join("aaa-daemon")
        .join("daemon.port");
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(s) = std::fs::read_to_string(&port_file) {
            if let Ok(p) = s.trim().parse::<u16>() {
                if p > 0 {
                    return p;
                }
            }
        }
        assert!(Instant::now() < deadline, "daemon did not publish its port");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn http(method: &str, port: u16, path: &str, token: Option<&str>, body: Option<Value>) -> (u16, Value) {
    let url = format!("http://127.0.0.1:{port}{path}");
    let mut req = match method {
        "GET" => ureq::get(&url),
        "POST" => ureq::post(&url),
        "DELETE" => ureq::delete(&url),
        _ => unreachable!(),
    };
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let result = match body {
        Some(b) => req.set("Content-Type", "application/json").send_string(&b.to_string()),
        None => req.call(),
    };
    match result {
        Ok(resp) => {
            let status = resp.status();
            let text = resp.into_string().unwrap_or_default();
            let v = serde_json::from_str(&text).unwrap_or(Value::Null);
            (status, v)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            let v = serde_json::from_str(&text).unwrap_or(Value::Null);
            (code, v)
        }
        Err(e) => panic!("http {method} {path}: {e}"),
    }
}

fn find_session<'a>(list: &'a Value, id: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .unwrap_or_else(|| panic!("session {id} not in list: {list}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn full_session_lifecycle() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);

    // ---- auth ----
    let (code, body) = http("GET", port, "/api/v1/health", None, None);
    assert_eq!(code, 401);
    assert_eq!(body["error"]["code"], "unauthorized");
    let (code, health) = http("GET", port, "/api/v1/health", Some(TOKEN), None);
    assert_eq!(code, 200);
    assert_eq!(health["ssd_mounted"], true);
    let (code, _) = http("GET", port, "/api/v1/health", Some("wrong"), None);
    assert_eq!(code, 401);

    // ---- agents table ----
    let (code, agents) = http("GET", port, "/api/v1/agents", Some(TOKEN), None);
    assert_eq!(code, 200);
    let shell = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "shell")
        .unwrap();
    assert_eq!(shell["available"], true);

    // ---- project create (+conflict) ----
    let (code, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "smoke test 项目", "agent": "shell"})),
    );
    assert_eq!(code, 200);
    assert_eq!(proj["name"], "smoke-test-项目");
    // response must be a full project object: at least {path, name, agent}
    assert_eq!(proj["agent"], "shell");
    assert!(proj["mtime"].is_string());
    assert!(proj["dir_size"].is_number());
    let proj_path = proj["path"].as_str().unwrap().to_string();
    assert!(Path::new(&proj_path).is_dir());
    let (code, dup) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "smoke test 项目"})),
    );
    assert_eq!(code, 409);
    assert_eq!(dup["error"]["code"], "conflict");
    // registry written in aaa format
    let reg = std::fs::read_to_string(env.root.join(".aaa-agents")).unwrap();
    assert_eq!(reg, format!("{proj_path}\tshell\n"));

    // unknown agent rejected
    let (code, e) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": proj_path, "agent": "nope"})),
    );
    assert_eq!(code, 400);
    assert_eq!(e["error"]["code"], "agent_unknown");

    // ---- spawn shell session ----
    let (code, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": proj_path, "agent": "shell", "resume": false})),
    );
    assert_eq!(code, 200, "session create failed: {sess}");
    let sid = sess["id"].as_str().unwrap().to_string();
    assert_eq!(sess["agent"], "shell");
    assert_eq!(sess["state"], "running");
    assert_eq!(sess["rows"], 40);
    assert_eq!(sess["cols"], 120);
    assert!(sess["pid"].as_u64().is_some());
    // full session object: resume_id / exit_code null here; asking is a bool
    assert!(sess["resume_id"].is_null());
    assert!(sess["exit_code"].is_null());
    assert_eq!(sess["asking"], false);
    assert_eq!(sess["project_path"].as_str().unwrap(), {
        let c = std::fs::canonicalize(&proj_path).unwrap();
        c.to_string_lossy().into_owned()
    });

    // ---- 幂等：同项目+同 agent 再 POST 返回同一个会话，不孵第二个进程 ----
    // （实测事故：双击「没反应」再点一次 → 两个进程 resume 同一对话）
    let (code, again) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": proj_path, "agent": "shell", "resume": false})),
    );
    assert_eq!(code, 200);
    assert_eq!(again["id"].as_str().unwrap(), sid, "重复 create 必须复用存活会话");
    // fresh:true 才允许并行开第二个
    let (code, second) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": proj_path, "agent": "shell", "fresh": true})),
    );
    assert_eq!(code, 200);
    let sid2 = second["id"].as_str().unwrap().to_string();
    assert_ne!(sid2, sid, "fresh:true 必须开新会话");
    let (code, _) = http(
        "POST",
        port,
        &format!("/api/v1/sessions/{sid2}/kill"),
        Some(TOKEN),
        Some(serde_json::json!({})),
    );
    assert_eq!(code, 200);

    // ---- WS attach: hello frame, echo roundtrip ----
    let ws_url = format!("ws://127.0.0.1:{port}/api/v1/sessions/{sid}/attach?token={TOKEN}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("ws connect");
    let hello = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("hello timeout")
        .unwrap()
        .unwrap();
    let hello_v: Value = serde_json::from_str(hello.to_text().unwrap()).unwrap();
    assert_eq!(hello_v["t"], "hello");
    assert_eq!(hello_v["session"]["id"], sid.as_str());

    // type a command into the PTY (binary frame = raw input)
    ws.send(tokio_tungstenite::tungstenite::Message::Binary(
        b"echo smoke-$((40+2))-echo\r".to_vec().into(),
    ))
    .await
    .unwrap();
    // accumulate output until we see the evaluated marker
    let mut acc = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "echo output not seen; got: {}", String::from_utf8_lossy(&acc));
        let msg = tokio::time::timeout(remaining, ws.next()).await;
        let Ok(Some(Ok(msg))) = msg else {
            panic!("ws stream ended early; got: {}", String::from_utf8_lossy(&acc))
        };
        if msg.is_binary() {
            acc.extend_from_slice(&msg.into_data());
            if String::from_utf8_lossy(&acc).contains("smoke-42-echo") {
                break;
            }
        }
    }

    // ---- resize via WS control frame ----
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"t":"resize","cols":100,"rows":30}"#.to_string().into(),
    ))
    .await
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
        let s = find_session(&list, &sid);
        if s["cols"] == 100 && s["rows"] == 30 {
            break;
        }
        assert!(Instant::now() < deadline, "resize not applied: {s}");
        std::thread::sleep(Duration::from_millis(100));
    }

    // ---- input endpoint (composer path) ----
    let (code, _) = http(
        "POST",
        port,
        &format!("/api/v1/sessions/{sid}/input"),
        Some(TOKEN),
        Some(serde_json::json!({"text": "true", "enter": true})),
    );
    assert_eq!(code, 200);

    // ---- kill -> exited ----
    let (code, _) = http(
        "POST",
        port,
        &format!("/api/v1/sessions/{sid}/kill"),
        Some(TOKEN),
        None,
    );
    assert_eq!(code, 200);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
        let s = find_session(&list, &sid);
        if s["state"] == "exited" {
            break;
        }
        assert!(Instant::now() < deadline, "session did not exit: {s}");
        std::thread::sleep(Duration::from_millis(200));
    }
    drop(ws);

    // exited session persisted to the state dir
    let meta_file = env
        .home
        .join(".local/state/aaa-daemon/sessions")
        .join(format!("{sid}.json"));
    assert!(meta_file.exists(), "exited session metadata persisted");

    // ---- restart daemon: exited session must still be listed + replayable ----
    drop(guard);
    let guard2 = spawn_daemon(&env);
    let port2 = wait_port(&env);
    let (_, list) = http("GET", port2, "/api/v1/sessions", Some(TOKEN), None);
    let s = find_session(&list, &sid);
    assert_eq!(s["state"], "exited");
    assert_eq!(s["project_name"], "smoke-test-项目");
    // attach to the restored exited session: replay carries the old output
    let ws_url = format!("ws://127.0.0.1:{port2}/api/v1/sessions/{sid}/attach?token={TOKEN}");
    let (mut ws2, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let hello = tokio::time::timeout(Duration::from_secs(5), ws2.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(hello.is_text());
    let replay = tokio::time::timeout(Duration::from_secs(5), ws2.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(replay.is_binary());
    let replay_text = String::from_utf8_lossy(&replay.into_data()).into_owned();
    assert!(
        replay_text.contains("smoke-42-echo"),
        "replay after restart should contain the session output"
    );
    drop(ws2);

    // ---- delete the record ----
    let (code, _) = http(
        "DELETE",
        port2,
        &format!("/api/v1/sessions/{sid}"),
        Some(TOKEN),
        None,
    );
    assert_eq!(code, 200);
    let (_, list) = http("GET", port2, "/api/v1/sessions", Some(TOKEN), None);
    assert!(list.as_array().unwrap().iter().all(|s| s["id"] != sid.as_str()));
    assert!(!meta_file.exists(), "persisted record removed");
    drop(guard2);
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

fn http_upload(port: u16, path: &str, name: &str, bytes: &[u8]) -> (u16, Value) {
    let url = format!(
        "http://127.0.0.1:{port}/api/v1/projects/upload?path={}&name={}",
        urlencode(path),
        urlencode(name)
    );
    let result = ureq::post(&url)
        .set("Authorization", &format!("Bearer {TOKEN}"))
        .set("Content-Type", "application/octet-stream")
        .send_bytes(bytes);
    match result {
        Ok(resp) => {
            let status = resp.status();
            let v = serde_json::from_str(&resp.into_string().unwrap_or_default())
                .unwrap_or(Value::Null);
            (status, v)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let v = serde_json::from_str(&resp.into_string().unwrap_or_default())
                .unwrap_or(Value::Null);
            (code, v)
        }
        Err(e) => panic!("upload: {e}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_saves_into_project_inbox_dir() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);
    let (_, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "up"})),
    );
    let proj_path = proj["path"].as_str().unwrap().to_string();

    let (code, r1) = http_upload(port, &proj_path, "设计 稿 v2.png", b"PNGDATA");
    assert_eq!(code, 200, "{r1}");
    let saved1 = r1["saved_path"].as_str().unwrap().to_string();
    assert!(saved1.contains("/_inbox/"), "{saved1}");
    assert!(saved1.ends_with("-设计-稿-v2.png"), "slugified name: {saved1}");
    assert_eq!(std::fs::read(&saved1).unwrap(), b"PNGDATA");

    // same name again within the same second must not overwrite
    let (_, r2) = http_upload(port, &proj_path, "设计 稿 v2.png", b"OTHER");
    let saved2 = r2["saved_path"].as_str().unwrap().to_string();
    assert_ne!(saved1, saved2, "anti-overwrite");
    assert_eq!(std::fs::read(&saved1).unwrap(), b"PNGDATA", "first file intact");
    assert_eq!(std::fs::read(&saved2).unwrap(), b"OTHER");

    // path traversal / unknown dirs rejected
    let (code, _) = http_upload(port, "/etc", "x", b"nope");
    assert_eq!(code, 404, "arbitrary-but-missing/unaccepted dirs handled");
    let bad = format!("{proj_path}/../../../tmp");
    let (code, _) = http_upload(port, &bad, "x", b"nope");
    assert_eq!(code, 404, "parent-ref paths rejected");
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn inbox_auto_feed_on_first_waiting() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);

    // events subscriber to catch inbox_changed
    let ws_url = format!("ws://127.0.0.1:{port}/api/v1/events?token={TOKEN}");
    let (mut events_ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let _snapshot = tokio::time::timeout(Duration::from_secs(5), events_ws.next())
        .await
        .unwrap();

    // two projects: one feeding, one with feed_inbox:false
    let mk = |name: &str| {
        let (_, p) = http(
            "POST",
            port,
            "/api/v1/projects",
            Some(TOKEN),
            Some(serde_json::json!({"name": name})),
        );
        p["path"].as_str().unwrap().to_string()
    };
    let p_feed = mk("feed-on");
    let p_off = mk("feed-off");
    for (p, task) in [(&p_feed, "修复登录"), (&p_feed, "写测试"), (&p_off, "不该被喂")] {
        let (code, _) = http(
            "POST",
            port,
            "/api/v1/inbox",
            Some(TOKEN),
            Some(serde_json::json!({"path": p, "text": task})),
        );
        assert_eq!(code, 200);
    }

    let spawn_sess = |p: &str, feed: bool| {
        let (code, s) = http(
            "POST",
            port,
            "/api/v1/sessions",
            Some(TOKEN),
            Some(serde_json::json!({
                "project_path": p, "agent": "shell", "feed_inbox": feed
            })),
        );
        assert_eq!(code, 200);
        s["id"].as_str().unwrap().to_string()
    };
    let sid_feed = spawn_sess(&p_feed, true);
    let sid_off = spawn_sess(&p_off, false);

    // attach and park both shells on a blocking read: the screen goes quiet
    // -> waiting. A shell has no structured "dialog is up" signal (that gate
    // exists for claude only: `asking` / untrusted folder), so its first
    // waiting gets the queued entries typed in.
    let attach = |sid: String| async move {
        let url = format!("ws://127.0.0.1:{port}/api/v1/sessions/{sid}/attach?token={TOKEN}");
        let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(5), ws.next()).await; // hello
        ws.send(tokio_tungstenite::tungstenite::Message::Binary(
            "read -k 1 '?接下来做什么?'\r".as_bytes().to_vec().into(),
        ))
        .await
        .unwrap();
        ws
    };
    let mut ws_feed = attach(sid_feed.clone()).await;
    let _ws_off = attach(sid_off.clone()).await;

    // within ~15s: the feeding project's inbox drains and the composed task
    // list is typed into the PTY; the feed_inbox:false project is untouched
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut acc = Vec::new();
    let mut fed = false;
    while Instant::now() < deadline && !fed {
        while let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(200), ws_feed.next()).await
        {
            if msg.is_binary() {
                acc.extend_from_slice(&msg.into_data());
            }
        }
        // note: `read -k 1` consumes (unechoed) the first character of the
        // fed text, so match on the echoed remainder of "任务清单：…"
        fed = String::from_utf8_lossy(&acc).contains("务清单");
    }
    assert!(fed, "composed task list must appear in the PTY: {}", String::from_utf8_lossy(&acc));
    let text = String::from_utf8_lossy(&acc);
    assert!(text.contains("修复登录"), "{text}");
    assert!(text.contains("写测试"), "{text}");

    let enc_feed = urlencode(&p_feed);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, lst) = http(
            "GET",
            port,
            &format!("/api/v1/inbox?path={enc_feed}"),
            Some(TOKEN),
            None,
        );
        if lst.as_array().map(|a| a.is_empty()).unwrap_or(false) {
            break;
        }
        assert!(Instant::now() < deadline, "fed inbox must drain: {lst}");
        std::thread::sleep(Duration::from_millis(200));
    }
    // feed_inbox:false project's entry survives (its session also went waiting)
    let enc_off = urlencode(&p_off);
    let (_, lst_off) = http(
        "GET",
        port,
        &format!("/api/v1/inbox?path={enc_off}"),
        Some(TOKEN),
        None,
    );
    assert_eq!(lst_off.as_array().unwrap().len(), 1, "feed_inbox:false untouched");

    // inbox_changed must have been broadcast (from add and/or feed)
    let mut saw_inbox_changed = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !saw_inbox_changed {
        let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(300), events_ws.next()).await
        else {
            break;
        };
        if msg.is_text() {
            let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
            if v["t"] == "inbox_changed" {
                saw_inbox_changed = true;
            }
        }
    }
    assert!(saw_inbox_changed, "inbox_changed event expected");

    // 看板：终端会话不进看板，但形状必须齐
    let (code, dash) = http("GET", port, "/api/v1/history/dashboard", Some(TOKEN), None);
    assert_eq!(code, 200);
    assert!(dash["sessions"].as_array().unwrap().is_empty(), "终端不进看板");
    assert_eq!(dash["counts"]["open_items"], 0);

    // messages endpoint: shell sessions are unsupported
    let (code, m) = http(
        "GET",
        port,
        &format!("/api/v1/sessions/{sid_feed}/messages"),
        Some(TOKEN),
        None,
    );
    assert_eq!(code, 200);
    assert_eq!(m["supported"], false);
    assert_eq!(m["source"], "none");

    for sid in [&sid_feed, &sid_off] {
        let _ = http(
            "POST",
            port,
            &format!("/api/v1/sessions/{sid}/kill"),
            Some(TOKEN),
            None,
        );
    }
    drop(guard);
}

/// 删项目 = 目录 + 全部会话：还活着的会话必须先被结束并摘出池子，否则目录没了、
/// PTY 还在，cwd 成幽灵（手机上的删除入口一直允许对活着的项目按下去）。
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_project_kills_its_live_sessions() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);

    let (code, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "doomed", "agent": "shell"})),
    );
    assert_eq!(code, 200);
    let path = proj["path"].as_str().unwrap().to_string();
    let (code, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": path, "agent": "shell", "resume": false})),
    );
    assert_eq!(code, 200);
    let sid = sess["id"].as_str().unwrap().to_string();
    let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
    assert!(list.as_array().unwrap().iter().any(|s| s["id"] == sid), "会话应在池子里");

    // 同目录再来一个会话并让它先退出：混着已退出的回放和活着的会话一起删
    let (code, gone) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": path, "agent": "shell", "resume": false, "fresh": true})),
    );
    assert_eq!(code, 200);
    let gone_id = gone["id"].as_str().unwrap().to_string();
    let (code, _) = http("POST", port, &format!("/api/v1/sessions/{gone_id}/kill"), Some(TOKEN), None);
    assert_eq!(code, 200);

    let (code, resp) = http(
        "POST",
        port,
        "/api/v1/projects/delete",
        Some(TOKEN),
        Some(serde_json::json!({"paths": [path]})),
    );
    assert_eq!(code, 200);
    assert_eq!(resp["results"][0]["ok"], true);
    assert_eq!(resp["killed"].as_array().unwrap().len(), 2, "活的和已退出的回放都要收走");
    assert!(!Path::new(&path).exists(), "目录应已删除");

    let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
    for id in [&sid, &gone_id] {
        assert!(
            !list.as_array().unwrap().iter().any(|s| &s["id"] == id),
            "会话必须一起消失（含已退出的回放），否则 mac 侧栏会为「有会话但没登记」的目录补一行: {list}"
        );
    }
    // 日志里留着，并盖了删除戳
    let (_, hist) = http("GET", port, "/api/v1/history?limit=50", Some(TOKEN), None);
    let row = hist["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == sid)
        .expect("history keeps the record");
    assert!(row["deleted_at"].is_string());
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn ssd_guard_returns_503_and_never_mkdirs() {
    let env = setup_env();
    // simulate unmounted SSD
    std::fs::remove_dir_all(&env.root).unwrap();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);
    let (code, health) = http("GET", port, "/api/v1/health", Some(TOKEN), None);
    assert_eq!(code, 200);
    assert_eq!(health["ssd_mounted"], false);
    for (method, path, body) in [
        ("POST", "/api/v1/projects", serde_json::json!({"name":"x"})),
        ("POST", "/api/v1/projects/delete", serde_json::json!({"paths":["/x"]})),
        ("POST", "/api/v1/sessions", serde_json::json!({"project_path":"/x","agent":"shell"})),
    ] {
        let (code, resp) = http(method, port, path, Some(TOKEN), Some(body));
        assert_eq!(code, 503, "{path} must be 503 when unmounted, got {resp}");
        assert_eq!(resp["error"]["code"], "ssd_unmounted", "{path}");
    }
    assert!(!env.root.exists(), "daemon must never mkdir the project root");
    drop(guard);
}

#[tokio::test(flavor = "multi_thread")]
async fn events_ws_snapshot_and_session_updates() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);

    let ws_url = format!("ws://127.0.0.1:{port}/api/v1/events?token={TOKEN}");
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let snap = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let snap_v: Value = serde_json::from_str(snap.to_text().unwrap()).unwrap();
    assert_eq!(snap_v["t"], "snapshot");
    assert!(snap_v["sessions"].as_array().unwrap().is_empty());

    // wrong token refused at upgrade time
    let bad = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/api/v1/events?token=bad"
    ))
    .await;
    assert!(bad.is_err(), "events WS must reject a bad token");

    // spawning a session must surface as a (throttled) session event
    let (_, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "evt"})),
    );
    let proj_path = proj["path"].as_str().unwrap().to_string();
    let (code, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": proj_path, "agent": "shell"})),
    );
    assert_eq!(code, 200);
    let sid = sess["id"].as_str().unwrap().to_string();

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_projects_changed = false;
    let mut saw_session = false;
    while Instant::now() < deadline && !(saw_projects_changed && saw_session) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Ok(Some(Ok(msg))) = tokio::time::timeout(remaining, ws.next()).await else { break };
        if !msg.is_text() {
            continue;
        }
        let v: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        match v["t"].as_str() {
            Some("projects_changed") => saw_projects_changed = true,
            Some("session") if v["session"]["id"] == sid.as_str() => saw_session = true,
            _ => {}
        }
    }
    assert!(saw_projects_changed, "projects_changed event expected");
    assert!(saw_session, "session event for the new session expected");

    let (_, _) = http(
        "POST",
        port,
        &format!("/api/v1/sessions/{sid}/kill"),
        Some(TOKEN),
        None,
    );
    drop(guard);
}
