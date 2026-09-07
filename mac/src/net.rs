//! 网络层：tokio 后台运行时 + reqwest REST + tokio-tungstenite WS。
//!
//! gpui 有自己的 executor，reqwest/tungstenite 需要 tokio reactor：
//! 全部 IO 在一个常驻 tokio 运行时里跑，结果经 futures channel 送回 UI
//! （futures channel 与 executor 无关，gpui 任务可直接 await/poll）。

use std::sync::{Arc, OnceLock, RwLock};

use anyhow::{Result, anyhow};
use futures::channel::{mpsc, oneshot};
use serde::de::DeserializeOwned;
use tokio_tungstenite::tungstenite::{Bytes, Message};

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
// 变体大小差异是刻意的：这些是每帧从 WS 反序列化出来、立刻被消费掉的短命值，
// 装箱换来的是每个事件一次堆分配，比多占几百字节栈更贵。
#[allow(clippy::large_enum_variant)]
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
        /// WS 帧 payload 原样转发（Bytes 引用计数，避免每帧拷贝）
        bytes: Bytes,
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
    pub fn projects(&self) -> impl Future<Output = Result<Vec<Project>>> + use<> {
        self.get_json("/projects")
    }
    pub fn sessions(&self) -> impl Future<Output = Result<Vec<Session>>> + use<> {
        self.get_json("/sessions")
    }
    /// 看板：待办 + 按天流水 + 数字（daemon 一次算好）
    pub fn history_dashboard(&self) -> impl Future<Output = Result<Dashboard>> + use<> {
        self.get_json::<Dashboard>("/history/dashboard")
    }
    pub fn create_project(
        &self,
        name: Option<String>,
        agent: Option<String>,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/projects", serde_json::json!({"name": name, "agent": agent}))
    }
    /// 置顶 / 取消置顶（daemon 侧存；随后 projects_changed 帧会让列表重拉）
    /// 归档 / 取消归档（v1.15）：daemon 侧存；归档时活着的会话被结束（不确认）
    pub fn set_archived(&self, path: &str, archived: bool) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/projects/archive", serde_json::json!({ "path": path, "archived": archived }))
    }
    pub fn set_pinned(&self, path: &str, pinned: bool) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/projects/pin", serde_json::json!({ "path": path, "pinned": pinned }))
    }
    pub fn delete_projects(
        &self,
        paths: Vec<String>,
    ) -> impl Future<Output = Result<DeleteResponse>> + use<> {
        self.post_json("/projects/delete", serde_json::json!({ "paths": paths }))
    }
    /// 改 daemon 配置（写盘 + daemon 自我重启；有存活会话会被 409 拒绝）。
    /// `migrate`: project_root 变化时整根迁移（移动目录 + 注册表重写）。
    pub fn put_config(
        &self,
        port: Option<u16>,
        token: Option<String>,
        project_root: Option<String>,
        migrate: bool,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.request_raw(
            reqwest::Method::PUT,
            "/config".into(),
            Some(serde_json::json!({
                "port": port, "token": token,
                "project_root": project_root, "migrate": migrate,
            })),
        )
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
    /// 开一个终端（shell 会话）。`fresh:true` 是关键：POST /sessions 默认幂等
    /// （同目录同 agent 有存活会话就直接返回它），终端面板的「+」要的正是
    /// 第二个 shell，所以必须显式要求新开。shell 无 resume。
    pub fn create_terminal(
        &self,
        project_path: String,
        fresh: bool,
    ) -> impl Future<Output = Result<Session>> + use<> {
        self.post_json(
            "/sessions",
            serde_json::json!({
                "project_path": project_path, "agent": "shell",
                "resume": false, "fresh": fresh,
            }),
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
    /// 回答当前待答的 AskUserQuestion 表单：一项对应一题，顺序同 `question.questions`。
    /// daemon 负责翻译成对话框按键并确认对话框关闭。409（`ApiFailure::status`）=
    /// 没有待答问题 / 非 claude 会话 / 对话框没吃下——调用方提示用户去终端收尾。
    /// v1.16：替用户答权限对话框（allow / deny）
    pub fn session_permission(&self, id: &str, behavior: &str) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(&format!("/sessions/{id}/permission"), serde_json::json!({ "behavior": behavior }))
    }
    /// v1.16：看板上勾 / 取消勾清单项
    pub fn session_checklist(&self, id: &str, text: &str, done: bool) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(&format!("/sessions/{id}/checklist"), serde_json::json!({ "text": text, "done": done }))
    }
    pub fn session_answer(
        &self,
        id: &str,
        answers: Vec<AnswerItem>,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json(
            &format!("/sessions/{id}/answer"),
            serde_json::json!({ "answers": answers }),
        )
    }
    /// `POST /restart`：force = 连存活会话一起终止
    pub fn restart_daemon(&self, force: bool) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/restart", serde_json::json!({ "force": force }))
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
    pub fn pair(&self) -> impl Future<Output = Result<PairResponse>> + use<> {
        self.get_json("/pair")
    }
    /// `POST /projects/upload?path=&name=`：原始字节进项目 `_inbox/`，回 `saved_path`
    pub fn upload(
        &self,
        project_path: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<String>> + use<> {
        let (tx, rx) = oneshot::channel();
        let ep = self.endpoint();
        let http = self.http.clone();
        let path = format!(
            "/projects/upload?path={}&name={}",
            percent_encode(project_path),
            percent_encode(name)
        );
        runtime().spawn(async move {
            let res: Result<String> = async {
                let ep = ep.ok_or_else(|| anyhow!("未连接：无 daemon 地址"))?;
                let url = format!("{}{}", ep.http_base(), path);
                let resp = http
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", ep.token))
                    .header("Content-Type", "application/octet-stream")
                    .timeout(std::time::Duration::from_secs(120))
                    .body(bytes)
                    .send()
                    .await?;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                if !status.is_success() {
                    let (code, message) = serde_json::from_str::<ApiError>(&text)
                        .map(|e| (e.error.code, e.error.message))
                        .unwrap_or_default();
                    return Err(ApiFailure { status: status.as_u16(), code, message }.into());
                }
                let v: serde_json::Value = serde_json::from_str(&text)?;
                v["saved_path"]
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow!("响应缺 saved_path"))
            }
            .await;
            let _ = tx.send(res);
        });
        async move { rx.await.unwrap_or_else(|_| Err(anyhow!("网络任务中断"))) }
    }
    pub fn ports(&self, id: &str) -> impl Future<Output = Result<Vec<PortEntry>>> + use<> {
        self.get_json(&format!("/sessions/{id}/ports"))
    }
    /// v1.1 消息流：after = 已见最大 seq，增量拉取
    pub fn messages(
        &self,
        id: &str,
        after: u64,
    ) -> impl Future<Output = Result<MessagesResponse>> + use<> {
        self.get_json(&format!("/sessions/{id}/messages?after={after}&limit=500"))
    }

    /// v1.4 套餐用量（`plan` 为 null = 没有套餐信息）
    pub fn usage(&self) -> impl Future<Output = Result<UsageResponse>> + use<> {
        self.get_json("/usage")
    }
    /// 会话发布过的 Artifact 页面
    pub fn artifacts(&self, id: &str) -> impl Future<Output = Result<ArtifactsResponse>> + use<> {
        self.get_json(&format!("/sessions/{id}/artifacts"))
    }
    /// 项目收件箱
    pub fn inbox(&self, project_path: &str) -> impl Future<Output = Result<Vec<InboxItem>>> + use<> {
        self.get_json(&format!("/inbox?path={}", percent_encode(project_path)))
    }
    pub fn inbox_add(
        &self,
        project_path: &str,
        text: &str,
    ) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.post_json("/inbox", serde_json::json!({ "path": project_path, "text": text }))
    }
    pub fn inbox_delete(&self, id: &str) -> impl Future<Output = Result<serde_json::Value>> + use<> {
        self.request_delete(format!("/inbox/{}", percent_encode(id)))
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
                                // 读超时：daemon 每 20s 一个 Ping，45s 一帧都没有
                                // 就是半开连接（overlay 网络的经典死法），重连
                                _ = tokio::time::sleep(std::time::Duration::from_secs(45)) => break,
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
                                            bytes: data,
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
        AttachHandle { tx }
    }
}

