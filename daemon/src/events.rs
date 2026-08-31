//! Event hub for the `/api/v1/events` WebSocket.

use tokio::sync::broadcast;

#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<String>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(512);
        EventHub { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    pub fn send(&self, v: &serde_json::Value) {
        let _ = self.tx.send(v.to_string());
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

    pub fn inbox_changed(&self, path: &str) {
        self.send(&serde_json::json!({"t": "inbox_changed", "path": path}));
    }

    pub fn session_stalled(&self, id: &str, quiet_s: u64) {
        self.send(&serde_json::json!({"t": "session_stalled", "id": id, "quiet_s": quiet_s}));
    }
}
