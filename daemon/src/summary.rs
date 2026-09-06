//! Per-turn summaries: after every `Stop` (one Claude reply finished) ask
//! haiku for a one-line "what this turn did", and keep the last few on the
//! session (`summaries`, newest last) for the clients' detail views.
//!
//! Same machinery as the namer (`claude -p --model haiku`, gated by the
//! `namer` config flag). Input is the turn itself, read from the message
//! store: the last user prompt, the tools it ran, and the final reply.

use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::api::SharedApp;
use crate::messages::{Msg, MsgStore};
use crate::pool::Session;

/// One finished turn, as the clients see it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnSummary {
    pub ts: String,
    pub text: String,
}

/// How many turns a session remembers (session JSON carries all of them).
pub const KEEP: usize = 40;

const PROMPT_PREFIX: &str = "下面是一轮对话：用户的要求、助手用过的工具、助手最后的回复。用一句话（不超过 50 个字）说清这一轮做了什么、结果如何，使用对话的主要语言，直接输出这句话本身，不要引号、不要前缀、不要换行：\n\n";

fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// The last turn as text for the model: everything after the newest user
/// prompt. `None` when the store has no prompt yet (nothing to summarise).
pub fn turn_excerpt(store: &MsgStore) -> Option<String> {
    let start = store
        .msgs
        .iter()
        .rposition(|m| m.role == "user" && (m.kind == "text" || m.kind == "answer"))?;
    let turn: Vec<&Msg> = store.msgs.iter().skip(start).collect();
    let mut out = String::new();
    let mut tools = 0usize;
    let mut reply = String::new();
    for m in &turn {
        match (m.role.as_str(), m.kind.as_str()) {
            ("user", "text") | ("user", "answer") => {
                out.push_str("用户: ");
                out.push_str(&take_chars(&m.text, 1200));
                out.push('\n');
            }
            ("assistant", "tool_use") if tools < 24 => {
                if let Some(t) = &m.tool {
                    out.push_str(&format!("工具: {} {}\n", t.name, take_chars(&t.summary, 120)));
                    tools += 1;
                }
            }
            ("assistant", "question") => {
                out.push_str("助手提问: ");
                out.push_str(&take_chars(&m.text, 300));
                out.push('\n');
            }
            ("assistant", "text") => {
                // the last text block is the reply that ended the turn
                reply = m.text.clone();
            }
            _ => {}
        }
    }
    if !reply.is_empty() {
        out.push_str("助手回复: ");
        out.push_str(&take_chars(&reply, 2500));
        out.push('\n');
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Model output → one clean line, or None when unusable.
pub fn clean(out: &str) -> Option<String> {
    let line = out
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .trim_matches(|c| matches!(c, '"' | '“' | '”'))
        .to_string();
    let n = line.chars().count();
    if n == 0 || n > 120 {
        return None;
    }
    Some(line)
}

/// Append one summary, keeping the newest [`KEEP`].
pub fn push(list: &mut Vec<TurnSummary>, text: String) {
    list.push(TurnSummary { ts: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true), text });
    if list.len() > KEEP {
        let drop = list.len() - KEEP;
        list.drain(..drop);
    }
}

/// `Stop` just arrived for a claude session: summarise the turn. Blocking
/// (runs haiku for up to a minute) — call from `spawn_blocking`, never await
/// it in the hook handler. One at a time per session; a `Stop` that lands
/// while the previous summary is still running is skipped (the next one
/// covers the newest turn anyway).
pub fn on_turn_done(app: &SharedApp, sess: Arc<Session>) {
    if !app.cfg.namer {
        return;
    }
    if sess.summarizing.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let done = || sess.summarizing.store(false, std::sync::atomic::Ordering::Release);
    // the transcript is flushed slightly after the hook fires: give it a
    // moment, then read straight from the file so the store is current
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let excerpt = {
        let mut store = sess.msgs.lock().unwrap();
        crate::messages::poll_file(&mut store);
        turn_excerpt(&store)
    };
    let Some(excerpt) = excerpt else {
        done();
        return;
    };
    let prompt = format!("{PROMPT_PREFIX}{}", take_chars(&excerpt, 6000));
    let out = crate::agents::which("claude", &app.paths.home)
        .and_then(|exe| crate::namer::run_haiku(&exe, &prompt));
    let Some(text) = out.as_deref().and_then(clean) else {
        done();
        return;
    };
    {
        let mut meta = sess.meta.lock().unwrap();
        push(&mut meta.summaries, text);
    }
    sess.mark_dirty();
    if sess.state() == crate::pool::State::Exited {
        sess.persist(&app.pool.ctx);
    }
    done();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::ToolInfo;

    fn msg(seq: u64, role: &str, kind: &str, text: &str, tool: Option<(&str, &str)>) -> Msg {
        Msg {
            seq,
            ts: "t".into(),
            role: role.into(),
            kind: kind.into(),
            text: text.into(),
            tool: tool.map(|(n, s)| ToolInfo { name: n.into(), summary: s.into(), status: "ok".into() }),
            question: None,
        }
    }

    #[test]
    fn excerpt_covers_only_the_last_turn() {
        let mut store = MsgStore::for_agent("claude");
        store.msgs.push_back(msg(1, "user", "text", "第一轮", None));
        store.msgs.push_back(msg(2, "assistant", "text", "第一轮回复", None));
        store.msgs.push_back(msg(3, "user", "text", "修登录 bug", None));
        store.msgs.push_back(msg(4, "assistant", "thinking", "想一想", None));
        store.msgs.push_back(msg(5, "assistant", "tool_use", "", Some(("Bash", "cargo test"))));
        store.msgs.push_back(msg(6, "tool", "tool_result", "ok", None));
        store.msgs.push_back(msg(7, "assistant", "text", "中途说明", None));
        store.msgs.push_back(msg(8, "assistant", "text", "修好了，测试全过", None));
        let e = turn_excerpt(&store).unwrap();
        assert!(e.contains("用户: 修登录 bug"));
        assert!(e.contains("工具: Bash cargo test"));
        assert!(e.contains("助手回复: 修好了，测试全过"));
        assert!(!e.contains("第一轮"), "上一轮不进摘要");
        assert!(!e.contains("中途说明"), "只取最后一段回复");
        assert!(!e.contains("想一想"), "思考不进摘要");
    }

    #[test]
    fn excerpt_needs_a_prompt() {
        let mut store = MsgStore::for_agent("claude");
        assert!(turn_excerpt(&store).is_none());
        store.msgs.push_back(msg(1, "assistant", "text", "无人问我", None));
        assert!(turn_excerpt(&store).is_none());
        // 表单作答也算一轮的起点
        store.msgs.push_back(msg(2, "user", "answer", "选 B", None));
        store.msgs.push_back(msg(3, "assistant", "text", "好，按 B 做", None));
        assert!(turn_excerpt(&store).unwrap().contains("用户: 选 B"));
    }

    #[test]
    fn clean_and_cap() {
        assert_eq!(clean("\n \"修好了登录页。\"\n第二行").as_deref(), Some("修好了登录页。"));
        assert!(clean("   ").is_none());
        assert!(clean(&"很长".repeat(100)).is_none());
        let mut v = Vec::new();
        for i in 0..(KEEP + 5) {
            push(&mut v, format!("t{i}"));
        }
        assert_eq!(v.len(), KEEP);
        assert_eq!(v[0].text, "t5", "最老的先丢");
        assert_eq!(v.last().unwrap().text, format!("t{}", KEEP + 4));
    }
}
