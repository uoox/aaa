//! Raw-mode terminal plumbing for the `aaa` CLI.
//!
//! No crossterm/termion dependency: the CLI needs exactly four things — raw
//! mode, a key reader that can time out (so the menu refreshes itself while
//! idle), the window size, and a poll across stdin plus one socket. That is a
//! page of libc, and it keeps `aaa` linking nothing the daemon does not
//! already link.

use std::io::Write;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const CYAN: &str = "\x1b[36m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const RED: &str = "\x1b[31m";
pub const MAGENTA: &str = "\x1b[35m";
pub const GREY: &str = "\x1b[90m";
/// erase to end of line — every menu row ends with this so a redraw over a
/// longer previous frame leaves no debris (the menu never clears the screen
/// after the first frame, which is what keeps it flicker-free).
pub const EOL: &str = "\x1b[K";

pub fn clear() {
    print!("\x1b[2J\x1b[H");
    let _ = std::io::stdout().flush();
}

pub fn home() {
    print!("\x1b[H");
}

/// Raw mode with a saved terminal state, restored on drop (including on
/// panic — the guard lives on the stack, so unwinding runs it).
pub struct Raw {
    saved: libc::termios,
}

impl Raw {
    /// `post_output` keeps OPOST/ONLCR on, so plain `\n` in our own rendering
    /// still returns the cursor to column 0. The attach loop turns it off: a
    /// remote PTY has already applied its own output processing, and a second
    /// pass would corrupt cursor-positioning sequences.
    pub fn enter(post_output: bool) -> Option<Raw> {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) != 0 {
                return None;
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            if post_output {
                t.c_oflag |= libc::OPOST | libc::ONLCR;
            }
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) != 0 {
                return None;
            }
            Some(Raw { saved })
        }
    }

    /// Run `f` with the terminal back in cooked mode. Line editing and IME
    /// composition (Chinese project names) both need the real line
    /// discipline, so every text prompt goes through here.
    pub fn cooked<T>(&self, f: impl FnOnce() -> T) -> T {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.saved);
        }
        let out = f();
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut t) == 0 {
                libc::cfmakeraw(&mut t);
                t.c_oflag |= libc::OPOST | libc::ONLCR;
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t);
            }
        }
        out
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.saved);
        }
        print!("{RESET}");
        let _ = std::io::stdout().flush();
    }
}

/// (cols, rows); falls back to the daemon's default geometry when stdout is
/// not a tty (piped output still needs a sane width for truncation).
pub fn size() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return (ws.ws_col, ws.ws_row.max(10));
        }
    }
    (120, 40)
}

pub fn is_tty() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 && libc::isatty(libc::STDOUT_FILENO) == 1 }
}

/// Wait for stdin (and optionally one extra fd) to become readable.
/// Returns `(stdin_ready, other_ready)`.
pub fn poll(other: Option<i32>, timeout_ms: i32) -> (bool, bool) {
    let mut fds = [
        libc::pollfd { fd: libc::STDIN_FILENO, events: libc::POLLIN, revents: 0 },
        libc::pollfd { fd: other.unwrap_or(-1), events: libc::POLLIN, revents: 0 },
    ];
    let n = if other.is_some() { 2 } else { 1 };
    let rc = unsafe { libc::poll(fds.as_mut_ptr(), n, timeout_ms) };
    if rc <= 0 {
        return (false, false);
    }
    (fds[0].revents != 0, n == 2 && fds[1].revents != 0)
}

/// 裸 read(2)，绝不能走 `std::io::stdin()`：它带 8KB BufReader，读 ESC 那
/// 一下会把后面的 `[B` 一起吸进用户态缓冲，随后 `poll` 看 fd 上没数据，
/// CSI 序列就被判成裸 Esc——方向键按一下菜单直接退出（实测坑）。
pub fn read_stdin(buf: &mut [u8]) -> usize {
    unsafe {
        let n = libc::read(
            libc::STDIN_FILENO,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        );
        if n < 0 { 0 } else { n as usize }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Char(char),
    Ctrl(char),
    Other,
}

/// One key press, or `None` on timeout. `timeout_ms < 0` blocks.
pub fn read_key(timeout_ms: i32) -> Option<Key> {
    let (ready, _) = poll(None, timeout_ms);
    if !ready {
        return None;
    }
    let mut b = [0u8; 1];
    if read_stdin(&mut b) == 0 {
        return Some(Key::Esc);
    }
    Some(match b[0] {
        0x1b => {
            // Distinguish a bare Esc from a CSI sequence by whether more
            // bytes land within a beat — the same trick the zsh menu used.
            let (ready, _) = poll(None, 40);
            if !ready {
                return Some(Key::Esc);
            }
            let mut c = [0u8; 1];
            if read_stdin(&mut c) == 0 || (c[0] != b'[' && c[0] != b'O') {
                return Some(Key::Esc);
            }
            let mut d = [0u8; 1];
            if read_stdin(&mut d) == 0 {
                return Some(Key::Esc);
            }
            let k = match d[0] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                _ => Key::Other,
            };
            // swallow any trailing parameter bytes (e.g. Delete's "3~")
            while d[0].is_ascii_digit() || d[0] == b';' {
                let (ready, _) = poll(None, 20);
                if !ready || read_stdin(&mut d) == 0 {
                    break;
                }
                if d[0] == b'~' {
                    break;
                }
            }
            k
        }
        b'\r' | b'\n' => Key::Enter,
        0x7f | 0x08 => Key::Backspace,
        c @ 0x01..=0x1a => Key::Ctrl((b'a' + c - 1) as char),
        c if c.is_ascii_graphic() || c == b' ' => Key::Char(c as char),
        _ => Key::Other,
    })
}

/// Display width in terminal cells: CJK and full-width forms take two.
pub fn width(s: &str) -> usize {
    s.chars()
        .map(|c| {
            let u = c as u32;
            let wide = (0x1100..=0x115F).contains(&u)
                || (0x2E80..=0xA4CF).contains(&u)
                || (0xAC00..=0xD7A3).contains(&u)
                || (0xF900..=0xFAFF).contains(&u)
                || (0xFE30..=0xFE6F).contains(&u)
                || (0xFF00..=0xFF60).contains(&u)
                || (0xFFE0..=0xFFE6).contains(&u)
                || (0x1F300..=0x1FAFF).contains(&u);
            if u < 0x20 {
                0
            } else if wide {
                2
            } else {
                1
            }
        })
        .sum()
}

/// Truncate to `w` cells, ending with `…` when anything was cut.
pub fn truncate(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = width(&c.to_string());
        if used + cw > w.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// Truncate then pad to exactly `w` cells — the whole table layout rests on
/// this, so it must count cells rather than chars.
pub fn cell(s: &str, w: usize) -> String {
    let t = truncate(s, w);
    let pad = w.saturating_sub(width(&t));
    format!("{t}{}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn width_counts_cjk_as_two() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("邮件系统"), 8);
        assert_eq!(width("aaa-ui"), 6);
        assert_eq!(width("小卖部a"), 7);
    }

    #[test]
    fn cell_pads_to_exact_cells_not_chars() {
        assert_eq!(width(&cell("邮件系统", 12)), 12);
        assert_eq!(width(&cell("abc", 12)), 12);
        // over-long input is cut *and* still exactly the requested width
        assert_eq!(width(&cell("邮件系统邮件系统", 9)), 9);
    }

    #[test]
    fn truncate_marks_elision() {
        assert_eq!(truncate("abcdef", 6), "abcdef");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert!(truncate("邮件系统", 5).ends_with('…'));
    }
}
