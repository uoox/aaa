//! State-machine heuristics: given the visible screen of a silent session,
//! decide waiting (with best-effort question/options parsing) vs idle.
//!
//! Patterns (PROTOCOL.md): `? `, `❯`, `(y/n)`, `[Y/n]`, numbered options,
//! input box `│ >`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuestionOption {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub text: String,
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Waiting(Option<Question>),
    Idle,
}

/// Strip box-drawing borders and outer whitespace for pattern analysis.
/// U+2500–U+257F 是整个 Box Drawing 区块（含 ┌┐└┘├┤ 等所有角与交叉），
/// 逐字符列举永远列不全——审查抓到过 `├─` 开头的行被当成内容。
fn strip_box(line: &str) -> &str {
    line.trim_matches(|c: char| {
        c.is_whitespace() || ('\u{2500}'..='\u{257F}').contains(&c) || c == '¦'
    })
}

/// 横向分隔线/边框残余：strip_box 剥掉竖线和圆角后，`╭────╮`/`──────`
/// 只剩一串横线。这类行没有信息量——把它当「问题文本」或 preview 推进
/// 通知，用户收到的就是一条横杠（实测 bug）。
fn is_rule(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '─' | '━' | '═' | '┄' | '┅' | '┆' | '┈' | '┉' | '╌' | '╍' | '-' | '–'
                        | '—' | '_' | '⎯' | '·' | '•' | '∙' | '＿' | '＝' | '=' | '~'
                )
        })
}

/// 有信息量的行：剥边框后非空且不是分隔线；返回剥好的文本
fn meaningful(line: &str) -> Option<&str> {
    let s = strip_box(line);
    if s.is_empty() || is_rule(s) { None } else { Some(s) }
}

