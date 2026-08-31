//! 终端 VT 模型：alacritty_terminal `Term` 喂 WS 字节；
//! 键盘 → 控制序列编码；粘贴（bracketed paste）；IME 文本直通。
//! 渲染在 `ui::terminal_view`（自绘，不使用任何 GPL 代码）。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::Processor;

use crate::theme;

/// 收集 term 想写回 PTY 的应答（DSR/DA/颜色查询等），UI 层负责转发到 WS
#[derive(Clone, Default)]
pub struct ProxyListener {
    pty_writes: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl EventListener for ProxyListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                self.pty_writes.lock().unwrap().push_back(text.into_bytes());
            }
            Event::ColorRequest(idx, format) => {
                let rgb = color_for_index(idx);
                self.pty_writes
                    .lock()
                    .unwrap()
                    .push_back(format(rgb).into_bytes());
            }
            _ => {}
        }
    }
}

fn color_for_index(idx: usize) -> alacritty_terminal::vte::ansi::Rgb {
    let raw = match idx {
        0..=255 => theme::indexed_color(idx as u8),
        256 => theme::TERM_FG,
        257 => theme::TERM_BG,
        258 => theme::CYAN, // cursor
        _ => theme::TERM_FG,
    };
    alacritty_terminal::vte::ansi::Rgb {
        r: (raw >> 16) as u8,
        g: (raw >> 8) as u8,
        b: raw as u8,
    }
}

pub struct TermModel {
    pub term: Term<ProxyListener>,
    parser: Processor,
    listener: ProxyListener,
    pub cols: u16,
    pub rows: u16,
}

impl TermModel {
    pub fn new(cols: u16, rows: u16) -> Self {
        let listener = ProxyListener::default();
        let cols = cols.max(2);
        let rows = rows.max(2);
        let size = TermSize::new(cols as usize, rows as usize);
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        TermModel {
            term: Term::new(config, &size, listener.clone()),
            parser: Processor::new(),
            listener,
            cols,
            rows,
        }
    }

    /// 喂 PTY 输出字节；返回 term 产生的待写回 PTY 的应答字节
    pub fn advance(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.parser.advance(&mut self.term, bytes);
        let mut q = self.listener.pty_writes.lock().unwrap();
        q.drain(..).collect()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(2);
        let rows = rows.max(2);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term
            .resize(TermSize::new(cols as usize, rows as usize));
    }

