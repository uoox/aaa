//! 网络层：tokio 后台运行时 + reqwest REST + tokio-tungstenite WS。
//!
//! gpui 有自己的 executor，reqwest/tungstenite 需要 tokio reactor：
//! 全部 IO 在一个常驻 tokio 运行时里跑，结果经 futures channel 送回 UI
//! （futures channel 与 executor 无关，gpui 任务可直接 await/poll）。

use std::sync::{Arc, OnceLock, RwLock};

use anyhow::{Result, anyhow};
use futures::channel::{mpsc, oneshot};
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::Message;

use crate::model::*;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Connected,
    Disconnected,
}

/// 送回 UI 的事件
#[derive(Debug)]
pub enum UiEvent {
    Conn(ConnState),
    Daemon(DaemonEvent),
    TermHello {
        id: String,
        session: Box<Session>,
        rows: u16,
        cols: u16,
    },
    TermData {
        id: String,
        bytes: Vec<u8>,
    },
    /// attach 连接断开（会自动重连；UI 可显示提示）
    TermDown {
        id: String,
    },
}

/// attach 通道上行消息
pub enum AttachMsg {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

#[derive(Clone)]
pub struct AttachHandle {
    #[allow(dead_code)]
    pub id: String,
    tx: tokio::sync::mpsc::UnboundedSender<AttachMsg>,
}

impl AttachHandle {
    pub fn input(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(AttachMsg::Input(bytes));
    }
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(AttachMsg::Resize { cols, rows });
    }
}

#[derive(Clone)]
pub struct Net {
    endpoint: Arc<RwLock<Option<Endpoint>>>,
    http: reqwest::Client,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
    /// endpoint 变化时唤醒 events 重连循环
    wake: Arc<tokio::sync::Notify>,
}

impl Net {
    /// 返回 (Net, UI 事件接收端)。调用方负责在 gpui 任务里消费接收端。
    pub fn new(endpoint: Option<Endpoint>) -> (Self, mpsc::UnboundedReceiver<UiEvent>) {
        let (ui_tx, ui_rx) = mpsc::unbounded();
        let net = Net {
            endpoint: Arc::new(RwLock::new(endpoint)),
            http: reqwest::Client::new(),
            ui_tx,
            wake: Arc::new(tokio::sync::Notify::new()),
        };
        net.spawn_events_loop();
        (net, ui_rx)
    }

    /// 测试用：不启动 /events 循环（避免抢占单次 accept 的测试服务器）
    #[cfg(test)]
    fn new_quiet(endpoint: Option<Endpoint>) -> Self {
        let (ui_tx, _ui_rx) = mpsc::unbounded();
        Net {
            endpoint: Arc::new(RwLock::new(endpoint)),
            http: reqwest::Client::new(),
            ui_tx,
            wake: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn endpoint(&self) -> Option<Endpoint> {
        self.endpoint.read().unwrap().clone()
    }

    pub fn set_endpoint(&self, ep: Endpoint) {
        *self.endpoint.write().unwrap() = Some(ep);
        self.wake.notify_waiters();
    }

    // ── REST ────────────────────────────────────────────────────────────

    fn request_raw(
        &self,
        method: reqwest::Method,
        path: String,
        body: Option<serde_json::Value>,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        let (tx, rx) = oneshot::channel();
        let ep = self.endpoint();
        let http = self.http.clone();
        runtime().spawn(async move {
            let res = do_request(http, ep, method, path, body).await;
            let _ = tx.send(res);
        });
        async move { rx.await.unwrap_or_else(|_| Err(anyhow!("网络任务中断"))) }
    }

    fn get_json<T: DeserializeOwned + 'static>(
        &self,
        path: &str,
    ) -> impl Future<Output = Result<T>> + use<T> {
        let fut = self.request_raw(reqwest::Method::GET, path.to_string(), None);
        async move { Ok(serde_json::from_value(fut.await?)?) }
    }

    fn post_json<T: DeserializeOwned + 'static>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> impl Future<Output = Result<T>> + use<T> {
        let fut = self.request_raw(reqwest::Method::POST, path.to_string(), Some(body));
        async move { Ok(serde_json::from_value(fut.await?)?) }
    }

