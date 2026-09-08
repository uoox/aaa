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
pub const REJECT_NEW: &str = "No, exit";
/// 单键计数：Down 一次 + Enter 一次就够；多给几次是留给「Down 丢了」的重试
pub const MAX_PRESSES: u8 = 8;
pub const RETRY_SECS: u64 = 1;

const DOWN: &[u8] = b"\x1b[B";
const ENTER: &[u8] = b"\r";

/// The single key to send next for the dialog on screen, or None when no
/// dialog. One key per tick, and **Enter only once the screen shows「Yes」
/// highlighted**: sending Down and Enter back to back lost the Down often
/// enough (Ink not yet in raw mode) that Enter landed on「No, exit」and
/// Claude quit — the session then sat「exited」with the dialog still drawn
/// (2026-09-07 修).
pub fn next_key(screen: &vt100::Screen) -> Option<&'static [u8]> {
    let (_, cols) = screen.size();
    let rows: Vec<String> = screen.rows(0, cols).collect();
    let has = |needle: &str| rows.iter().any(|r| r.contains(needle));
    if has(PROMPT_OLD) && has(ACCEPT_OLD) {
        return Some(ENTER);
    }
    // 新对话框：提示行可能滚出屏幕（行数少的手机终端），认「Yes, I trust」+「No, exit」两行选项
    if has(ACCEPT_NEW) && has(REJECT_NEW) {
        let highlighted = |needle: &str| rows.iter().any(|r| r.contains('❯') && r.contains(needle));
        return if highlighted(ACCEPT_NEW) {
            Some(ENTER)
        } else if highlighted(REJECT_NEW) {
            Some(DOWN)
        } else {
            None // 高亮在别处 / 还没画出来：等下一 tick
        };
    }
    None
}

/// Is the trust dialog on screen? Both option lines must show — the prompt
/// alone also appears in docs/help text claude might print later.
pub fn dialog_visible(screen: &vt100::Screen) -> bool {
    let (_, cols) = screen.size();
    let rows: Vec<String> = screen.rows(0, cols).collect();
    let has = |needle: &str| rows.iter().any(|r| r.contains(needle));
    (has(PROMPT_OLD) && has(ACCEPT_OLD)) || (has(ACCEPT_NEW) && has(REJECT_NEW))
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
    let key = {
        let guard = sess.parser.lock().unwrap();
        guard.as_ref().and_then(|p| next_key(p.screen()))
    };
    if !should_press(true, &agent, alive, key.is_some(), presses, last.map(|t| t.elapsed())) {
        return;
    }
    let Some(key) = key else { return };
    // 一 tick 一个键：Down 之后下一 tick 屏幕上高亮到了「Yes」才按 Enter
    let ok = sess.write_input(key).is_ok();
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
        assert_eq!(next_key(dlg.screen()), Some(&b"\r"[..]));
        let doc = screen_of("Claude asked: Do you trust the files in this folder? — that is the trust dialog.");
        assert!(!dialog_visible(doc.screen()), "prompt text quoted in output is not a dialog");
        let composer = screen_of("> Yes, proceed with the refactor");
        assert!(!dialog_visible(composer.screen()));
    }

    #[test]
    fn new_dialog_sends_down_first_and_enter_only_when_yes_is_highlighted() {
        // 2.1.259 的实际画面（daemon /screen 抓的）
        let dlg = screen_of(
            " Accessing workspace:\n\n /tmp/x/demo\n\n Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source\n project, or work from your team). If not, take a moment to review what's in this folder first.\n\n Claude Code'll be able to read, edit, and execute files here.\n\n Security guide\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel",
        );
        assert!(dialog_visible(dlg.screen()));
        assert_eq!(next_key(dlg.screen()), Some(DOWN), "高亮在 No：只按 Down，绝不带 Enter");
        // 高亮到了 Yes：按 Enter
        let yes = screen_of(" Quick safety check: Is this a project you created or one you trust?\n\n   No, exit\n ❯ Yes, I trust this folder\n");
        assert_eq!(next_key(yes.screen()), Some(ENTER));
        // 提示行滚出了屏幕（手机终端行数少）：光凭两行选项也认
        let scrolled = screen_of(" Security guide\n\n ❯ No, exit\n   Yes, I trust this folder\n\n Enter to confirm · Esc to cancel");
        assert!(dialog_visible(scrolled.screen()));
        assert_eq!(next_key(scrolled.screen()), Some(DOWN));
        // 高亮不在两项上（正在重绘）：这一 tick 什么都不按
        let mid = screen_of("   No, exit\n   Yes, I trust this folder\n");
        assert!(dialog_visible(mid.screen()));
        assert_eq!(next_key(mid.screen()), None);
        // 只有提示没有选项行（比如被引用在输出里）不算
        let doc = screen_of("It said: Quick safety check: Is this a project you created? — then I answered.");
        assert!(!dialog_visible(doc.screen()));
    }

    #[test]
    fn press_gate() {
        let s = Duration::from_secs;
        assert!(should_press(true, "claude", true, true, 0, None));
        assert!(should_press(true, "claude", true, true, 1, Some(s(2))));
        assert!(!should_press(true, "claude", true, true, 1, Some(Duration::from_millis(300))), "retry spacing");
        assert!(!should_press(true, "claude", true, true, MAX_PRESSES, Some(s(9))), "gives up");
        assert!(!should_press(false, "claude", true, true, 0, None), "config off");
        assert!(!should_press(true, "shell", true, true, 0, None));
        assert!(!should_press(true, "claude", false, true, 0, None));
        assert!(!should_press(true, "claude", true, false, 0, None));
    }
}
