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

    // ---- agent 表 + 换 agent ----
    // 表里是 claude 与 agy，没有 shell（终端是面板，不是 agent）
    let (code, list) = http("GET", port, "/api/v1/agents", Some(TOKEN), None);
    assert_eq!(code, 200);
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["claude", "agy"]);
    assert!(list[0]["available"].is_boolean() && list[0]["label"] == "Claude");

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

/// v1.27 两个批量口：紧急制动（收掉在跑的项目会话，终端不动）与清空已退出的记录。
#[tokio::test(flavor = "multi_thread")]
async fn kill_all_spares_terminals_and_clean_exited_clears_the_pool() {
    let env = setup_env();
    let guard = spawn_daemon(&env);
    let port = wait_port(&env);

    let (_, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "brake"})),
    );
    let path = proj["path"].as_str().unwrap().to_string();
    // 一个「项目会话」（这里用 shell 当替身：测试环境里没有 claude）与一个终端。
    // 项目会话之所以是项目会话，看的是 agent 不等于 shell——所以这里改用
    // 一个真的项目会话拿不到，退而验证「终端不会被收」这一条。
    let (_, term) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": path, "agent": "shell", "resume": false, "fresh": true})),
    );
    let term_id = term["id"].as_str().unwrap().to_string();

    let (code, r) = http("POST", port, "/api/v1/sessions/kill_all", Some(TOKEN), Some(serde_json::json!({})));
    assert_eq!(code, 200, "静态段不能被 /sessions/{{id}} 抢走");
    assert_eq!(r["count"], 0, "终端不算项目会话，一个都不该收");
    let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
    assert_eq!(list.as_array().unwrap().len(), 1, "终端还活着");

    // 收掉终端（这条走单会话的口），它就成了池子里一条已退出的记录
    let (code, _) = http("POST", port, &format!("/api/v1/sessions/{term_id}/kill"), Some(TOKEN), None);
    assert_eq!(code, 200);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (_, l) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
        if l[0]["state"] == "exited" {
            break;
        }
        assert!(Instant::now() < deadline, "终端没有退出");
        std::thread::sleep(Duration::from_millis(100));
    }

    let (code, r) = http("POST", port, "/api/v1/sessions/clean_exited", Some(TOKEN), None);
    assert_eq!(code, 200);
    assert_eq!(r["removed"], 1);
    let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
    assert!(list.as_array().unwrap().is_empty(), "池子清空了");
    // 会话日志里记录还在（盖了删除戳），项目目录也还在
    let (_, hist) = http("GET", port, "/api/v1/history", Some(TOKEN), None);
    assert!(hist["entries"].as_array().unwrap().iter().any(|e| e["id"] == term_id.as_str()));
    assert!(Path::new(&path).is_dir(), "只清记录，不动目录");
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
    // 日志里留着，并盖了删除戳。这条断言是 GET /history 存在的理由：看板不收终端，
    // 只有历史账本收——2026-09-08 审计说这个路由零调用方，其实它是账本唯一的读出口
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

/// v1.22 的契约：daemon 把「一件事」算好下发，客户端只画（PROTOCOL「版本兼容」）。
/// 两端删掉了自己那套推导，所以这几个字段少一个都是线上事故——端到端钉死。
#[tokio::test(flavor = "multi_thread")]
async fn projects_and_sessions_carry_the_derived_fields() {
    let env = setup_env();
    let _guard = spawn_daemon(&env);
    let port = wait_port(&env);

    // schema 是客户端唯一的兼容闸门：它说 >=2，客户端才敢直接用下面这些字段
    let (code, health) = http("GET", port, "/api/v1/health", Some(TOKEN), None);
    assert_eq!(code, 200);
    assert_eq!(health["schema"], aaa_daemon::SCHEMA, "/health 必须带 schema");

    let (code, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "derived", "agent": "shell"})),
    );
    assert_eq!(code, 200, "{proj}");
    let path = proj["path"].as_str().unwrap().to_string();

    let (code, projects) = http("GET", port, "/api/v1/projects", Some(TOKEN), None);
    assert_eq!(code, 200);
    let row = projects
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["path"] == path.as_str())
        .unwrap_or_else(|| panic!("新建的项目不在列表里: {projects}"));
    // 一个会话都还没有：五态是 paused，标题退到目录名，排序时间退到目录 mtime
    assert_eq!(row["status"], "paused");
    assert_eq!(row["title"], "derived");
    assert!(row["session_id"].is_null(), "还没有会话");
    assert!(row["updated_at"].as_str().is_some_and(|s| s.contains('T')), "{row}");
    assert_eq!(row["registered"], true);

    // 会话对象：status / asking_seq / checklist 三样都在（shell 也一样带）
    let (code, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": path, "agent": "shell", "resume": false})),
    );
    assert_eq!(code, 200, "{sess}");
    let id = sess["id"].as_str().unwrap().to_string();
    let (_, list) = http("GET", port, "/api/v1/sessions", Some(TOKEN), None);
    let s = find_session(&list, &id);
    assert!(
        ["running", "waiting", "active", "asking", "background", "paused"]
            .contains(&s["status"].as_str().unwrap_or("")),
        "会话要带 status: {s}"
    );
    assert!(s["asking_seq"].is_null(), "shell 没有结构化表单");
    assert_eq!(s["checklist"], serde_json::json!([]), "清单由 daemon 解析好，空的就是空数组");

    // 终端不代表项目：开了 shell 会话，项目行仍然是 paused（PROTOCOL「终端」）
    let (_, projects) = http("GET", port, "/api/v1/projects", Some(TOKEN), None);
    let row = projects.as_array().unwrap().iter().find(|p| p["path"] == path.as_str()).unwrap();
    assert_eq!(row["status"], "paused", "shell 不进项目列表的状态判定");
    assert!(row["session_id"].is_null(), "shell 不是项目的代表会话");
}

