//! 消息流按「轮」折叠——纯函数，不碰 gpui，与 Android `StreamFold.kt` 同构。
//!
//! 一轮 = 用户发出的一条 text 到下一条之间的全部消息。**assistant 的每一条 text 都是
//! 回答**，一律露出（Claude 的回答天生分段：说一句 → 干活 → 再说一句），2026-09-07
//! 之前只留最后一条、其余折进过程，看起来就像把回答当成了思考。折叠里只有思考 /
//! 工具调用 / 工具结果 / system；轮内的过程按真实位置切成若干折叠段，夹在各段回答
//! 之间；question / answer 永不折叠，按真实位置独立成项。

use std::collections::HashSet;

use crate::model::ChatMessage;

/// 一轮。`key` 取轮内第一条消息的 seq——用户轮就是用户消息的 seq，跨次刷新稳定。
/// `body` 是除用户消息外的全部消息，保持原序。
/// `live`：会话仍在跑且这是最后一轮——尾部那段过程画成「进行中」并带最近一步。
#[derive(Debug)]
pub struct Turn<'a> {
    pub user: Option<&'a ChatMessage>,
    pub body: Vec<&'a ChatMessage>,
    pub key: u64,
    pub live: bool,
}

impl<'a> Turn<'a> {
    /// 轮内所有会被折叠的消息（思考 / 工具 / system）
    pub fn steps(&self) -> Vec<&'a ChatMessage> {
        self.body.iter().copied().filter(|m| is_step(m)).collect()
    }
    /// 轮内所有回答（assistant text），按顺序
    pub fn replies(&self) -> Vec<&'a ChatMessage> {
        self.body.iter().copied().filter(|m| is_assistant_text(m)).collect()
    }
}

/// 列表的一项。`key()` 带类型前缀，同一条消息从 Reply 变成 Step 时 key 跟着变。
#[derive(Debug)]
pub enum StreamItem<'a> {
    User(&'a ChatMessage),
    /// 折叠段；`fold_key` = 段内第一条消息的 seq（展开状态按它记）。
    /// `live_tail` 是 live 尾段里最后一条 tool_use，折叠着也能看出它在干什么。
    Fold {
        fold_key: u64,
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
            StreamItem::Fold { fold_key, .. } => format!("f{fold_key}"),
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

/// 会被折叠的：思考 / 工具调用 / 工具结果 / system 提示。回答与表单永远不折。
fn is_step(m: &ChatMessage) -> bool {
    !is_assistant_text(m) && !is_form(m)
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
            let body = if user.is_some() { g[1..].to_vec() } else { g };
            Turn { user, body, key, live: live && i == last }
        })
        .collect()
}

/// 连续的过程消息切成段（原序）。
fn segments<'a>(body: &[&'a ChatMessage]) -> Vec<Vec<&'a ChatMessage>> {
    let mut segs: Vec<Vec<&'a ChatMessage>> = Vec::new();
    let mut open = false;
    for m in body {
        if is_step(m) {
            if !open {
                segs.push(Vec::new());
                open = true;
            }
            segs.last_mut().unwrap().push(*m);
        } else {
            open = false;
        }
    }
    segs
}

/// 轮 → 列表项。轮内先出 User，随后按真实顺序走：连续的过程消息并成一个 Fold
/// （展开时紧跟它的 Step），回答 / question / answer 各自成项。只有 live 轮**贴在
/// 轮尾**的那一段画成「进行中」——后面还有回答，说明那段已经结束了。
pub fn flatten<'a>(turns: &[Turn<'a>], expanded: &HashSet<u64>) -> Vec<StreamItem<'a>> {
    let mut out = Vec::new();
    for t in turns {
        if let Some(u) = t.user {
            out.push(StreamItem::User(u));
        }
        let segs = segments(&t.body);
        let live_key = match (t.live, t.body.last(), segs.last()) {
            (true, Some(last), Some(seg)) if is_step(last) => Some(seg[0].seq),
            _ => None,
        };
        let mut seg_ix = 0usize;
        let mut in_seg = false;
        for m in &t.body {
            if is_step(m) {
                if !in_seg {
                    let seg = &segs[seg_ix];
                    let fold_key = seg[0].seq;
                    let live = Some(fold_key) == live_key;
                    out.push(StreamItem::Fold {
                        fold_key,
                        steps: seg.clone(),
                        live_tail: if live { seg.iter().rev().copied().find(|m| m.kind == "tool_use") } else { None },
                        live,
                    });
                    if expanded.contains(&fold_key) {
                        out.extend(seg.iter().map(|m| StreamItem::Step(m)));
                    }
                    seg_ix += 1;
                    in_seg = true;
                }
                continue;
            }
            in_seg = false;
            out.push(if is_assistant_text(m) {
                StreamItem::Reply(m)
            } else if m.kind == "question" {
                StreamItem::Question(m)
            } else {
                StreamItem::Answer(m)
            });
        }
    }
    out
}

/// 步数 = tool_use 条数；thinking / 结果不算步。
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

