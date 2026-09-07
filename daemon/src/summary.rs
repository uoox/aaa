//! Conversation checklist: after every `Stop` (one Claude reply finished)
//! ask haiku to rewrite the session's running to-do list — what has been
//! done (`- [x]`) and what is still open (`- [ ]`), README-style — from the
//! previous list plus the turn that just ended. One markdown block per
//! session (`summary`), replaced each turn, persisted.
//!
//! Same machinery as the namer (`claude -p --model haiku`, gated by the
//! `namer` config flag). The first list of a session (or a resumed one
//! with no list yet) is built from the whole conversation so far.

use std::sync::Arc;

use crate::api::SharedApp;
use crate::messages::{Msg, MsgStore};
use crate::pool::Session;

/// Checklist size cap (lines).
pub const MAX_ITEMS: usize = 16;

const PROMPT_PREFIX: &str = "你在维护一个 Claude Code 对话的进度清单，形式像 GitHub README 里的 todo list。\
下面先给出目前的清单（可能为空），再给出刚结束的这一轮（用户的要求、用过的工具、最后的回复）。\
请输出**更新后的完整清单**：已经做完的写 `- [x] …`，用户提出但还没做、做到一半、或助手说稍后再做的写 `- [ ] …`。\
合并重复项，去掉已经不相关的项，每项不超过 30 个字，总共不超过 12 项，已做的在前。\
使用对话的主要语言。只输出清单本身，每行一项，不要标题、不要解释、不要空行。\n\n";

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
    excerpt_from(store.msgs.iter().skip(start))
}

/// The whole conversation so far, compressed: every user prompt plus the
/// reply that ended each turn (tools skipped). For a session's first list.
pub fn conversation_excerpt(store: &MsgStore) -> Option<String> {
    let mut out = String::new();
    let mut reply = String::new();
    let flush = |out: &mut String, reply: &mut String| {
        if !reply.is_empty() {
            out.push_str("助手回复: ");
            out.push_str(&take_chars(reply, 600));
            out.push('\n');
            reply.clear();
        }
    };
    for m in &store.msgs {
        match (m.role.as_str(), m.kind.as_str()) {
            ("user", "text") | ("user", "answer") => {
                flush(&mut out, &mut reply);
                out.push_str("用户: ");
                out.push_str(&take_chars(&m.text, 500));
                out.push('\n');
            }
            ("assistant", "text") => reply = m.text.clone(),
            _ => {}
        }
    }
    flush(&mut out, &mut reply);
    if out.trim().is_empty() {
        return None;
    }
    // keep the tail: the newest turns matter most
    let chars: Vec<char> = out.chars().collect();
    let keep = 9000;
    Some(if chars.len() > keep { chars[chars.len() - keep..].iter().collect() } else { out })
}