    pub fn health(&self) -> impl Future<Output = Result<Health>> + use<> {
        self.get_json("/health")
    }
    pub fn agents(&self) -> impl Future<Output = Result<Vec<AgentInfo>>> + use<> {
        self.get_json("/agents")
    }
    pub fn projects(&self) -> impl Future<Output = Result<Vec<Project>>> + use<> {
        self.get_json("/projects")
    }
    pub fn sessions(&self) -> impl Future<Output = Result<Vec<Session>>> + use<> {
        self.get_json("/sessions")
    }
    pub fn create_project(
        &self,
        name: Option<String>,
        agent: Option<String>,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/projects", serde_json::json!({"name": name, "agent": agent}))
    }
    pub fn delete_projects(
        &self,
        paths: Vec<String>,
    ) -> impl Future<Output = Result<DeleteResponse>> + use<> {
        self.post_json("/projects/delete", serde_json::json!({ "paths": paths }))
    }
    pub fn set_project_agent(
        &self,
        path: String,
        agent: String,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/projects/agent", serde_json::json!({"path": path, "agent": agent}))
    }
    pub fn create_session(
        &self,
        project_path: String,
        agent: String,
        resume: bool,
    ) -> impl Future<Output = Result<Session>> + use<> {
        self.post_json(
            "/sessions",
            serde_json::json!({"project_path": project_path, "agent": agent, "resume": resume}),
        )
    }
    pub fn session_input(
        &self,
        id: &str,
        text: String,
        enter: bool,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(
            &format!("/sessions/{id}/input"),
            serde_json::json!({"text": text, "enter": enter}),
        )
    }
    pub fn kill_session(&self, id: &str) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(&format!("/sessions/{id}/kill"), serde_json::json!({}))
    }
    pub fn delete_session(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.request_delete(format!("/sessions/{id}"))
    }
    fn request_delete(
        &self,
        path: String,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.request_raw(reqwest::Method::DELETE, path, None)
    }
    pub fn rename_session(
        &self,
        id: &str,
        title: String,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(
            &format!("/sessions/{id}/rename"),
            serde_json::json!({ "title": title }),
        )
    }
    pub fn permissions(&self) -> impl Future<Output = Result<Vec<Permission>>> + use<> {
        self.get_json("/mac/permissions")
    }
    pub fn request_permissions(&self) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/mac/permissions/request", serde_json::json!({"ids": ["all"]}))
    }
    pub fn pair(&self) -> impl Future<Output = Result<PairResponse>> + use<> {
        self.get_json("/pair")
    }
    pub fn ports(&self, id: &str) -> impl Future<Output = Result<Vec<PortEntry>>> + use<> {
        self.get_json(&format!("/sessions/{id}/ports"))
    }

    // ── /events WS：断线指数退避重连 ─────────────────────────────────────

    fn spawn_events_loop(&self) {
        let endpoint = self.endpoint.clone();
        let ui_tx = self.ui_tx.clone();
        let wake = self.wake.clone();
        runtime().spawn(async move {
            let mut backoff_ms: u64 = 500;
            loop {
                let ep = endpoint.read().unwrap().clone();
                let Some(ep) = ep else {
                    let _ = ui_tx.unbounded_send(UiEvent::Conn(ConnState::Disconnected));
                    // 无 endpoint：等待手动配置
                    wake.notified().await;
                    continue;
                };
                let _ = ui_tx.unbounded_send(UiEvent::Conn(ConnState::Connecting));
                let url = format!("{}/events?token={}", ep.ws_base(), ep.token);
                match tokio_tungstenite::connect_async(&url).await {
                    Ok((mut ws, _)) => {
                        backoff_ms = 500;
                        let _ = ui_tx.unbounded_send(UiEvent::Conn(ConnState::Connected));
                        use futures::StreamExt;
                        loop {
                            tokio::select! {
                                msg = ws.next() => match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        match serde_json::from_str::<DaemonEvent>(&text) {
                                            Ok(ev) => {
                                                let _ = ui_tx.unbounded_send(UiEvent::Daemon(ev));
                                            }
                                            Err(e) => log::warn!("events 帧解析失败: {e}: {text}"),
                                        }
                                    }
                                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_))) => {}
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                },
                                _ = wake.notified() => break, // endpoint 变了，立即重连
                            }
                        }
                    }
                    Err(e) => {
                        log::debug!("events 连接失败: {e}");
                    }
                }
                let _ = ui_tx.unbounded_send(UiEvent::Conn(ConnState::Disconnected));
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
                    _ = wake.notified() => {}
                }
                backoff_ms = (backoff_ms * 2).min(15_000);
            }
        });
    }

    // ── attach WS ───────────────────────────────────────────────────────

    /// 为会话建立 attach 连接。返回的 handle 丢弃后连接自动关闭。
    pub fn attach(&self, id: &str) -> AttachHandle {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AttachMsg>();
        let endpoint = self.endpoint.clone();
        let ui_tx = self.ui_tx.clone();
        let sid = id.to_string();
        runtime().spawn(async move {
            let mut backoff_ms: u64 = 500;
            let mut last_resize: Option<(u16, u16)> = None;
            'outer: loop {
                let ep = endpoint.read().unwrap().clone();
                let Some(ep) = ep else {
                    let _ = ui_tx.unbounded_send(UiEvent::TermDown { id: sid.clone() });
                    return;
                };
                let url = format!("{}/sessions/{}/attach?token={}", ep.ws_base(), sid, ep.token);
                match tokio_tungstenite::connect_async(&url).await {
                    Ok((mut ws, _)) => {
                        backoff_ms = 500;
                        use futures::{SinkExt, StreamExt};
                        // 重连后重放最后一次 resize，保证行列一致
                        if let Some((cols, rows)) = last_resize {
                            let _ = ws.send(Message::Text(resize_frame(cols, rows).into())).await;
                        }
                        loop {
                            tokio::select! {
                                msg = ws.next() => match msg {
                                    Some(Ok(Message::Binary(data))) => {
                                        let _ = ui_tx.unbounded_send(UiEvent::TermData {
                                            id: sid.clone(),
                                            bytes: data.to_vec(),
                                        });
                                    }
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(AttachServerMsg::Hello { session, rows, cols }) =
                                            serde_json::from_str::<AttachServerMsg>(&text)
                                        {
                                            let _ = ui_tx.unbounded_send(UiEvent::TermHello {
                                                id: sid.clone(),
                                                session: Box::new(session),
                                                rows,
                                                cols,
                                            });
                                        }
                                    }
                                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                },
                                up = rx.recv() => match up {
                                    Some(AttachMsg::Input(bytes)) => {
                                        if ws.send(Message::Binary(bytes.into())).await.is_err() {
                                            break;
                                        }
                                    }
                                    Some(AttachMsg::Resize { cols, rows }) => {
                                        last_resize = Some((cols, rows));
                                        if ws
                                            .send(Message::Text(resize_frame(cols, rows).into()))
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    None => {
                                        // handle 丢弃：优雅关闭
                                        let _ = ws.close(None).await;
                                        break 'outer;
                                    }
                                },
                            }
                        }
                    }
                    Err(e) => log::debug!("attach {sid} 连接失败: {e}"),
                }
                let _ = ui_tx.unbounded_send(UiEvent::TermDown { id: sid.clone() });
                // handle 已丢弃则退出
                if rx.is_closed() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                backoff_ms = (backoff_ms * 2).min(15_000);
            }
        });
        AttachHandle {
            id: id.to_string(),
            tx,
        }
    }
}

