//! 消息流按「轮」折叠——纯函数，不碰 gpui，与 Android `StreamFold.kt` 同构。
//!
//! 一轮 = 用户发出的一条 text 到下一条之间的全部消息。默认只露出用户消息与这一轮
//! 最后一条 assistant text（reply）；中间的思考 / 工具调用 / 工具结果 / 中途文本
//! 全部折进 process；question / answer 永不折叠，按真实位置独立成项。

use std::collections::HashSet;

use crate::model::ChatMessage;

/// 一轮。`key` 取轮内第一条消息的 seq——用户轮就是用户消息的 seq，跨次刷新稳定。
/// `live`：会话仍在跑且这是最后一轮——过程行画成「进行中」并带最近一步。
#[derive(Debug)]
pub struct Turn<'a> {
    pub user: Option<&'a ChatMessage>,
    pub process: Vec<&'a ChatMessage>,
    pub reply: Option<&'a ChatMessage>,
    pub questions: Vec<&'a ChatMessage>,
    pub key: u64,
    pub live: bool,
}

/// 列表的一项。`key()` 带类型前缀，同一条消息从 Reply 变成 Step 时 key 跟着变。
#[derive(Debug)]
pub enum StreamItem<'a> {
    User(&'a ChatMessage),
    /// 折叠行；`live_tail` 是 live 尾轮里最后一条 tool_use，折叠着也能看出它在干什么。
    Fold {
        turn_key: u64,
        steps: Vec<&'a ChatMessage>,
        live_tail: Option<&'a ChatMessage>,
        live: bool,
    },
    Step(&'a ChatMessage),
    Reply(&'a ChatMessage),
    Question(&'a ChatMessage),
    Answer(&'a ChatMessage),
}

impl StreamItem<'_> {
    /// 带类型前缀的稳定键，与 Android 的 `StreamItem.key` 同形（测试断言用）
    #[cfg(test)]
    pub fn key(&self) -> String {
        match self {
            StreamItem::User(m) => format!("u{}", m.seq),
            StreamItem::Fold { turn_key, .. } => format!("f{turn_key}"),
            StreamItem::Step(m) => format!("s{}", m.seq),
            StreamItem::Reply(m) => format!("r{}", m.seq),
            StreamItem::Question(m) => format!("q{}", m.seq),
            StreamItem::Answer(m) => format!("a{}", m.seq),
        }
    }
}

fn starts_turn(m: &ChatMessage) -> bool {
    m.role == "user" && m.kind == "text"
}

fn is_assistant_text(m: &ChatMessage) -> bool {
    m.role == "assistant" && m.kind == "text"
}

fn is_form(m: &ChatMessage) -> bool {
    m.kind == "question" || m.kind == "answer"
}

/// 每条 user text 开一轮；第一轮之前的非 user 消息归入一个 `user == None` 的首轮
/// （增量拉取只拿到后半段时常见）。
pub fn fold_turns(messages: &[ChatMessage], live: bool) -> Vec<Turn<'_>> {
    let mut groups: Vec<Vec<&ChatMessage>> = Vec::new();
    for m in messages {
        if groups.is_empty() || starts_turn(m) {
            groups.push(Vec::new());
        }
        groups.last_mut().unwrap().push(m);
    }
    let last = groups.len().saturating_sub(1);
    groups
        .into_iter()
        .enumerate()
        .map(|(i, g)| {
            let key = g[0].seq;
            let user = g.first().copied().filter(|m| starts_turn(m));
            let body = if user.is_some() { &g[1..] } else { &g[..] };
            let questions: Vec<&ChatMessage> = body.iter().copied().filter(|m| is_form(m)).collect();
            let rest: Vec<&ChatMessage> = body.iter().copied().filter(|m| !is_form(m)).collect();
            let reply = rest.iter().rev().copied().find(|m| is_assistant_text(m));
            let process = match reply {
                Some(r) => rest.into_iter().filter(|m| m.seq != r.seq).collect(),
                None => rest,
            };
            Turn {
                user,
                process,
                reply,
                questions,
                key,
                live: live && i == last,
            }
        })
        .collect()
}

/// 轮 → 列表项。轮内先出 User，随后 Fold / Reply / Question / Answer 按各自起始 seq
/// 排序（question 可能出现在过程中间，也可能在回复之后，跟着真实顺序走）；展开的
/// Fold 紧跟各 Step。process 为空不出 Fold。
pub fn flatten<'a>(turns: &[Turn<'a>], expanded: &HashSet<u64>) -> Vec<StreamItem<'a>> {
    let mut out = Vec::new();
    for t in turns {
        if let Some(u) = t.user {
            out.push(StreamItem::User(u));
        }
        let mut blocks: Vec<(u64, Vec<StreamItem<'a>>)> = Vec::new();
        if let Some(first) = t.process.first() {
            let fold = StreamItem::Fold {
                turn_key: t.key,
                steps: t.process.clone(),
                live_tail: if t.live {
                    t.process.iter().rev().copied().find(|m| m.kind == "tool_use")
                } else {
                    None
                },
                live: t.live,
            };
            let mut block = vec![fold];
            if expanded.contains(&t.key) {
                block.extend(t.process.iter().map(|m| StreamItem::Step(m)));
            }
            blocks.push((first.seq, block));
        }
        if let Some(r) = t.reply {
            blocks.push((r.seq, vec![StreamItem::Reply(r)]));
        }
        for q in &t.questions {
            let item = if q.kind == "question" {
                StreamItem::Question(q)
            } else {
                StreamItem::Answer(q)
            };
            blocks.push((q.seq, vec![item]));
        }
        blocks.sort_by_key(|(seq, _)| *seq);
        for (_, b) in blocks {
            out.extend(b);
        }
    }
    out
}