/// Parse `❯ 1. label` / `  2. label` / `3) label`; returns (selected, key, label).
fn parse_option(line: &str) -> Option<(bool, String, String)> {
    let s = strip_box(line);
    let (selected, rest) = if let Some(r) = s.strip_prefix('❯') {
        (true, r.trim_start())
    } else if let Some(r) = s.strip_prefix('>') {
        (true, r.trim_start())
    } else {
        (false, s)
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let after = &rest[digits.len()..];
    let after = after.strip_prefix('.').or_else(|| after.strip_prefix(')'))?;
    let label = after.trim();
    if label.is_empty() {
        return None;
    }
    Some((selected, digits, label.to_string()))
}

fn is_yn_prompt(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("(y/n)")
        || l.contains("[y/n]")
        || l.contains("(yes/no)")
        || l.contains("(y/n/")
}

/// Analyze the visible screen lines (top to bottom) of a silent session.
pub fn analyze(lines: &[String]) -> Verdict {
    // drop trailing blank / rule-only lines（composer 的下边框剥完就是横线）
    let mut end = lines.len();
    while end > 0 && meaningful(&lines[end - 1]).is_none() {
        end -= 1;
    }
    if end == 0 {
        return Verdict::Idle;
    }
    let view = &lines[..end];

    // 1) numbered option block (Claude Code style `❯ 1. xxx`), search the tail:
    //    split tail lines into contiguous runs of option lines, keep the last
    //    plausible run (>= 2 options, or a single option carrying the ❯ marker).
    let tail_start = view.len().saturating_sub(15);
    let mut runs: Vec<Vec<(usize, bool, String, String)>> = Vec::new();
    let mut cur: Vec<(usize, bool, String, String)> = Vec::new();
    for (i, line) in view.iter().enumerate().skip(tail_start) {
        if let Some((sel, key, label)) = parse_option(line) {
            cur.push((i, sel, key, label));
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    // 只认带 ❯/> 高亮标记的选项块：agent 的普通回答里满是「1. …\n2. …”
    // 编号列表，没有高亮标记就当它是文本——否则每个带清单的回答都会被
    // 判成假 waiting 并推高优通知（审查 P0）。真正的选择对话框（Claude
    // Code 等）永远有一项被 ❯ 标着。
    let best_block = runs
        .into_iter()
        .rev()
        .find(|r| r.iter().any(|b| b.1))
        .unwrap_or_default();
    if !best_block.is_empty() {
        // question = nearest non-empty, non-option line above the block
        let first_idx = best_block[0].0;
        let mut text = String::new();
        for line in view[..first_idx].iter().rev() {
            let Some(s) = meaningful(line) else { continue };
            if parse_option(line).is_some() {
                continue;
            }
            text = s.trim_start_matches("? ").trim().to_string();
            break;
        }
        let options = best_block
            .iter()
            .map(|(_, _, k, l)| QuestionOption { key: k.clone(), label: l.clone() })
            .collect();
        return Verdict::Waiting(Some(Question { text, options }));
    }

    // 2) last non-empty line patterns
    let last_raw = &view[view.len() - 1];
    let last = strip_box(last_raw);

    if is_yn_prompt(last) {
        return Verdict::Waiting(Some(Question {
            text: last.to_string(),
            options: vec![
                QuestionOption { key: "y".into(), label: "Yes".into() },
                QuestionOption { key: "n".into(), label: "No".into() },
            ],
        }));
    }
    if let Some(q) = last.strip_prefix("? ") {
        return Verdict::Waiting(Some(Question { text: q.trim().to_string(), options: vec![] }));
    }
    if last.starts_with('❯') {
        return Verdict::Waiting(None);
    }
    if last.ends_with('?') || last.ends_with('？') {
        return Verdict::Waiting(Some(Question { text: last.to_string(), options: vec![] }));
    }
    // 3) input box `│ >`, or an unnumbered `❯` choice list — Claude's trust
    //    dialog being the one every new project hits first. Such a list is
    //    navigated with the arrow keys, so each option carries the Down-arrow
    //    run that lands the highlight on it (the marked one needs none). A
    //    client sends `key` followed by Return, which selects exactly that
    //    entry — without options the phone can only stare at it, and a typed
    //    message would answer whatever happens to be highlighted.
    let lo = view.len().saturating_sub(8);
    for idx in (lo..view.len()).rev() {
        let t = view[idx].trim_start();
        if t.starts_with("│ >") {
            return Verdict::Waiting(None);
        }
        if strip_box(&view[idx]).trim_start().starts_with('❯') {
            let options = unnumbered_options(view, idx);
            if options.is_empty() {
                return Verdict::Waiting(None);
            }
            let text = question_above(view, idx);
            return Verdict::Waiting(Some(Question { text, options }));
        }
    }
    Verdict::Idle
}

/// Options of an unnumbered choice list whose highlighted row is `marker_idx`:
/// that row plus the sibling rows under it, each keyed by the Down-arrow run
/// that reaches it. Empty when the list has no alternatives to offer.
fn unnumbered_options(view: &[String], marker_idx: usize) -> Vec<QuestionOption> {
    let head = strip_box(&view[marker_idx]).trim_start();
    let Some(label) = head.strip_prefix('❯') else { return Vec::new() };
    let mut opts =
        vec![QuestionOption { key: String::new(), label: label.trim().to_string() }];
    for line in view.iter().skip(marker_idx + 1) {
        let t = strip_box(line).trim();
        // The list ends at a blank line, a rule, a hint line, or anything box-drawn.
        if t.is_empty()
            || is_rule(t)
            || t.starts_with('│')
            || t.contains("to confirm")
            || t.contains("to cancel")
            || t.contains("确认")
        {
            break;
        }
        opts.push(QuestionOption { key: "\x1b[B".repeat(opts.len()), label: t.to_string() });
        if opts.len() >= 6 {
            break;
        }
    }
    if opts.len() < 2 { Vec::new() } else { opts }
}

/// Nearest question-ish line above `idx`: prefer one ending in a question mark,
/// else the closest non-empty line.
fn question_above(view: &[String], idx: usize) -> String {
    let mut fallback = String::new();
    for line in view[..idx].iter().rev().take(12) {
        let Some(t) = meaningful(line) else { continue };
        let t = t.trim();
        if t.ends_with('?') || t.ends_with('？') {
            return t.to_string();
        }
        if fallback.is_empty() {
            fallback = t.to_string();
        }
    }
    fallback
}

/// Convenience: run the vt100 screen through the heuristics.
pub fn analyze_screen(screen: &vt100::Screen) -> Verdict {
    let (_, cols) = screen.size();
    let lines: Vec<String> = screen.rows(0, cols).collect();
    analyze(&lines)
}

/// Preview: last `n` *meaningful* lines of the visible screen——空行和边框
/// 分隔线不算数（preview 会进 ntfy 通知正文，横杠行毫无信息量）。
pub fn preview(screen: &vt100::Screen, n: usize) -> String {
    let (_, cols) = screen.size();
    let lines: Vec<String> = screen.rows(0, cols).collect();
    let picked: Vec<&str> = lines
        .iter()
        .filter_map(|l| meaningful(l))
        .collect();
    let start = picked.len().saturating_sub(n);
    picked[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(input: &str) -> vt100::Parser {
        let mut p = vt100::Parser::new(24, 80, 100);
        p.process(input.as_bytes());
        p
    }

    #[test]
    fn claude_style_numbered_options_with_ansi() {
        // ANSI colours + ❯ marker, as Claude Code renders choice dialogs
        let p = feed(
            "\x1b[2J\x1b[H\x1b[1m? 原型是否需要包含 iPad 布局？\x1b[0m\r\n\r\n\x1b[36m❯ 1. 需要\x1b[0m\r\n  2. 不需要\r\n",
        );
        match analyze_screen(p.screen()) {
            Verdict::Waiting(Some(q)) => {
                assert_eq!(q.text, "原型是否需要包含 iPad 布局？");
                assert_eq!(q.options.len(), 2);
                assert_eq!(q.options[0], QuestionOption { key: "1".into(), label: "需要".into() });
                assert_eq!(q.options[1], QuestionOption { key: "2".into(), label: "不需要".into() });
            }
            other => panic!("expected waiting with question, got {other:?}"),
        }
    }

    #[test]
    fn yn_prompt() {
        let p = feed("Overwrite existing file? (y/n) ");
        match analyze_screen(p.screen()) {
            Verdict::Waiting(Some(q)) => {
                assert!(q.text.contains("Overwrite"));
                assert_eq!(q.options.len(), 2);
                assert_eq!(q.options[0].key, "y");
            }
            other => panic!("expected y/n waiting, got {other:?}"),
        }
    }

    #[test]
    fn bracketed_yn() {
        let p = feed("Proceed? [Y/n] ");
        assert!(matches!(analyze_screen(p.screen()), Verdict::Waiting(Some(_))));
    }

    #[test]
    fn boxed_input_composer_is_waiting_without_question() {
        let p = feed("done thinking\r\n╭──────────╮\r\n│ > \r\n╰──────────╯\r\n");
        assert_eq!(analyze_screen(p.screen()), Verdict::Waiting(None));
    }

    #[test]
    fn claude_trust_dialog_unnumbered_options() {
        // real screen captured from a fresh `claude` in a new folder: unnumbered
        // ❯ options with a hint line below. Every new project meets this first,
        // so the options must reach the phone as tappable choices — keyed by the
        // Down-arrow run that selects them.
        let p = feed(
            " Do you trust the files in this folder?\r\n\r\n ❯ No, exit\r\n   Yes, I trust this folder\r\n\r\n Enter to confirm · Esc to cancel\r\n",
        );
        let Verdict::Waiting(Some(q)) = analyze_screen(p.screen()) else {
            panic!("trust dialog must parse into a question with options");
        };
        assert_eq!(q.text, "Do you trust the files in this folder?");
        assert_eq!(q.options.len(), 2);
        assert_eq!(q.options[0].key, "");
        assert_eq!(q.options[0].label, "No, exit");
        assert_eq!(q.options[1].key, "\x1b[B");
        assert_eq!(q.options[1].label, "Yes, I trust this folder");
    }

    #[test]
    fn lone_unnumbered_marker_has_no_options() {
        // a single ❯ row with nothing under it is a cursor, not a choice list
        let p = feed(" ❯ \r\n");
        assert_eq!(analyze_screen(p.screen()), Verdict::Waiting(None));
    }

    #[test]
    fn plain_output_is_idle() {
        let p = feed("compiling foo v0.1.0\r\nFinished dev profile\r\n");
        assert_eq!(analyze_screen(p.screen()), Verdict::Idle);
    }

    #[test]
    fn boxed_options_inside_borders() {
        let p = feed(
            "│ Do you want to make this edit to main.rs? │\r\n│ ❯ 1. Yes │\r\n│   2. Yes, allow all edits during this session │\r\n│   3. No, and tell Claude what to do differently │\r\n",
        );
        match analyze_screen(p.screen()) {
            Verdict::Waiting(Some(q)) => {
                assert_eq!(q.options.len(), 3);
                assert!(q.text.contains("make this edit"));
                assert_eq!(q.options[2].key, "3");
            }
            other => panic!("expected boxed options, got {other:?}"),
        }
    }

    #[test]
    fn preview_takes_last_lines() {
        let p = feed("l1\r\nl2\r\nl3\r\nl4\r\nl5\r\n\r\n\r\n");
        assert_eq!(preview(p.screen(), 4), "l2\nl3\nl4\nl5");
    }

    #[test]
    fn numbered_list_in_answer_is_not_a_question() {
        // agent 回答里的编号清单（无 ❯ 高亮）+ 下方 composer：
        // 是文本不是选择对话框，只算「输入框 waiting」
        let p = feed(
            "计划如下：\r\n1. 先改 daemon\r\n2. 再改客户端\r\n3. 发版\r\n╭────────╮\r\n│ > \r\n╰────────╯\r\n",
        );
        assert_eq!(analyze_screen(p.screen()), Verdict::Waiting(None));
    }

    #[test]
    fn rules_never_become_question_text_or_preview() {
        // 实测 bug：claude code 的分隔线/输入框边框剥完竖线只剩横线，
        // 曾被当成问题文本/preview 推进通知——用户收到一条横杠。
        // 1) 选项块上方紧邻分隔线：问题要跳过横线取到真文本
        let p = feed(
            "要选哪个部署方式？\r\n────────────────────\r\n❯ 1. docker\r\n  2. 裸机\r\n",
        );
        match analyze_screen(p.screen()) {
            Verdict::Waiting(Some(q)) => assert_eq!(q.text, "要选哪个部署方式？"),
            other => panic!("expected waiting, got {other:?}"),
        }
        // 2) preview 不含分隔线与空行
        let p = feed("done thinking\r\n╭──────────╮\r\n│ > \r\n╰──────────╯\r\n");
        let pv = preview(p.screen(), 4);
        assert!(!pv.contains('─'), "preview 不应含横线: {pv:?}");
        assert!(pv.contains("done thinking"));
        // 3) 未编号 ❯ 列表被分隔线截断，不把横线收作选项
        let p = feed(
            "Pick one\r\n❯ first\r\n  second\r\n──────\r\n  stray\r\n",
        );
        if let Verdict::Waiting(Some(q)) = analyze_screen(p.screen()) {
            assert!(
                q.options.iter().all(|o| !o.label.contains('─') && o.label != "stray"),
                "选项不应越过分隔线: {:?}",
                q.options
            );
        }
        // 4) 满屏只有横线 = 没有可判内容 → idle，绝不 waiting
        let p = feed("──────────\r\n──────────\r\n");
        assert_eq!(analyze_screen(p.screen()), Verdict::Idle);
    }
}