/// REST 非 2xx。Display 仍是 `code: message`（toast 文案不变），但把 HTTP 状态码
/// 一并带着：回答表单要区分 409（没有待答问题 / 对话框没吃下 → 让用户去终端收尾）
/// 与其它失败，在字符串里找 "409" 不可靠。`anyhow::Error::downcast_ref` 取回。
#[derive(Debug, Clone)]
pub struct ApiFailure {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code.is_empty(), self.message.is_empty()) {
            (true, true) => write!(f, "HTTP {}", self.status),
            (true, false) => f.write_str(&self.message),
            (false, true) => write!(f, "{} (HTTP {})", self.code, self.status),
            (false, false) => write!(f, "{}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ApiFailure {}

/// 从请求错误里取 HTTP 状态码；不是 REST 层失败（没连上、超时、解析错）就 None
/// 查询串里的路径 / 文件名：RFC 3986 unreserved 之外的一律 %XX
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn http_status(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<ApiFailure>().map(|f| f.status)
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
        // daemon 的 `{"error":{code,message}}`；非 JSON 体（代理、v1 daemon 404）就只剩状态码
        let (code, message) = serde_json::from_str::<ApiError>(&text)
            .map(|e| (e.error.code, e.error.message))
            .unwrap_or_default();
        return Err(ApiFailure {
            status: status.as_u16(),
            code,
            message,
        }
        .into());
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
        assert_eq!(http_status(&err), Some(503), "状态码要随错误一起回来");
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("POST /api/v1/sessions HTTP/1.1"));
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let v: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(v["project_path"], "/Volumes/SSD/project/x");
        assert_eq!(v["agent"], "claude");
        assert_eq!(v["resume"], true);
    }

