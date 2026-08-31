//! Row formatting shared by the menu and the non-interactive listings.
//!
//! Colours match the design tokens in PROTOCOL.md, so a session that is amber
//! on the phone is amber here too.

use crate::client::{Project, Session};
use crate::tty::{self, BOLD, CYAN, DIM, GREEN, GREY, RED, RESET, YELLOW};

pub fn state_color(state: &str) -> &'static str {
    match state {
        "running" => GREEN,
        "waiting" => YELLOW,
        "exited" => RED,
        _ => GREY,
    }
}

/// A non-zero exit code is the difference between "finished" and "died", so
/// it belongs in the one column that reports how a session ended.
pub fn state_label(s: &Session) -> String {
    match s.state.as_str() {
        "running" => "运行中".into(),
        "waiting" => "等待输入".into(),
        "idle" => "空闲".into(),
        "exited" => match s.exit_code {
            Some(0) | None => "已退出".into(),
            Some(n) => format!("退出 {n}"),
        },
        other => other.into(),
    }
}

pub fn agent_color(agent: &str) -> &'static str {
    match agent {
        "claude" => "\x1b[38;5;179m",
        "codex" => "\x1b[38;5;111m",
        "pi" => "\x1b[38;5;150m",
        "reasonix" => "\x1b[38;5;175m",
        "agy" => "\x1b[38;5;140m",
        _ => GREY,
    }
}

/// Permission status → (colour, label). `needs_settings` is its own state on
/// purpose: full disk access has no API that can prompt, so the only honest
/// thing to say is "go open the pane".
pub fn perm_status(status: &str) -> (&'static str, &'static str) {
    match status {
        "granted" => (GREEN, "已授权"),
        "denied" => (RED, "已拒绝"),
        "undetermined" => (YELLOW, "未申请"),
        "needs_settings" => (CYAN, "需手动开"),
        _ => (GREY, "未知"),
    }
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes}{}", U[0])
    } else if v < 10.0 {
        format!("{v:.1}{}", U[i])
    } else {
        format!("{v:.0}{}", U[i])
    }
}

/// "3秒前" / "12分" / "2小时" / "3天" — short enough for a table column.
pub fn ago(iso: &str) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else {
        return String::new();
    };
    let secs = (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 0 {
        return "刚刚".into();
    }
    match secs {
        0..=59 => format!("{secs}秒"),
        60..=3599 => format!("{}分", secs / 60),
        3600..=86399 => format!("{}小时", secs / 3600),
        _ => format!("{}天", secs / 86400),
    }
}

/// One session line for the menu and for `aaa ls`.
/// `width` is the terminal width; the headline takes whatever is left.
pub fn session_row(s: &Session, width: usize) -> String {
    let dot = format!("{}●{RESET}", state_color(&s.state));
    let name = format!("{}{}{RESET}", BOLD, tty::cell(&s.project_name, 14));
    let agent = format!("{}{}{RESET}", agent_color(&s.agent), tty::cell(&s.agent, 9));
    let state = format!("{}{}{RESET}", state_color(&s.state), tty::cell(&state_label(s), 9));
    let when = format!("{DIM}{}{RESET}", tty::cell(&ago(&s.last_output_at), 5));
    // 1 dot + 1 gap + 14 + 1 + 9 + 1 + 9 + 1 + 5 + 1
    let used = 43;
    let head = tty::truncate(s.headline(), width.saturating_sub(used).max(10));
    format!("{dot} {name} {agent} {state} {when} {DIM}{head}{RESET}")
}

/// One project line, columns kept in the order the aaa CLI used.
pub fn project_row(p: &Project, width: usize) -> String {
    let name = format!("{BOLD}{}{RESET}", tty::cell(&p.name, 18));
    let agent = format!("{}{}{RESET}", agent_color(&p.agent), tty::cell(&p.agent, 9));
    let size = tty::cell(&human_size(p.dir_size), 6);
    let ctx = match p.ctx_size {
        Some(c) if c > 0 => tty::cell(&human_size(c), 6),
        _ => tty::cell("-", 6),
    };
    let when = tty::cell(&ago(&p.mtime), 5);
    let used = 18 + 1 + 9 + 1 + 6 + 1 + 6 + 1 + 5 + 1;
    let title = p.session_title.clone().unwrap_or_default();
    let title = tty::truncate(&title, width.saturating_sub(used).max(10));
    format!("{name} {agent} {DIM}{size} {ctx} {when}{RESET} {CYAN}{title}{RESET}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_the_cli_did() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(39 * 1024 * 1024), "39M");
    }

    #[test]
    fn ago_buckets() {
        let now = chrono::Utc::now();
        let f = |d: chrono::Duration| ago(&(now - d).to_rfc3339());
        assert_eq!(f(chrono::Duration::seconds(5)), "5秒");
        assert_eq!(f(chrono::Duration::minutes(3)), "3分");
        assert_eq!(f(chrono::Duration::hours(5)), "5小时");
        assert_eq!(f(chrono::Duration::days(2)), "2天");
        assert_eq!(ago("not-a-date"), "", "坏时间戳不该炸，留空即可");
    }
}
