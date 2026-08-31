//! ntfy push (optional, config `[ntfy]`).
//!
//! Uses the JSON publish endpoint (`POST <url>` with `{topic,title,message,
//! priority}`) so UTF-8 titles survive; failures are silent.

use crate::config::NtfyConfig;

/// ntfy priority levels (1 min .. 5 max).
pub const PRIO_DEFAULT: u8 = 3;
pub const PRIO_HIGH: u8 = 4;

pub fn push_blocking(cfg: &NtfyConfig, title: &str, message: &str, priority: u8) {
    let body = serde_json::json!({
        "topic": cfg.topic,
        "title": title,
        "message": message,
        "priority": priority,
    })
    .to_string();
    let _ = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .post(&cfg.url)
        .set("Content-Type", "application/json")
        .send_string(&body);
}
