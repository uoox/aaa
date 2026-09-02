//! Inbox auto-feed on a session's first `waiting`.
//!
//! The daemon no longer reads the screen to decide whether typed text would
//! land in a composer or be swallowed by a dialog. Two structured signals
//! stand in for that guess, both claude-only:
//!
//! * the transcript shows an AskUserQuestion with no answer (`asking`) — a
//!   dialog is up, feeding would answer it with whatever is highlighted;
//! * `~/.claude.json` does not yet mark the folder trusted — the first screen
//!   of a fresh project is the trust dialog, which discards free text.
//!
//! Either way the entries stay queued and the next `waiting` retries.

use std::sync::Arc;

use crate::api::SharedApp;
use crate::pool::{Session, State};

/// Has Claude Code recorded the user's trust decision for `project_path`?
/// Read-only: claude rewrites this file constantly, so the daemon never
/// writes it (a read-modify-write would race and could drop its updates).
pub fn claude_trusts(home: &std::path::Path, project_path: &str) -> bool {
    let Ok(body) = std::fs::read_to_string(home.join(".claude.json")) else { return false };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return false };
    trusted_in(&v, project_path)
}

/// Pure lookup over the parsed `~/.claude.json`.
pub fn trusted_in(claude_json: &serde_json::Value, project_path: &str) -> bool {
    claude_json
        .get("projects")
        .and_then(|p| p.get(project_path))
        .and_then(|p| p.get("hasTrustDialogAccepted"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
}

/// Should the queued entries be typed into this session right now?
pub fn can_feed(
    state: State,
    alive: bool,
    feed_inbox: bool,
    already_fed: bool,
    agent: &str,
    asking: bool,
    trusted: bool,
) -> bool {
    state == State::Waiting
        && alive
        && feed_inbox
        && !already_fed
        && !(agent == "claude" && (asking || !trusted))
}

pub fn on_waiting(app: &SharedApp, sess: Arc<Session>) {
    let (project_path, agent, state, feed_inbox, fed, asking) = {
        let meta = sess.meta.lock().unwrap();
        (
            meta.project_path.clone(),
            meta.agent.clone(),
            meta.state,
            meta.feed_inbox,
            meta.inbox_fed,
            meta.asking,
        )
    };
    let alive = sess.live.lock().unwrap().is_some();
    if !feed_inbox || fed || !alive {
        return;
    }
    // cheap checks first; the inbox lookup and the claude.json read only when
    // there is actually something queued
    let has_entries = !app.inbox.lock().unwrap().list(&project_path).is_empty();
    if !has_entries {
        return;
    }
    let trusted = agent != "claude" || claude_trusts(&app.paths.home, &project_path);
    if !can_feed(state, alive, feed_inbox, fed, &agent, asking, trusted) {
        return;
    }
    let entries = app.inbox.lock().unwrap().take_all(&project_path);
    if entries.is_empty() {
        return;
    }
    let mut text = crate::inbox::compose_feed(&entries);
    text.push('\r');
    if sess.write_input(text.as_bytes()).is_ok() {
        sess.meta.lock().unwrap().inbox_fed = true;
        app.hub.inbox_changed(&project_path);
        sess.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn feed_gate() {
        let ok = |agent: &str, asking: bool, trusted: bool| {
            can_feed(State::Waiting, true, true, false, agent, asking, trusted)
        };
        assert!(ok("claude", false, true));
        assert!(!ok("claude", true, true), "dialog up: feeding would answer it");
        assert!(!ok("claude", false, false), "trust dialog is the first screen");
        assert!(ok("codex", false, false), "no structured signal for other agents: feed on waiting");
        assert!(ok("shell", false, false));
        assert!(!can_feed(State::Running, true, true, false, "shell", false, true));
        assert!(!can_feed(State::Waiting, false, true, false, "shell", false, true), "dead pty");
        assert!(!can_feed(State::Waiting, true, false, false, "shell", false, true), "feed_inbox:false");
        assert!(!can_feed(State::Waiting, true, true, true, "shell", false, true), "once per session");
    }

    #[test]
    fn trust_lookup() {
        let v = json!({"projects": {"/p/a": {"hasTrustDialogAccepted": true}, "/p/b": {"allowedTools": []}}});
        assert!(trusted_in(&v, "/p/a"));
        assert!(!trusted_in(&v, "/p/b"));
        assert!(!trusted_in(&v, "/p/none"));
        assert!(!trusted_in(&json!({}), "/p/a"));
    }
}