/// 断线期间发生的事，重连后必须靠**全量 snapshot** 找齐——这是 `/events` 唯一的补齐机制，
/// 所以 v1.23 明确钉住它（评审提过「事件流该加 revision」：不需要，daemon 在**掉帧**
/// (broadcast Lagged) 和**重连**两种情形下都补发 snapshot，而 snapshot 是整表替换、
/// 两端都照单换掉自己那份列表，倒退不了）。
#[tokio::test(flavor = "multi_thread")]
async fn events_reconnect_snapshot_is_authoritative() {
    let env = setup_env();
    let _guard = spawn_daemon(&env);
    let port = wait_port(&env);
    let ws_url = format!("ws://127.0.0.1:{port}/api/v1/events?token={TOKEN}");

    // 第一次连上：空的
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let snap = tokio::time::timeout(Duration::from_secs(5), ws.next()).await.unwrap().unwrap().unwrap();
    let v: Value = serde_json::from_str(snap.to_text().unwrap()).unwrap();
    assert_eq!(v["t"], "snapshot");
    assert!(v["sessions"].as_array().unwrap().is_empty());

    // 断开，然后在「断线期间」开一个会话——这几帧它一条都收不到
    drop(ws);
    let (_, proj) = http("POST", port, "/api/v1/projects", Some(TOKEN), Some(serde_json::json!({"name": "recon"})));
    let path = proj["path"].as_str().unwrap().to_string();
    let (code, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": path, "agent": "shell"})),
    );
    assert_eq!(code, 200, "{sess}");
    let sid = sess["id"].as_str().unwrap().to_string();

    // 重连：新的 snapshot 里必须有它，而且带着 v1.22 那几个派生字段
    let (mut ws2, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();
    let snap2 = tokio::time::timeout(Duration::from_secs(5), ws2.next()).await.unwrap().unwrap().unwrap();
    let v2: Value = serde_json::from_str(snap2.to_text().unwrap()).unwrap();
    assert_eq!(v2["t"], "snapshot");
    let found = v2["sessions"].as_array().unwrap().iter().find(|s| s["id"] == sid.as_str());
    let found = found.unwrap_or_else(|| panic!("重连的 snapshot 里没有断线期间开的会话: {v2}"));
    assert!(found["status"].as_str().is_some(), "snapshot 里的会话也要带 status");
    assert!(found["queued"].is_array(), "queued 也在");
}


/// v1.35 产物里的 Markdown：`GET /sessions/:id/artifacts` 带上项目里的 md 清单，
/// `GET /files/read` 读得到正文，出项目根一律 404。守卫在 daemon 这一层做完，
/// 两端只画——所以这条必须端到端跑，而不是只测 `files.rs` 里的纯函数。
#[test]
fn project_docs_are_listed_and_readable_but_never_outside_the_root() {
    let env = setup_env();
    let _guard = spawn_daemon(&env);
    let port = wait_port(&env);

    let (_, proj) = http(
        "POST",
        port,
        "/api/v1/projects",
        Some(TOKEN),
        Some(serde_json::json!({"name": "browse"})),
    );
    let dir = proj["path"].as_str().unwrap().to_string();
    std::fs::create_dir_all(Path::new(&dir).join("docs")).unwrap();
    std::fs::create_dir_all(Path::new(&dir).join("node_modules")).unwrap();
    std::fs::write(Path::new(&dir).join("NOTE.md"), "# 标题\n正文").unwrap();
    std::fs::write(Path::new(&dir).join("docs/api.md"), "# api").unwrap();
    std::fs::write(Path::new(&dir).join("app.rs"), "fn main(){}").unwrap();
    std::fs::write(Path::new(&dir).join("node_modules/dep.md"), "# dep").unwrap();

    let (_, sess) = http(
        "POST",
        port,
        "/api/v1/sessions",
        Some(TOKEN),
        Some(serde_json::json!({"project_path": dir, "agent": "shell"})),
    );
    let sid = sess["id"].as_str().unwrap().to_string();

    let (code, arts) = http("GET", port, &format!("/api/v1/sessions/{sid}/artifacts"), Some(TOKEN), None);
    assert_eq!(code, 200);
    let mut rels: Vec<&str> = arts["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["rel"].as_str().unwrap())
        .collect();
    rels.sort();
    assert_eq!(rels, vec!["NOTE.md", "docs/api.md"], "只有 md，装依赖的目录不进来");

    let (code, file) = http(
        "GET",
        port,
        &format!("/api/v1/files/read?path={dir}/NOTE.md"),
        Some(TOKEN),
        None,
    );
    assert_eq!(code, 200);
    assert_eq!(file["text"], "# 标题\n正文");
    assert_eq!(file["kind"], "markdown");

    // 出根：`..` 与根之外的绝对路径都是 404，不是「读到了别人的文件」
    let (code, _) = http("GET", port, "/api/v1/files/read?path=/etc/hosts", Some(TOKEN), None);
    assert_eq!(code, 404);
    let (code, _) = http("GET", port, &format!("/api/v1/files/read?path={dir}/../../etc/hosts"), Some(TOKEN), None);
    assert_eq!(code, 404, "`..` 爬不出去");
    // 老路由整个没了（目录 explorer 2026-09-10 删掉）
    let (code, _) = http("GET", port, &format!("/api/v1/files?path={dir}"), Some(TOKEN), None);
    assert_eq!(code, 404, "「浏览」那条列目录的路由不再存在");
}