/// 步数 = tool_use 条数；thinking / 中途文本 / 结果都不算步。
pub fn step_count(steps: &[&ChatMessage]) -> usize {
    steps.iter().filter(|m| m.kind == "tool_use").count()
}

fn tool_name(m: &ChatMessage) -> &str {
    match m.tool.as_ref().map(|t| t.name.trim()) {
        Some(n) if !n.is_empty() => n,
        _ => "tool",
    }
}

/// 折叠态标签：`过程 · 10 步 · Bash ×7 · Read ×3`——工具名按次数降序取前三，
/// 同次数按首次出现。
pub fn fold_label(steps: &[&ChatMessage]) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for m in steps.iter().filter(|m| m.kind == "tool_use") {
        let name = tool_name(m);
        match counts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, c)) => *c += 1,
            None => counts.push((name, 1)),
        }
    }
    // 稳定排序保留首次出现的先后
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let mut s = format!("过程 · {} 步", step_count(steps));
    for (name, n) in counts.iter().take(3) {
        s.push_str(&format!(" · {name} ×{n}"));
    }
    s
}

/// live 尾轮的折叠态标签：`进行中 · 5 步 · 最近：Bash cargo test`。
pub fn live_label(steps: &[&ChatMessage], tail: Option<&ChatMessage>) -> String {
    let base = format!("进行中 · {} 步", step_count(steps));
    let Some(tail) = tail else {
        return base;
    };
    let summary = tail.tool.as_ref().map(|t| t.summary.trim()).unwrap_or("");
    let what = if summary.is_empty() {
        tool_name(tail).to_string()
    } else {
        format!("{} {}", tool_name(tail), summary)
    };
    format!("{base} · 最近：{what}")
}

/// 尾轮是否「进行中」：会话活着，且最后一轮（最后一条 user text 之后）还没有任何
/// assistant text 当回复。question / answer 不算回复。
pub fn tail_is_live(messages: &[ChatMessage], alive: bool) -> bool {
    if !alive || messages.is_empty() {
        return false;
    }
    !messages
        .iter()
        .rev()
        .take_while(|m| !starts_turn(m))
        .any(|m| is_assistant_text(m))
}

