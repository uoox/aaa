//! Answering an AskUserQuestion form on the user's behalf.
//!
//! Clients send *what* was chosen (`POST /sessions/:id/answer`); this module
//! knows *how* Claude Code's dialog takes it. Keystroke protocol, measured
//! against Claude Code 2.1.258 (2026-09-02, pyte harness):
//!
//! * A form has a Review/Submit tab iff it has more than one question or any
//!   multi-select question. A single single-select question submits on the
//!   choice itself.
//! * Single-select: the option's digit. Advances to the next tab (or
//!   submits). Free text: digit of "Type something" (= options + 1), the
//!   text, Return — same advance.
//! * Multi-select: each chosen option's digit toggles it; the highlight stays
//!   on row 1. Free text: Down × options lands on "Type something", typing
//!   fills and auto-checks it — Return there would *un*check it, so never
//!   press it. Tab from text mode moves to the Submit/Next row (Return then
//!   advances); Tab from a plain row advances straight away.
//! * Review tab: Return on the highlighted "Submit answers".
//!
//! The driver types with a beat between commands (Ink processes each read as
//! one input event) and then watches the screen: while the Review question
//! is showing it presses Return; it reports success only once the dialog's
//! hint line is gone.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use crate::messages::QuestionSpec;
use crate::pool::Session;

#[derive(Clone, Debug, Deserialize, PartialEq, Default)]
pub struct Answer {
    /// 0-based indexes into the question's options
    #[serde(default)]
    pub selected: Vec<usize>,
    /// free text for the "Type something" slot
    #[serde(default)]
    pub other: Option<String>,
}

/// One PTY write followed by a pause (ms) before the next.
pub type Step = (Vec<u8>, u64);

const BEAT: u64 = 70;
const TEXT_MODE: u64 = 160;

fn key(s: &str, pause: u64) -> Step {
    (s.as_bytes().to_vec(), pause)
}

/// 权限对话框（Bash 授权 / ExitPlanMode 批准…，Ink 的单选列表，与 AskUserQuestion 同款）：
/// allow = 第 1 项的数字（Yes；数字键即选中，与表单一致）再补一个 Return 兜底——
/// 对话框已经没了的话 Return 只是在空输入框上回车，无害；deny = Esc（No / 打断，回到输入框）。
pub fn permission_steps(behavior: &str) -> Result<Vec<Step>, String> {
    match behavior {
        "allow" => Ok(vec![key("1", 250), key("\r", BEAT)]),
        "deny" => Ok(vec![key("\x1b", BEAT)]),
        other => Err(format!("behavior 只能是 allow / deny，不是 {other}")),
    }
}

/// Validate answers against the form. Returns a message for the client.
pub fn validate(spec: &QuestionSpec, answers: &[Answer]) -> Result<(), String> {
    if answers.len() != spec.questions.len() {
        return Err(format!(
            "expected {} answers, got {}",
            spec.questions.len(),
            answers.len()
        ));
    }
    for (i, (q, a)) in spec.questions.iter().zip(answers).enumerate() {
        let other = a.other.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if let Some(bad) = a.selected.iter().find(|&&k| k >= q.options.len()) {
            return Err(format!("question {}: option index {bad} out of range", i + 1));
        }
        if !q.multi_select && a.selected.len() + usize::from(other.is_some()) != 1 {
            return Err(format!("question {}: pick exactly one option or type an answer", i + 1));
        }
        if q.multi_select && a.selected.is_empty() && other.is_none() {
            return Err(format!("question {}: pick at least one option", i + 1));
        }
        if other.is_some_and(|t| t.contains('\n') || t.contains('\r') || t.contains('\x1b')) {
            return Err(format!("question {}: free text must be a single line", i + 1));
        }
    }
    Ok(())
}

/// The keystrokes that answer `spec` with `answers` (validated first).
pub fn plan(spec: &QuestionSpec, answers: &[Answer]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for (q, a) in spec.questions.iter().zip(answers) {
        let n = q.options.len();
        let other = a.other.as_deref().map(str::trim).filter(|t| !t.is_empty());
        if !q.multi_select {
            match (a.selected.first(), other) {
                (Some(&k), _) => steps.push(key(&(k + 1).to_string(), BEAT * 2)),
                (None, Some(text)) => {
                    steps.push(key(&(n + 1).to_string(), TEXT_MODE));
                    steps.push(key(text, TEXT_MODE));
                    steps.push(key("\r", BEAT * 2));
                }
                (None, None) => {}
            }
            continue;
        }
        for &k in &a.selected {
            steps.push(key(&(k + 1).to_string(), BEAT));
        }
        if let Some(text) = other {
            steps.push(("\x1b[B".repeat(n).into_bytes(), TEXT_MODE));
            steps.push(key(text, TEXT_MODE));
            steps.push(key("\t", BEAT * 2)); // leave the field: highlight → Submit/Next row
            steps.push(key("\r", BEAT * 2)); // advance
        } else {
            steps.push(key("\t", BEAT * 2)); // straight to the next tab
        }
    }
    steps
}

