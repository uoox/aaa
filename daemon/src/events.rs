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

    /// v1.3 plan 配额（statusLine 的 rate_limits），任一会话转来新值就广播
    pub fn usage(&self, plan: &serde_json::Value) {
        self.send(&serde_json::json!({"t": "usage", "plan": plan}));
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