/// 尾部是否「进行中」：会话活着，且最后一条消息不是回答。回答之后又来了工具调用
/// 说明这一轮还在往下做（回答本来就可以分段），此时仍然是进行中。
pub fn tail_is_live(messages: &[ChatMessage], alive: bool) -> bool {
    alive && messages.last().map(|m| !is_assistant_text(m)).unwrap_or(false)
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
        assert_eq!(seqs(&turns[0].steps()), vec![2, 3]);
        assert_eq!(seqs(&turns[0].replies()), vec![4]);
        assert_eq!(turns[0].key, 1);
        assert_eq!(turns[1].user.map(|m| m.seq), Some(5));
        assert!(turns[1].steps().is_empty());
        assert_eq!(seqs(&turns[1].replies()), vec![6]);
    }

    // 2. 轮内每条 assistant text 都是回答，过程按位置切成多段（2026-09-07 修：
    //    以前只留最后一条，中间几段真回答被折进「过程」，看着像思考）
    #[test]
    fn every_assistant_text_is_a_reply_and_process_splits_around_them() {
        let msgs = vec![user(1), say(2), use_(3, "Read"), result(4), say(5), use_(6, "Edit"), say(7)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns.len(), 1);
        assert_eq!(seqs(&turns[0].replies()), vec![2, 5, 7]);
        assert_eq!(seqs(&turns[0].steps()), vec![3, 4, 6]);
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "r2", "f3", "r5", "f6", "r7"]);
        // 展开只展开被点的那一段
        assert_eq!(
            keys(&flatten(&turns, &HashSet::from([3]))),
            ["u1", "r2", "f3", "s3", "s4", "r5", "f6", "r7"]
        );
    }

    // 3. question 永不折叠，也不吃掉回答
    #[test]
    fn questions_stay_out_of_the_fold() {
        let msgs = vec![user(1), use_(2, "Bash"), question(3)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(seqs(&turns[0].steps()), vec![2]);
        assert!(turns[0].replies().is_empty());
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "f2", "q3"]);

        // 问题排在回复之后也不改变它独立成项
        let msgs = vec![user(1), say(2), question(3)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "r2", "q3"]);
    }

    // 3b. answer 与 question 一样独立成项、不折叠；键前缀 a
    #[test]
    fn answers_are_standalone_items_too() {
        let msgs = vec![user(1), question(2), answer(3), use_(4, "Edit"), say(5)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(seqs(&turns[0].steps()), vec![4]);
        assert_eq!(keys(&flatten(&turns, &none())), ["u1", "q2", "a3", "f4", "r5"]);
    }

    // 4. 首条非 user 的消息归入 user==None 首轮
    #[test]
    fn leading_non_user_messages_form_a_headless_turn() {
        let msgs = vec![use_(7, "Bash"), result(8), say(9), user(10), say(11)];
        let turns = fold_turns(&msgs, false);
        assert_eq!(turns.len(), 2);
        assert!(turns[0].user.is_none());
        assert_eq!(turns[0].key, 7);
        assert_eq!(seqs(&turns[0].steps()), vec![7, 8]);
        assert_eq!(seqs(&turns[0].replies()), vec![9]);
        assert_eq!(turns[1].user.map(|m| m.seq), Some(10));
    }

    // 5. 没有过程 → 不出 Fold 项
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

    // 7. 只有贴在轮尾的那一段是 live，live_tail = 段内最后一条 tool_use
    #[test]
    fn only_the_trailing_segment_of_the_live_turn_is_live() {
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
        assert_eq!(keys(&items), ["u1", "f2", "r4", "f5"], "中途那句 say(4) 露在外面");
        let fs = folds(&items);
        assert_eq!(fs.len(), 2);
        let lives: Vec<bool> = fs.iter().map(|f| matches!(f, StreamItem::Fold { live: true, .. })).collect();
        assert_eq!(lives, [false, true]);
        let StreamItem::Fold { steps, live_tail, .. } = fs[1] else { unreachable!() };
        assert_eq!(live_tail.map(|m| m.seq), Some(5));
        assert_eq!(live_label(steps, *live_tail), "进行中 · 1 步 · 最近：Bash cargo test");
        assert_eq!(live_label(steps, None), "进行中 · 1 步");

        // 会话不在 running：没有 live 段
        let turns = fold_turns(&msgs, false);
        let items = flatten(&turns, &none());
        assert!(folds(&items).iter().all(|f| matches!(f, StreamItem::Fold { live: false, live_tail: None, .. })));

        // 只有最后一轮算 live
        let mut two = msgs.clone();
        two.push(user(7));
        two.push(use_(8, "Edit"));
        let turns = fold_turns(&two, true);
        let items2 = flatten(&turns, &none());
        let fs2 = folds(&items2);
        let lives: Vec<bool> = fs2.iter().map(|f| matches!(f, StreamItem::Fold { live: true, .. })).collect();
        assert_eq!(lives, [false, false, true]);

        // 尾段后面又出了回答 → 那段不再是「进行中」
        let done = vec![user(1), use_(2, "Bash"), say(3)];
        let turns3 = fold_turns(&done, true);
        let items = flatten(&turns3, &none());
        assert!(folds(&items).iter().all(|f| matches!(f, StreamItem::Fold { live: false, .. })));
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
        // 展开状态按段键（段内第一条 seq）记
        let items = flatten(&turns, &HashSet::from([2]));
        assert_eq!(
            keys(&items),
            ["u1", "f2", "s2", "s3", "s4", "r5", "u6", "f7", "r8"]
        );
        let collapsed = flatten(&turns, &none());
        assert_eq!(keys(&collapsed), ["u1", "f2", "r5", "u6", "f7", "r8"]);
    }

    #[test]
    fn empty_input_folds_to_nothing() {
        assert!(fold_turns(&[], true).is_empty());
        assert!(flatten(&[], &none()).is_empty());
    }

    // 尾部进行中 = 活着且最后一条不是回答
    #[test]
    fn tail_is_live_until_an_answer_lands() {
        assert!(tail_is_live(&[user(1), use_(2, "Bash")], true));
        assert!(tail_is_live(&[user(1)], true));
        assert!(tail_is_live(&[user(1), question(2)], true));
        assert!(!tail_is_live(&[user(1), use_(2, "Bash")], false));
        assert!(!tail_is_live(&[user(1), say(2)], true));
        // 回答之后又来了工具调用：这一轮还在往下做（回答本来就分段）
        assert!(tail_is_live(&[user(1), say(2), use_(3, "Bash")], true));
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