/// Does the form end on a Review tab the driver must confirm?
pub fn has_review_tab(spec: &QuestionSpec) -> bool {
    spec.questions.len() > 1 || spec.questions.iter().any(|q| q.multi_select)
}

pub const HINT: &str = "Enter to select";
pub const REVIEW: &str = "Ready to submit your answers?";

fn dialog_state(sess: &Session) -> (bool, bool) {
    let guard = sess.parser.lock().unwrap();
    match guard.as_ref() {
        Some(p) => (
            crate::screen::screen_contains(p.screen(), HINT),
            crate::screen::screen_contains(p.screen(), REVIEW),
        ),
        None => (false, false),
    }
}

/// Type the plan, confirm the Review tab if one shows up, and wait for the
/// dialog to leave the screen. `Err` = the dialog is still there (the user
/// should finish in the terminal).
pub async fn drive(sess: &Arc<Session>, steps: Vec<Step>) -> Result<(), String> {
    for (bytes, pause) in steps {
        sess.write_input(&bytes).map_err(|e| format!("pty write: {e}"))?;
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
    let mut confirmed = false;
    for _ in 0..12 {
        let (hint, review) = dialog_state(sess);
        if review && !confirmed {
            sess.write_input(b"\r").map_err(|e| format!("pty write: {e}"))?;
            confirmed = true;
        } else if !hint && !review {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("the dialog did not take the answer; finish it in the terminal".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{QOption, QuestionItem};

    fn q(header: &str, labels: &[&str], multi: bool) -> QuestionItem {
        QuestionItem {
            header: header.into(),
            question: format!("{header}?"),
            options: labels
                .iter()
                .map(|l| QOption { label: l.to_string(), description: String::new() })
                .collect(),
            multi_select: multi,
        }
    }
    fn spec(items: Vec<QuestionItem>) -> QuestionSpec {
        QuestionSpec { questions: items }
    }
    fn keys(steps: &[Step]) -> Vec<String> {
        steps.iter().map(|(b, _)| String::from_utf8(b.clone()).unwrap()).collect()
    }
    fn sel(v: &[usize]) -> Answer {
        Answer { selected: v.to_vec(), other: None }
    }
    fn other(t: &str) -> Answer {
        Answer { selected: vec![], other: Some(t.into()) }
    }

    #[test]
    fn single_question_single_select_is_one_digit() {
        let s = spec(vec![q("Pet", &["Cat", "Dog"], false)]);
        assert_eq!(keys(&plan(&s, &[sel(&[1])])), vec!["2"]);
        assert!(!has_review_tab(&s), "submits on the digit itself");
    }

    #[test]
    fn two_questions_single_then_multi() {
        // measured: 2 → auto-advance; 1,3 toggle; Tab → Review; Return
        let s = spec(vec![q("Color", &["Red", "Green", "Blue"], false), q("Fruits", &["Apple", "Banana", "Cherry"], true)]);
        assert_eq!(keys(&plan(&s, &[sel(&[1]), sel(&[0, 2])])), vec!["2", "1", "3", "\t"]);
        assert!(has_review_tab(&s));
    }

    #[test]
    fn free_text_in_single_select() {
        // measured: digit of "Type something" (options+1), text, Return
        let s = spec(vec![q("Name", &["Alice", "Bob"], false)]);
        assert_eq!(keys(&plan(&s, &[other("Zed")])), vec!["3", "Zed", "\r"]);
    }

    #[test]
    fn free_text_in_multi_select_never_presses_return_on_the_field() {
        // measured: Return on the filled row un-checks it; Tab leaves the
        // field onto the Submit row, Return there advances
        let s = spec(vec![q("Cities", &["Paris", "Rome", "Oslo"], true)]);
        let k = keys(&plan(&s, &[Answer { selected: vec![1], other: Some("Zurich".into()) }]));
        assert_eq!(k, vec!["2", "\x1b[B\x1b[B\x1b[B", "Zurich", "\t", "\r"]);
        assert!(has_review_tab(&s), "single multi-select form still ends on Review");
    }

    #[test]
    fn validation() {
        let s = spec(vec![q("Pet", &["Cat", "Dog"], false), q("Tools", &["Hammer", "Saw"], true)]);
        assert!(validate(&s, &[sel(&[0])]).is_err(), "answer count");
        assert!(validate(&s, &[sel(&[5]), sel(&[0])]).is_err(), "index range");
        assert!(validate(&s, &[sel(&[0, 1]), sel(&[0])]).is_err(), "single-select takes one");
        assert!(validate(&s, &[Answer { selected: vec![0], other: Some("x".into()) }, sel(&[0])]).is_err());
        assert!(validate(&s, &[sel(&[0]), sel(&[])]).is_err(), "multi needs something");
        assert!(validate(&s, &[sel(&[0]), other("drill\nsaw")]).is_err(), "single line");
        assert!(validate(&s, &[sel(&[0]), other("  ")]).is_err(), "blank other is nothing");
        assert!(validate(&s, &[sel(&[1]), sel(&[0, 1])]).is_ok());
        assert!(validate(&s, &[other("fish"), other("drill")]).is_ok());
    }
}
