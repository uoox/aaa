//! Auto-accept Claude Code's folder trust dialog.
//!
//! A fresh project's first screen is the trust dialog. The user has already
//! chosen the folder in AAA, so asking again is pure friction; the daemon
//! answers it for them. Two generations of the dialog exist:
//!
//! - old (≤ 2.1.2xx):「Do you trust the files in this folder?」with
//!  「❯ 1. Yes, proceed」highlighted → Enter.
//! - new (2.1.259+):「Quick safety check: Is this a project you created or one
//!   you trust?」with **「❯ No, exit」highlighted first** and「Yes, I trust this
//!   folder」below → Down, then Enter. Pressing plain Enter here exits Claude,
//!   which is exactly what an unaware daemon did.
//!
//! This is the one place the daemon still reads the screen to act — kept
//! deliberately narrow: the prompt and an option line must both be visible,
//! only claude sessions, at most [`MAX_PRESSES`] tries spaced [`RETRY_SECS`]
//! apart. The real record of the decision is written by claude itself into
//! `~/.claude.json` (the daemon never writes that file; see `feed`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::api::SharedApp;
use crate::pool::Session;

pub const PROMPT_OLD: &str = "Do you trust the files in this folder";
pub const ACCEPT_OLD: &str = "Yes, proceed";
pub const PROMPT_NEW: &str = "Quick safety check";
pub const ACCEPT_NEW: &str = "Yes, I trust this folder";
pub const MAX_PRESSES: u8 = 3;
pub const RETRY_SECS: u64 = 2;

const DOWN: &[u8] = b"\x1b[B";
const ENTER: &[u8] = b"\r";

/// Keys that accept the dialog currently on screen, or None when no dialog.
/// Old dialog / new dialog with「Yes」already highlighted → Enter. New dialog
/// with「No, exit」highlighted → Down + Enter.
pub fn accept_keys(screen: &vt100::Screen) -> Option<Vec<u8>> {
    let (_, cols) = screen.size();
    let rows: Vec<String> = screen.rows(0, cols).collect();
    let has = |needle: &str| rows.iter().any(|r| r.contains(needle));
    if has(PROMPT_OLD) && has(ACCEPT_OLD) {
        return Some(ENTER.to_vec());
    }
    if has(PROMPT_NEW) && has(ACCEPT_NEW) {
        let highlighted_yes = rows
            .iter()
            .any(|r| r.contains('❯') && r.contains(ACCEPT_NEW));
        return Some(if highlighted_yes {
            ENTER.to_vec()
        } else {
            [DOWN, ENTER].concat()
        });
    }
    None
}

/// Is the trust dialog on screen? Both a prompt and its option line must show —
/// the prompt alone also appears in docs/help text claude might print later.
pub fn dialog_visible(screen: &vt100::Screen) -> bool {
    accept_keys(screen).is_some()
}

/// Pure decision: act now?
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
    let keys = {
        let guard = sess.parser.lock().unwrap();
        guard.as_ref().and_then(|p| accept_keys(p.screen()))
    };
    if !should_press(true, &agent, alive, keys.is_some(), presses, last.map(|t| t.elapsed())) {
        return;
    }
    let Some(keys) = keys else { return };
    // Down 与 Enter 之间留一拍：Ink 的列表在同一帧里吃两个键会只认第一个
    let ok = if keys.len() > ENTER.len() {
        let (mv, enter) = keys.split_at(keys.len() - ENTER.len());
        sess.write_input(mv).is_ok() && {
            std::thread::sleep(Duration::from_millis(120));
            sess.write_input(enter).is_ok()
        }
    } else {
        sess.write_input(&keys).is_ok()
    };
    if ok {
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
        let mut p = vt100::Parser::new(24, 120, 0);
        p.process(text.replace('\n', "\r\n").as_bytes());
        p
    }

    #[test]
    fn recognises_the_old_dialog_only_with_both_markers() {
        let dlg = screen_of(
            "╭──────────────────────────────╮\n│ Do you trust the files in this folder? │\n│ /Users/x/proj │\n│ ❯ 1. Yes, proceed │\n│   2. No, exit │\n╰──────────────────────────────╯",
        );
        assert!(dialog_visible(dlg.screen()));
        assert_eq!(accept_keys(dlg.screen()), Some(b"\r".to_vec()));
        let doc = screen_of("Claude asked: Do you trust the files in this folder? — that is the trust dialog.");
        assert!(!dialog_visible(doc.screen()), "prompt text quoted in output is not a dialog");
        let composer = screen_of("> Yes, proceed with the refactor");
        assert!(!dialog_visible(composer.screen()));
    }

    #[test]
    fn new_dialog_with_no_highlighted_needs_down_then_enter() {
        // 2.1.259 的实际画面（daemon /screen 抓的）
        let dlg = screen_of(
            " Accessing workspace:\n\n /tmp/x/demo\n\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source\n project, or work from your team). If not, take a moment to review what's in this folder first.\n\n Claude Code'll be able to read, edit, and execute files here.\n\n Security guide\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel",
        );
        assert!(dialog_visible(dlg.screen()));
        assert_eq!(accept_keys(dlg.screen()), Some(b"\x1b[B\r".to_vec()));
        // 用户（或上一次 Down）已经把光标移到 Yes：只按 Enter
        let yes = screen_of(" Quick safety check: Is this a project you created or one you trust?\n\n   No, exit\n ❯ Yes, I trust this folder\n");
        assert_eq!(accept_keys(yes.screen()), Some(b"\r".to_vec()));
        // 只有提示没有选项行（比如被引用在输出里）不算
        let doc = screen_of("It said: Quick safety check: Is this a project you created? — then I answered.");
        assert!(!dialog_visible(doc.screen()));
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