    #[test]
    fn rest_create_terminal_is_fresh_shell() {
        // 终端面板「+」：agent 固定 shell、不 resume、fresh:true 才会真开第二个
        let (port, req_rx) = one_shot_server(
            "HTTP/1.1 200 OK",
            r#"{"id":"s_t1","agent":"shell","project_path":"/Volumes/SSD/project"}"#,
        );
        let net = test_net(port);
        let s = futures::executor::block_on(
            net.create_terminal("/Volumes/SSD/project".into(), true),
        )
        .unwrap();
        assert_eq!(s.id, "s_t1");
        assert!(s.is_terminal());
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("POST /api/v1/sessions HTTP/1.1"));
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let v: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(v["project_path"], "/Volumes/SSD/project");
        assert_eq!(v["agent"], "shell");
        assert_eq!(v["resume"], false);
        assert_eq!(v["fresh"], true);
    }

    #[test]
    fn rest_input_options_key() {
        // composer 发送：POST input {"text":"1","enter":true}
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
    fn rest_answer_body_and_409_status() {
        // 对话框没吃下 → daemon 409；message 与状态码都要能到调用方手里
        let (port, req_rx) = one_shot_server(
            "HTTP/1.1 409 Conflict",
            r#"{"error":{"code":"conflict","message":"the dialog did not take the answer; finish it in the terminal"}}"#,
        );
        let net = test_net(port);
        let err = futures::executor::block_on(net.session_answer(
            "s_1",
            vec![
                AnswerItem {
                    selected: vec![0, 2],
                    other: None,
                },
                AnswerItem {
                    selected: vec![],
                    other: Some("Zed".into()),
                },
            ],
        ))
        .unwrap_err();
        assert_eq!(http_status(&err), Some(409), "{err}");
        assert!(err.to_string().contains("finish it in the terminal"), "{err}");
        let f = err.downcast_ref::<ApiFailure>().unwrap();
        assert_eq!(f.code, "conflict");
        let req = req_rx.recv().unwrap();
        assert!(
            req.starts_with("POST /api/v1/sessions/s_1/answer HTTP/1.1"),
            "req: {req}"
        );
        let body_start = req.find("\r\n\r\n").unwrap() + 4;
        let v: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
        assert_eq!(v["answers"][0]["selected"], serde_json::json!([0, 2]));
        assert!(v["answers"][0].get("other").is_none(), "other 为空时不发字段");
        assert_eq!(v["answers"][1]["selected"], serde_json::json!([]));
        assert_eq!(v["answers"][1]["other"], "Zed");
    }

    #[test]
    fn rest_inbox_paths() {
        // 收件箱 GET：路径进查询串要编码
        let (port, req_rx) = one_shot_server("HTTP/1.1 200 OK", r#"[{"id":"i1","text":"t"}]"#);
        let net = test_net(port);
        let items = futures::executor::block_on(net.inbox("/Volumes/SSD/project/x y")).unwrap();
        assert_eq!(items[0].id, "i1");
        let req = req_rx.recv().unwrap();
        assert!(
            req.starts_with("GET /api/v1/inbox?path=%2FVolumes%2FSSD%2Fproject%2Fx%20y HTTP/1.1"),
            "req: {req}"
        );
        // 删除条目：DELETE /inbox/:id
        let (port, req_rx) = one_shot_server("HTTP/1.1 200 OK", "");
        let net = test_net(port);
        futures::executor::block_on(net.inbox_delete("i1")).unwrap();
        let req = req_rx.recv().unwrap();
        assert!(req.starts_with("DELETE /api/v1/inbox/i1 HTTP/1.1"), "req: {req}");
    }

    #[test]
    fn plain_http_error_has_status_only() {
        // 非 JSON 错误体（v1 daemon 的 404）：Display 退回 "HTTP 404"，状态码照样可取
        let (port, _rx) = one_shot_server("HTTP/1.1 404 Not Found", "not found");
        let net = test_net(port);
        let err = futures::executor::block_on(net.health()).unwrap_err();
        assert_eq!(err.to_string(), "HTTP 404");
        assert_eq!(http_status(&err), Some(404));
    }

    #[test]
    fn no_endpoint_fails_gracefully() {
        let net = Net::new_quiet(None);
        let err = futures::executor::block_on(net.health()).unwrap_err();
        assert!(err.to_string().contains("未连接"));
    }
}