    /// 滚动回滚缓冲：delta > 0 向上（看历史）
    pub fn scroll_display(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_len(&self) -> usize {
        self.term.grid().total_lines() - self.term.grid().screen_lines()
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }
}

// ── 键盘编码 ────────────────────────────────────────────────────────────────

/// 与 gpui 解耦的按键描述（gpui `Keystroke` 在 UI 层转换过来），便于单元测试
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyInput<'a> {
    pub key: &'a str, // gpui 键名：小写字母/数字/"enter"/"up"/…
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// xterm 修饰键参数：1 + shift(1) + alt(2) + ctrl(4)
fn mod_code(k: &KeyInput) -> u8 {
    1 + (k.shift as u8) + ((k.alt as u8) << 1) + ((k.ctrl as u8) << 2)
}

fn csi_mod(k: &KeyInput, ch: char) -> Vec<u8> {
    if mod_code(k) == 1 {
        format!("\x1b[{ch}").into_bytes()
    } else {
        format!("\x1b[1;{}{ch}", mod_code(k)).into_bytes()
    }
}

fn tilde_seq(k: &KeyInput, num: u8) -> Vec<u8> {
    if mod_code(k) == 1 {
        format!("\x1b[{num}~").into_bytes()
    } else {
        format!("\x1b[{num};{}~", mod_code(k)).into_bytes()
    }
}

/// 控制键/组合键 → 字节序列。
/// 返回 None 表示这不是终端要处理的控制键（可打印字符走 IME 输入通道）。
pub fn encode_key(k: KeyInput, app_cursor: bool) -> Option<Vec<u8>> {
    // cmd 组合不进终端（App 快捷键），由调用方过滤
    let out = match k.key {
        "enter" => {
            if k.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "tab" => {
            if k.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "escape" => b"\x1b".to_vec(),
        "backspace" => {
            if k.alt {
                b"\x1b\x7f".to_vec()
            } else if k.ctrl {
                b"\x08".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        "delete" => tilde_seq(&k, 3),
        "insert" => tilde_seq(&k, 2),
        "pageup" => tilde_seq(&k, 5),
        "pagedown" => tilde_seq(&k, 6),
        "up" | "down" | "right" | "left" | "home" | "end" => {
            let ch = match k.key {
                "up" => 'A',
                "down" => 'B',
                "right" => 'C',
                "left" => 'D',
                "home" => 'H',
                _ => 'F',
            };
            if app_cursor && mod_code(&k) == 1 {
                format!("\x1bO{ch}").into_bytes()
            } else {
                csi_mod(&k, ch)
            }
        }
        "f1" | "f2" | "f3" | "f4" => {
            let ch = ['P', 'Q', 'R', 'S'][(k.key.as_bytes()[1] - b'1') as usize];
            if mod_code(&k) == 1 {
                format!("\x1bO{ch}").into_bytes()
            } else {
                csi_mod(&k, ch)
            }
        }
        "f5" => tilde_seq(&k, 15),
        "f6" => tilde_seq(&k, 17),
        "f7" => tilde_seq(&k, 18),
        "f8" => tilde_seq(&k, 19),
        "f9" => tilde_seq(&k, 20),
        "f10" => tilde_seq(&k, 21),
        "f11" => tilde_seq(&k, 23),
        "f12" => tilde_seq(&k, 24),
        "space" if k.ctrl => vec![0x00],
        "space" if k.alt => b"\x1b ".to_vec(),
        key if key.len() == 1 => {
            let c = key.as_bytes()[0];
            if k.ctrl {
                let byte = match c {
                    b'a'..=b'z' => c & 0x1f,
                    b'@' | b'2' => 0x00,
                    b'[' => 0x1b,
                    b'\\' => 0x1c,
                    b']' => 0x1d,
                    b'^' | b'6' => 0x1e,
                    b'_' | b'-' | b'/' => 0x1f,
                    b'?' => 0x7f,
                    _ => return None,
                };
                let mut v = Vec::new();
                if k.alt {
                    v.push(0x1b);
                }
                v.push(byte);
                v
            } else if k.alt {
                // meta 风格：ESC 前缀（alt-b / alt-f 词移动等）
                let ch = if k.shift {
                    key.to_uppercase()
                } else {
                    key.to_string()
                };
                let mut v = vec![0x1b];
                v.extend_from_slice(ch.as_bytes());
                v
            } else {
                return None; // 普通可打印字符走 IME 通道
            }
        }
        _ => return None,
    };
    Some(out)
}

/// 粘贴编码：bracketed paste 模式包裹，防止粘贴内容被当成按键
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    // 终端粘贴约定：\n → \r
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(normalized.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: &str) -> KeyInput<'_> {
        KeyInput {
            key,
            ..Default::default()
        }
    }

    #[test]
    fn basic_keys() {
        assert_eq!(encode_key(k("enter"), false).unwrap(), b"\r");
        assert_eq!(encode_key(k("tab"), false).unwrap(), b"\t");
        assert_eq!(encode_key(k("escape"), false).unwrap(), b"\x1b");
        assert_eq!(encode_key(k("backspace"), false).unwrap(), b"\x7f");
        assert_eq!(encode_key(k("delete"), false).unwrap(), b"\x1b[3~");
    }

    #[test]
    fn arrows_normal_and_app_cursor() {
        assert_eq!(encode_key(k("up"), false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(k("left"), false).unwrap(), b"\x1b[D");
        assert_eq!(encode_key(k("up"), true).unwrap(), b"\x1bOA");
        // 带修饰键时 app_cursor 也走 CSI 1;m
        let ctrl_up = KeyInput {
            key: "up",
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(encode_key(ctrl_up, true).unwrap(), b"\x1b[1;5A");
        let alt_right = KeyInput {
            key: "right",
            alt: true,
            ..Default::default()
        };
        assert_eq!(encode_key(alt_right, false).unwrap(), b"\x1b[1;3C");
    }

    #[test]
    fn ctrl_letters() {
        let ctrl = |key| KeyInput {
            key,
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(encode_key(ctrl("c"), false).unwrap(), vec![0x03]);
        assert_eq!(encode_key(ctrl("a"), false).unwrap(), vec![0x01]);
        assert_eq!(encode_key(ctrl("z"), false).unwrap(), vec![0x1a]);
        assert_eq!(encode_key(ctrl("["), false).unwrap(), vec![0x1b]);
        assert_eq!(encode_key(ctrl("space"), false).unwrap(), vec![0x00]);
    }

    #[test]
    fn alt_meta_prefix() {
        let alt = |key| KeyInput {
            key,
            alt: true,
            ..Default::default()
        };
        assert_eq!(encode_key(alt("b"), false).unwrap(), b"\x1bb");
        assert_eq!(encode_key(alt("f"), false).unwrap(), b"\x1bf");
        assert_eq!(
            encode_key(
                KeyInput {
                    key: "b",
                    alt: true,
                    shift: true,
                    ..Default::default()
                },
                false
            )
            .unwrap(),
            b"\x1bB"
        );
    }

    #[test]
    fn printable_passthrough() {
        // 普通字符不在这里编码（IME 通道负责）
        assert!(encode_key(k("a"), false).is_none());
        assert!(encode_key(k("1"), false).is_none());
    }

    #[test]
    fn shift_tab_and_fkeys() {
        let shift_tab = KeyInput {
            key: "tab",
            shift: true,
            ..Default::default()
        };
        assert_eq!(encode_key(shift_tab, false).unwrap(), b"\x1b[Z");
        assert_eq!(encode_key(k("f1"), false).unwrap(), b"\x1bOP");
        assert_eq!(encode_key(k("f5"), false).unwrap(), b"\x1b[15~");
        assert_eq!(encode_key(k("f12"), false).unwrap(), b"\x1b[24~");
    }

    #[test]
    fn paste_bracketed() {
        assert_eq!(
            encode_paste("ls\npwd\n", true),
            b"\x1b[200~ls\rpwd\r\x1b[201~".to_vec()
        );
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb".to_vec());
    }

    #[test]
    fn term_model_feeds_and_renders() {
        let mut tm = TermModel::new(20, 5);
        tm.advance(b"hello \x1b[31mred\x1b[0m");
        let content = tm.term.renderable_content();
        let mut text = String::new();
        let mut saw_red = false;
        for cell in content.display_iter {
            if cell.point.line.0 == 0 {
                text.push(cell.c);
                if cell.c == 'r' {
                    use alacritty_terminal::vte::ansi::{Color, NamedColor};
                    saw_red = matches!(cell.fg, Color::Named(NamedColor::Red));
                }
            }
        }
        assert!(text.starts_with("hello red"), "line0: {text:?}");
        assert!(saw_red, "SGR 31 前景应为红");
    }

    #[test]
    fn term_model_resize_and_scrollback() {
        let mut tm = TermModel::new(10, 4);
        for i in 0..20 {
            tm.advance(format!("line{i}\r\n").as_bytes());
        }
        assert!(tm.history_len() > 0, "应有回滚缓冲");
        assert_eq!(tm.display_offset(), 0);
        tm.scroll_display(5);
        assert_eq!(tm.display_offset(), 5);
        tm.scroll_to_bottom();
        assert_eq!(tm.display_offset(), 0);
        tm.resize(20, 10);
        assert_eq!((tm.cols, tm.rows), (20, 10));
    }

    #[test]
    fn term_answers_dsr() {
        // ESC[6n 光标位置查询 → term 应产生写回 PTY 的应答
        let mut tm = TermModel::new(10, 4);
        let answers = tm.advance(b"\x1b[6n");
        assert!(!answers.is_empty(), "DSR 应有应答");
        let ans = String::from_utf8_lossy(&answers[0]).to_string();
        assert!(ans.starts_with("\x1b["), "应答应是 CSI: {ans:?}");
        assert!(ans.ends_with('R'));
    }

    #[test]
    fn selection_to_string_roundtrip() {
        use alacritty_terminal::index::{Column, Line, Point, Side};
        use alacritty_terminal::selection::{Selection, SelectionType};
        let mut tm = TermModel::new(20, 5);
        tm.advance(b"hello world");
        let mut sel = Selection::new(
            SelectionType::Simple,
            Point::new(Line(0), Column(0)),
            Side::Left,
        );
        sel.update(Point::new(Line(0), Column(4)), Side::Right);
        tm.term.selection = Some(sel);
        assert_eq!(tm.term.selection_to_string().as_deref(), Some("hello"));
        // 语义选择（双击词选）
        let sel = Selection::new(
            SelectionType::Semantic,
            Point::new(Line(0), Column(7)),
            Side::Left,
        );
        tm.term.selection = Some(sel);
        assert_eq!(tm.term.selection_to_string().as_deref(), Some("world"));
        // 输入清选区语义由 TerminalView::send_input 保证（此处仅验证 None 情况）
        tm.term.selection = None;
        assert_eq!(tm.term.selection_to_string(), None);
    }

    #[test]
    fn alt_screen_mode_flag() {
        let mut tm = TermModel::new(10, 4);
        assert!(!tm.mode().contains(TermMode::ALT_SCREEN));
        tm.advance(b"\x1b[?1049h");
        assert!(tm.mode().contains(TermMode::ALT_SCREEN));
    }
}