fn excerpt_from<'a>(msgs: impl Iterator<Item = &'a Msg>) -> Option<String> {
    let turn: Vec<&Msg> = msgs.collect();
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

/// Model output → a normalised checklist (`- [x] …` / `- [ ] …` lines, done
/// first), or None when nothing usable came back. Tolerates `*` bullets,
/// `[X]`, and stray prose lines (dropped).
pub fn clean(out: &str) -> Option<String> {
    let mut done = Vec::new();
    let mut open = Vec::new();
    for raw in out.lines() {
        let l = raw.trim().trim_start_matches(['-', '*', '•']).trim_start();
        let (checked, rest) = if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
            (true, r)
        } else if let Some(r) = l.strip_prefix("[ ]").or_else(|| l.strip_prefix("[]")) {
            (false, r)
        } else {
            continue;
        };
        let text: String = rest.trim().chars().take(60).collect();
        if text.is_empty() {
            continue;
        }
        if checked { done.push(text) } else { open.push(text) }
    }
    if done.is_empty() && open.is_empty() {
        return None;
    }
    let lines: Vec<String> = done
        .into_iter()
        .map(|t| format!("- [x] {t}"))
        .chain(open.into_iter().map(|t| format!("- [ ] {t}")))
        .take(MAX_ITEMS)
        .collect();
    Some(lines.join("\n"))
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
    let previous = sess.meta.lock().unwrap().summary.clone();
    let excerpt = {
        let mut store = sess.msgs.lock().unwrap();
        crate::messages::poll_file(&mut store);
        // 第一份清单（新会话，或 resume 进来还没有清单）看整段对话；之后只看这一轮
        if previous.is_empty() { conversation_excerpt(&store) } else { turn_excerpt(&store) }
    };
    let Some(excerpt) = excerpt else {
        done();
        return;
    };
    let prompt = format!(
        "{PROMPT_PREFIX}目前的清单：\n{}\n\n刚结束的这一轮：\n{}",
        if previous.is_empty() { "（空）".to_string() } else { previous },
        take_chars(&excerpt, 9000)
    );
    let out = crate::agents::which("claude", &app.paths.home)
        .and_then(|exe| crate::namer::run_haiku(&exe, &prompt));
    let Some(text) = out.as_deref().and_then(clean) else {
        done();
        return;
    };
    {
        let mut meta = sess.meta.lock().unwrap();
        // 看板上手工勾过的项按用户的来，haiku 重写不许翻回去
        let overrides = meta.checklist_overrides.clone();
        let mut text = text;
        for (item, done) in &overrides {
            text = set_item(&text, item, *done);
        }
        meta.summary = text;
    }
    sess.mark_dirty();
    if sess.state() == crate::pool::State::Exited {
        sess.persist(&app.pool.ctx);
    }
    done();
}

/// 把清单里文字等于 `item` 的那一行改成 done / 未 done；没有这一项就原样返回
pub fn set_item(summary: &str, item: &str, done: bool) -> String {
    summary
        .lines()
        .map(|raw| {
            let l = raw.trim().trim_start_matches(['-', '*']).trim_start();
            let rest = l
                .strip_prefix("[x]")
                .or_else(|| l.strip_prefix("[X]"))
                .or_else(|| l.strip_prefix("[ ]"));
            match rest {
                Some(r) if r.trim() == item => format!("- [{}] {}", if done { 'x' } else { ' ' }, item),
                _ => raw.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn set_item_flips_only_the_matching_line() {
        let s = "- [ ] 补测试\n- [x] 修登录\n* [ ] 发版";
        assert_eq!(super::set_item(s, "补测试", true), "- [x] 补测试\n- [x] 修登录\n* [ ] 发版");
        assert_eq!(super::set_item(s, "修登录", false), "- [ ] 补测试\n- [ ] 修登录\n* [ ] 发版");
        assert_eq!(super::set_item(s, "没有的", true), s);
    }

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
    fn clean_normalises_a_checklist() {
        let raw = "清单：\n- [x] 修好登录 bug\n* [ ] 补测试\n- [X] 更新文档\n随便一句话\n-[ ] 发版\n";
        assert_eq!(clean(raw).unwrap(), "- [x] 修好登录 bug\n- [x] 更新文档\n- [ ] 补测试\n- [ ] 发版");
        assert!(clean("没有清单，只有话").is_none());
        assert!(clean("- [ ]   ").is_none(), "空项不算");
        let many: String = (0..30).map(|i| format!("- [ ] 项{i}\n")).collect();
        assert_eq!(clean(&many).unwrap().lines().count(), MAX_ITEMS);
    }

    #[test]
    fn conversation_excerpt_keeps_every_prompt_and_final_replies() {
        let mut store = MsgStore::for_agent("claude");
        assert!(conversation_excerpt(&store).is_none());
        store.msgs.push_back(msg(1, "user", "text", "第一轮", None));
        store.msgs.push_back(msg(2, "assistant", "text", "中途", None));
        store.msgs.push_back(msg(3, "assistant", "tool_use", "", Some(("Bash", "ls"))));
        store.msgs.push_back(msg(4, "assistant", "text", "第一轮回复", None));
        store.msgs.push_back(msg(5, "user", "text", "第二轮", None));
        store.msgs.push_back(msg(6, "assistant", "text", "第二轮回复", None));
        let e = conversation_excerpt(&store).unwrap();
        assert_eq!(e, "用户: 第一轮\n助手回复: 第一轮回复\n用户: 第二轮\n助手回复: 第二轮回复\n");
    }
}
