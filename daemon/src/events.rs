//! Event hub for the `/api/v1/events` WebSocket.
//!
//! Frames are serialized exactly once per event: the channel carries
//! `Utf8Bytes` (a refcounted buffer that `Message::Text` takes as-is), so a
//! broadcast to N subscribers clones N refcounts, not N strings.

use axum::extract::ws::Utf8Bytes;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Utf8Bytes>,
}

impl EventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        EventHub { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Utf8Bytes> {
        self.tx.subscribe()
    }

    pub fn send(&self, v: &serde_json::Value) {
        let _ = self.tx.send(Utf8Bytes::from(v.to_string()));
    }

    pub fn session(&self, session_json: serde_json::Value) {
        self.send(&serde_json::json!({"t": "session", "session": session_json}));
    }

    pub fn session_removed(&self, id: &str) {
        self.send(&serde_json::json!({"t": "session_removed", "id": id}));
    }

    pub fn projects_changed(&self) {
        self.send(&serde_json::json!({"t": "projects_changed"}));
    }

    pub fn health(&self, ssd_mounted: bool) {
        self.send(&serde_json::json!({"t": "health", "ssd_mounted": ssd_mounted}));
    }

    // ---- v1.1 frames (older clients tolerate unknown frame types) ----

    pub fn messages_changed(&self, id: &str, last_seq: u64) {
        self.send(&serde_json::json!({"t": "messages_changed", "id": id, "last_seq": last_seq}));
    }

    /// v1.3 plan 配额（claude 那一份），新值就广播。
    /// **这一帧永远只说 claude**：v1.38 之前的客户端把每一个 `usage` 帧都当成账号
    /// 配额收下，别的 agent 混进来就会把 Claude 那一栏的数字改错。
    pub fn usage(&self, plan: &serde_json::Value) {
        self.send(&serde_json::json!({"t": "usage", "agent": "claude", "plan": plan}));
    }

    /// v1.39 别的 agent 的配额（当前只有 `agy`）。**另起一个帧名**而不是给 `usage`
    /// 加字段：老客户端认得 `usage`、不认得这个，未知帧它们本来就丢掉——一升 daemon
    /// 就把 Antigravity 的数字画到 Claude 头上，是这里唯一要防的事。
    pub fn agent_usage(&self, agent: &str, plan: &serde_json::Value) {
        self.send(&serde_json::json!({"t": "agent_usage", "agent": agent, "plan": plan}));
    }

    pub fn inbox_changed(&self, path: &str) {
        self.send(&serde_json::json!({"t": "inbox_changed", "path": path}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_serialization_shared_by_all_subscribers() {
        let hub = EventHub::new();
        let mut rx1 = hub.subscribe();
        let mut rx2 = hub.subscribe();
        hub.session(serde_json::json!({"id": "s_x"}));
        let a = rx1.try_recv().unwrap();
        let b = rx2.try_recv().unwrap();
        assert_eq!(a.as_str(), b.as_str());
        // same underlying allocation: the JSON was serialized exactly once
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
    }
}