/// 消息集变化后要不要滚到底：首批数据总是到底；之后只有变化**前**就在底部才跟——
/// 用户正在翻历史时新消息不该把人拽下去。
pub fn should_follow_tail(was_at_bottom: bool, had_messages: bool, has_messages: bool) -> bool {
    has_messages && (!had_messages || was_at_bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ToolInfo;

    fn msg(seq: u64, role: &str, kind: &str, text: &str, tool: Option<ToolInfo>) -> ChatMessage {
        ChatMessage {
            seq,
            ts: String::new(),
            role: role.into(),
            kind: kind.into(),
            text: text.into(),
            tool,
            question: None,
        }
    }
    fn info(name: &str, summary: &str) -> ToolInfo {
        ToolInfo {
            name: name.into(),
            summary: summary.into(),
            status: "ok".into(),
        }
    }
    fn user(seq: u64) -> ChatMessage {
        msg(seq, "user", "text", "做", None)
    }
    fn say(seq: u64) -> ChatMessage {
        msg(seq, "assistant", "text", "好", None)
    }
    fn think(seq: u64) -> ChatMessage {
        msg(seq, "assistant", "thinking", "hmm", None)
    }
    fn use_(seq: u64, name: &str) -> ChatMessage {
        use_s(seq, name, "")
    }
    fn use_s(seq: u64, name: &str, summary: &str) -> ChatMessage {
        msg(seq, "tool", "tool_use", "", Some(info(name, summary)))
    }
    fn result(seq: u64) -> ChatMessage {
        msg(seq, "tool", "tool_result", "out", Some(info("", "")))
    }
    fn question(seq: u64) -> ChatMessage {
        msg(seq, "assistant", "question", "which?", None)
    }
    fn answer(seq: u64) -> ChatMessage {
        msg(seq, "user", "answer", "that one", Some(info("AskUserQuestion", "")))
    }
    fn seqs(v: &[&ChatMessage]) -> Vec<u64> {
        v.iter().map(|m| m.seq).collect()
    }
    fn keys(items: &[StreamItem]) -> Vec<String> {
        items.iter().map(|i| i.key()).collect()
    }
    fn folds<'a, 'b>(items: &'b [StreamItem<'a>]) -> Vec<&'b StreamItem<'a>> {
        items.iter().filter(|i| matches!(i, StreamItem::Fold { .. })).collect()
    }
    fn none() -> HashSet<u64> {
        HashSet::new()
    }

    // 1. 两轮基本切分
    #[test]
    fn splits_into_turns_on_user_text() {
        let msgs = vec![user(1), use_(2, "Bash"), result(3), say(4), user(5), say(6)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].user.map(|m| m.seq), Some(1));
        assert_eq!(seqs(&turns[0].process), vec![2, 3]);
        assert_eq!(turns[0].reply.map(|m| m.seq), Some(4));
        assert_eq!(turns[0].key, 1);
        assert_eq!(turns[1].user.map(|m| m.seq), Some(5));
        assert!(turns[1].process.is_empty());
        assert_eq!(turns[1].reply.map(|m| m.seq), Some(6));
    }

    // 2. 轮内多条 assistant text：只有最后一条是 reply
    #[test]
    fn only_the_last_assistant_text_is_the_reply() {
        let msgs = vec![user(1), say(2), use_(3, "Read"), say(4), use_(5, "Edit"), say(6)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].reply.map(|m| m.seq), Some(6));
        assert_eq!(seqs(&turns[0].process), vec![2, 3, 4, 5]);
    }

    // 3. question 永不折叠且不占 reply 位
    #[test]
    fn questions_stay_out_of_process_and_reply() {
        let msgs = vec![user(1), use_(2, "Bash"), question(3)];
        let turns = fold_turns(&msgs, false);
        let t = &turns[0];
        assert_eq!(seqs(&t.questions), vec![3]);
        assert!(t.reply.is_none());
        assert_eq!(seqs(&t.process), vec![2]);
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "f1", "q3"]);

        // 问题排在回复之后也不改变它独立成项
        let msgs = vec![user(1), say(2), question(3)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns[0].reply.map(|m| m.seq), Some(2));
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "r2", "q3"]);
    }

    // 3b. answer 与 question 一样独立成项、不折叠、不占 reply 位；键前缀 a
    #[test]
    fn answers_are_standalone_items_too() {
        let msgs = vec![user(1), question(2), answer(3), use_(4, "Edit"), say(5)];
        let turns = fold_turns(&msgs, false);
        let t = &turns[0];
        assert_eq!(seqs(&t.questions), vec![2, 3]);
        assert_eq!(seqs(&t.process), vec![4]);
        assert_eq!(t.reply.map(|m| m.seq), Some(5));
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "q2", "a3", "f1", "r5"]);
    }

    // 4. 首条非 user 的消息归入 user==None 首轮
    #[test]
    fn leading_non_user_messages_form_a_headless_turn() {
        let msgs = vec![use_(7, "Bash"), result(8), say(9), user(10), say(11)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns.len(), 2);
        assert!(turns[0].user.is_none());
        assert_eq!(turns[0].key, 7);
        assert_eq!(seqs(&turns[0].process), vec![7, 8]);
        assert_eq!(turns[0].reply.map(|m| m.seq), Some(9));
        assert_eq!(turns[1].user.map(|m| m.seq), Some(10));
    }

    // 5. process 空 → 不出 Fold 项
    #[test]
    fn empty_process_emits_no_fold() {
        let msgs = vec![user(1), say(2)];
        let turns = fold_turns(&msgs, true);
        let items = flatten(&turns, &HashSet::from([1]));
        assert_eq!(keys(&items), ["u1", "r2"]);
        assert!(folds(&items).is_empty());
    }

    // 6. Fold 标签：计数与排序，thinking 不计
    #[test]
    fn fold_label_counts_tool_uses_and_ranks_names() {
        let mut owned = vec![think(1)];
        owned.extend((2..=8).map(|s| use_(s, "Bash")));
        owned.extend((9..=11).map(|s| use_(s, "Read")));
        owned.push(result(12));
        owned.push(msg(13, "assistant", "text", "中途", None));
        let steps: Vec<&ChatMessage> = owned.iter().collect();
        assert_eq!(step_count(&steps), 10);
        assert_eq!(fold_label(&steps), "过程 · 10 步 · Bash ×7 · Read ×3");

        // 前三之外的工具不列；同次数按首次出现
        let many = vec![
            use_(1, "Edit"),
            use_(2, "Grep"),
            use_(3, "Bash"),
            use_(4, "Bash"),
            use_(5, "Read"),
            use_(6, "Write"),
        ];
        let many: Vec<&ChatMessage> = many.iter().collect();
        assert_eq!(fold_label(&many), "过程 · 6 步 · Bash ×2 · Edit ×1 · Grep ×1");
        let only_think = [think(1)];
        assert_eq!(fold_label(&[&only_think[0]]), "过程 · 0 步");
        // 没带工具名的 tool_use 记作 tool
        let anon = [msg(1, "tool", "tool_use", "", Some(info("  ", "")))];
        assert_eq!(fold_label(&[&anon[0]]), "过程 · 1 步 · tool ×1");
    }

    // 7. live 尾轮的 live_tail = 最后一条 tool_use
    #[test]
    fn live_tail_is_the_last_tool_use_of_the_live_last_turn() {
        let msgs = vec![
            user(1),
            use_s(2, "Read", "a.kt"),
            result(3),
            say(4),
            use_s(5, "Bash", "cargo test"),
            result(6),
        ];
        let turns = fold_turns(&msgs, true);
        let items = flatten(&turns, &none());
        let fs = folds(&items);
        assert_eq!(fs.len(), 1);
        let StreamItem::Fold { steps, live_tail, live, .. } = fs[0] else {
            unreachable!()
        };
        assert!(*live);
        assert_eq!(live_tail.map(|m| m.seq), Some(5));
        assert_eq!(live_label(steps, *live_tail), "进行中 · 2 步 · 最近：Bash cargo test");
        assert_eq!(live_label(steps, None), "进行中 · 2 步");
        // 最新那条 assistant text 暂当 reply；之后的工具调用已经滚进 process
        assert_eq!(keys(&items), ["u1", "f1", "r4"]);

        // 会话不在 running：没有 live_tail
        let turns = fold_turns(&msgs, false);
        let items = flatten(&turns, &none());
        let StreamItem::Fold { live_tail, live, .. } = folds(&items)[0] else {
            unreachable!()
        };
        assert!(!*live);
        assert!(live_tail.is_none());

        // 只有最后一轮算 live
        let mut two = msgs.clone();
        two.push(user(7));
        two.push(use_(8, "Edit"));
        let turns = fold_turns(&two, true);
        let items = flatten(&turns, &none());
        let fs = folds(&items);
        let lives: Vec<bool> = fs
            .iter()
            .map(|f| matches!(f, StreamItem::Fold { live: true, .. }))
            .collect();
        assert_eq!(lives, [false, true]);
        let StreamItem::Fold { live_tail, .. } = fs[1] else {
            unreachable!()
        };
        assert_eq!(live_tail.map(|m| m.seq), Some(8));
    }

    #[test]
    fn expanded_fold_is_followed_by_its_steps() {
        let msgs = vec![
            user(1),
            think(2),
            use_(3, "Bash"),
            result(4),
            say(5),
            user(6),
            use_(7, "Read"),
            say(8),
        ];
        let turns = fold_turns(&msgs, false);
        let items = flatten(&turns, &HashSet::from([1]));
        assert_eq!(
            keys(&items),
            ["u1", "f1", "s2", "s3", "s4", "r5", "u6", "f6", "r8"]
        );
        let collapsed = flatten(&turns, &none());
        assert_eq!(keys(&collapsed), ["u1", "f1", "r5", "u6", "f6", "r8"]);
    }

    #[test]
    fn empty_input_folds_to_nothing() {
        assert!(fold_turns(&[], true).is_empty());
        assert!(flatten(&[], &none()).is_empty());
    }

    // 尾轮进行中 = 活着且最后一轮没有 assistant text；question 不算回复
    #[test]
    fn tail_is_live_when_last_turn_has_no_reply() {
        assert!(tail_is_live(&[user(1), use_(2, "Bash")], true));
        assert!(tail_is_live(&[user(1)], true));
        assert!(tail_is_live(&[user(1), question(2)], true));
        assert!(!tail_is_live(&[user(1), use_(2, "Bash")], false));
        assert!(!tail_is_live(&[user(1), say(2)], true));
        // 回复之后又来了工具调用：最新 text 仍暂当 reply，不算进行中
        assert!(!tail_is_live(&[user(1), say(2), use_(3, "Bash")], true));
        // 上一轮有回复不影响本轮
        assert!(tail_is_live(&[user(1), say(2), user(3), think(4)], true));
        assert!(!tail_is_live(&[], true));
    }

    // 滚动判定：首批数据到底；之后只有变化前在底部才跟
    #[test]
    fn follows_tail_only_when_already_at_bottom_or_on_first_data() {
        assert!(should_follow_tail(false, false, true));
        assert!(should_follow_tail(true, true, true));
        assert!(!should_follow_tail(false, true, true));
        assert!(!should_follow_tail(true, true, false));
    }
}
