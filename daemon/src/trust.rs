//! Auto-accept Claude Code's folder trust dialog.
//!
//! A fresh project's first screen is「Do you trust the files in this folder?」
//! with「Yes, proceed」highlighted. The user has already chosen the folder in
//! AAA, so asking again is pure friction; the daemon presses Enter for them.
//!
//! This is the one place the daemon still reads the screen to act — kept
//! deliberately narrow: both marker strings must be visible, only claude
//! sessions, at most [`MAX_PRESSES`] tries spaced [`RETRY_SECS`] apart. The
//! real record of the decision is written by claude itself into
//! `~/.claude.json` (the daemon never writes that file; see `feed`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::SharedApp;
use crate::pool::Session;

pub const PROMPT: &str = "Do you trust the files in this folder";
pub const ACCEPT: &str = "Yes, proceed";
pub const MAX_PRESSES: u8 = 3;
pub const RETRY_SECS: u64 = 2;

/// Is the trust dialog on screen? Both markers must show — the prompt alone
/// also appears in docs/help text claude might print later.
pub fn dialog_visible(screen: &vt100::Screen) -> bool {
    crate::screen::screen_contains(screen, PROMPT) && crate::screen::screen_contains(screen, ACCEPT)
}

/// Pure decision: press Enter now?
pub fn should_press(
    enabled: bool,
    agent: &str,
    alive: bool,
    visible: bool,
    presses: u8,
    since_last: Option<Duration>,
) -> bool {
    enabled
        && agent == "claude"
        && alive
        && visible
        && presses < MAX_PRESSES
        && since_last.is_none_or(|d| d >= Duration::from_secs(RETRY_SECS))
}

/// Called from the 1s tick for every session.
pub fn on_tick(app: &SharedApp, sess: &Arc<Session>) {
    let (agent, presses, last) = {
        let meta = sess.meta.lock().unwrap();
        (meta.agent.clone(), meta.trust_presses, meta.trust_pressed_inst)
    };
    if !app.cfg.auto_trust || agent != "claude" || presses >= MAX_PRESSES {
        return;
    }
    let alive = sess.live.lock().unwrap().is_some();
    let visible = {
        let guard = sess.parser.lock().unwrap();
        guard.as_ref().is_some_and(|p| dialog_visible(p.screen()))
    };
    if !should_press(true, &agent, alive, visible, presses, last.map(|t| t.elapsed())) {
        return;
    }
    if sess.write_input(b"\r").is_ok() {
        let mut meta = sess.meta.lock().unwrap();
        meta.trust_presses += 1;
        meta.trust_pressed_inst = Some(Instant::now());
        println!("[trust] {} accepted folder trust for {}", sess.id, meta.project_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen_of(text: &str) -> vt100::Parser {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(text.replace('\n', "\r\n").as_bytes());
        p
    }

    #[test]
    fn recognises_the_dialog_only_with_both_markers() {
        let dlg = screen_of(
            "╭──────────────────────────────╮\n│ Do you trust the files in this folder? │\n│ /Users/x/proj │\n│ ❯ 1. Yes, proceed │\n│   2. No, exit │\n╰──────────────────────────────╯",
        );
        assert!(dialog_visible(dlg.screen()));
        let doc = screen_of("Claude asked: Do you trust the files in this folder? — that is the trust dialog.");
        assert!(!dialog_visible(doc.screen()), "prompt text quoted in output is not a dialog");
        let composer = screen_of("> Yes, proceed with the refactor");
        assert!(!dialog_visible(composer.screen()));
    }

    #[test]
    fn press_gate() {
        let s = Duration::from_secs;
        assert!(should_press(true, "claude", true, true, 0, None));
        assert!(should_press(true, "claude", true, true, 1, Some(s(2))));
        assert!(!should_press(true, "claude", true, true, 1, Some(s(1))), "retry spacing");
        assert!(!should_press(true, "claude", true, true, MAX_PRESSES, Some(s(9))), "gives up");
        assert!(!should_press(false, "claude", true, true, 0, None), "config off");
        assert!(!should_press(true, "shell", true, true, 0, None));
        assert!(!should_press(true, "claude", false, true, 0, None));
        assert!(!should_press(true, "claude", true, false, 0, None));
    }
}