async fn do_request(
    http: reqwest::Client,
    ep: Option<Endpoint>,
    method: reqwest::Method,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value> {
    let ep = ep.ok_or_else(|| anyhow!("未连接：无 daemon 地址"))?;
    let url = format!("{}{}", ep.http_base(), path);
    let mut req = http
        .request(method, &url)
        .header("Authorization", format!("Bearer {}", ep.token))
        .timeout(std::time::Duration::from_secs(15));
    if let Some(b) = body {
        req = req.json(&b);
    }
    let resp = req.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if let Ok(e) = serde_json::from_str::<ApiError>(&text) {
            return Err(anyhow!("{}: {}", e.error.code, e.error.message));
        }
        return Err(anyhow!("HTTP {}", status.as_u16()));
    }
    if text.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 极简单次 HTTP 应答服务器：接收一个请求，返回给定 body；请求原文送回
    fn one_shot_server(
        status_line: &'static str,
        body: &'static str,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 8192];
            // 读满 header + content-length 指定的 body（POST body 可能分包到达）
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data);
                if let Some(hdr_end) = text.find("\r\n\r\n") {
                    let clen = text
                        .lines()
                        .find(|l| l.to_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if data.len() >= hdr_end + 4 + clen {
                        break;
                    }
                }
            }
            let req = String::from_utf8_lossy(&data).to_string();
            let resp = format!(
                "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(resp.as_bytes()).unwrap();
            let _ = tx.send(req);
        });
        (port, rx)
    }

    fn test_net(port: u16) -> Net {
        Net::new_quiet(Some(Endpoint {
            host: "127.0.0.1".into(),
            port,
            token: "aaa_tk_test".into(),
        }))
    }

    #[test]
    fn rest_get_sends_bearer_and_parses() {
        let (port, req_rx) = one_shot_server(
            "HTTP/1.1 200 OK",
            r#"{"version":"0.1.0","ssd_mounted":true,"project_root":"/Volumes/SSD/project","uptime_s":42}"#,
        );
        let net = test_net(port);
        let health = futures::executor::block_on(net.health()).unwrap();
        assert_eq!(health.version, "0.1.0");
        assert!(health.ssd_mounted);
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("GET /api/v1/health HTTP/1.1"), "req: {req}");
        assert!(
            req.to_lowercase().contains("authorization: bearer aaa_tk_test"),
            "缺 Bearer 头: {req}"
        );
    }

    #[test]
    fn rest_post_body_and_error_mapping() {
        let (port, req_rx) = one_shot_server(
            "HTTP/1.1 503 Service Unavailable",
            r#"{"error":{"code":"ssd_unmounted","message":"SSD 未挂载"}}"#,
        );
        let net = test_net(port);
        let err = futures::executor::block_on(net.create_session(
            "/Volumes/SSD/project/x".into(),
            "claude".into(),
            true,
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ssd_unmounted"), "错误未带 code: {msg}");
        assert!(msg.contains("SSD 未挂载"), "错误未带 message: {msg}");
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("POST /api/v1/sessions HTTP/1.1"));
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let v: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(v["project_path"], "/Volumes/SSD/project/x");
        assert_eq!(v["agent"], "claude");
        assert_eq!(v["resume"], true);
    }

    #[test]
    fn rest_input_options_key() {
        // waiting 会话快捷作答：POST input {"text":"1","enter":true}
        let (port, req_rx) = one_shot_server("HTTP/1.1 200 OK", "{}");
        let net = test_net(port);
        futures::executor::block_on(net.session_input("s_1", "1".into(), true)).unwrap();
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("POST /api/v1/sessions/s_1/input HTTP/1.1"));
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let v: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(v["text"], "1");
        assert_eq!(v["enter"], true);
    }

    #[test]
    fn no_endpoint_fails_gracefully() {
        let net = Net::new_quiet(None);
        let err = futures::executor::block_on(net.health()).unwrap_err();
        assert!(err.to_string().contains("未连接"));
    }
}
