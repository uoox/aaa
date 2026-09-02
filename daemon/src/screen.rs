//! Plain-text helpers over the vt100 screen: the `preview` shown in session
//! lists. Nothing here interprets the screen — the daemon stopped guessing
//! what a TUI is asking (see PROTOCOL.md, 2026-09-02); dialogs now come
//! structured from the agent transcript.

/// Strip box-drawing borders and outer whitespace.
/// U+2500–U+257F 是整个 Box Drawing 区块（含 ┌┐└┘├┤ 等所有角与交叉）。
fn strip_box(line: &str) -> &str {
    line.trim_matches(|c: char| {
        c.is_whitespace() || ('\u{2500}'..='\u{257F}').contains(&c) || c == '¦'
    })
}

/// 横向分隔线/边框残余：strip_box 剥掉竖线和圆角后，`╭────╮`/`──────`
/// 只剩一串横线，没有信息量。
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

/// Preview: last `n` *meaningful* lines of the visible screen——空行和边框
/// 分隔线不算数。
pub fn preview(screen: &vt100::Screen, n: usize) -> String {
    let (_, cols) = screen.size();
    let lines: Vec<String> = screen.rows(0, cols).collect();
    let picked: Vec<&str> = lines.iter().filter_map(|l| meaningful(l)).collect();
    let start = picked.len().saturating_sub(n);
    picked[start..].join("\n")
}

/// Does any visible row contain `needle`? Used by the answer driver to
/// confirm a dialog is gone after the keystrokes went in.
pub fn screen_contains(screen: &vt100::Screen, needle: &str) -> bool {
    let (_, cols) = screen.size();
    screen.rows(0, cols).any(|r| r.contains(needle))
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
    fn preview_takes_last_lines() {
        let p = feed("l1\r\nl2\r\nl3\r\nl4\r\nl5\r\n\r\n\r\n");
        assert_eq!(preview(p.screen(), 4), "l2\nl3\nl4\nl5");
    }

    #[test]
    fn preview_skips_rules_and_borders() {
        // claude code 的分隔线/输入框边框剥完竖线只剩横线——曾被当成
        // preview 推进通知，用户收到一条横杠
        let p = feed("done thinking\r\n╭──────────╮\r\n│ > \r\n╰──────────╯\r\n");
        let pv = preview(p.screen(), 4);
        assert!(!pv.contains('─'), "preview 不应含横线: {pv:?}");
        assert!(pv.contains("done thinking"));
    }

    #[test]
    fn contains_scans_visible_rows() {
        let p = feed("Pick a color\r\n❯ 1. Red\r\nEnter to select · Esc to cancel\r\n");
        assert!(screen_contains(p.screen(), "Enter to select"));
        assert!(!screen_contains(p.screen(), "Ready to submit"));
    }
}
