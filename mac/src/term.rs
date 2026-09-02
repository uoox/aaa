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

    /// 滚动到指定回看深度（0 = 底部；scrollbar 拖拽用）
    pub fn scroll_to(&mut self, offset: usize) {
        let delta = offset as i64 - self.display_offset() as i64;
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta as i32));
        }
    }
}

/// 滚轮 → 鼠标上报序列。TUI（claude code 等）开了鼠标上报（1000/1002/1003）
/// 时，滚轮必须按鼠标协议转发、由应用自己滚内容——真终端（iTerm 等）里 TUI
/// 能滚就是靠这个；备用屏没有回滚缓冲，滚视口在那里是死路。
/// 返回 None = 应用没开鼠标上报（调用方自行决定滚视口或转 ↑↓）。
pub fn encode_wheel(mode: TermMode, up: bool, col: u16, row: u16) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let btn: u16 = if up { 64 } else { 65 };
    Some(mouse_report(mode, btn, false, col, row))
}

/// 鼠标事件的 Cb 字段：按钮号 + 修饰键位（shift 4 / alt 8 / ctrl 16），
/// 移动事件再加 32。xterm 约定，SGR 与 X10 两种编码共用。
fn mouse_cb(button: u8, motion: bool, (shift, alt, ctrl): (bool, bool, bool)) -> u16 {
    button as u16
        + if shift { 4 } else { 0 }
        + if alt { 8 } else { 0 }
        + if ctrl { 16 } else { 0 }
        + if motion { 32 } else { 0 }
}

/// 把一条鼠标事件按当前模式编码。SGR（1006）：`ESC [ < Cb ; Cx ; Cy M`，
/// 松开用小写 `m`、按钮号保留；坐标 1 起、不限长。
/// 传统 X10：`ESC [ M` + 三个字节（32 + 值），松开不分哪个键统一按钮 3
/// （低两位），修饰键位保留；坐标上限 223（单字节放不下更大的）。
fn mouse_report(mode: TermMode, cb: u16, release: bool, col: u16, row: u16) -> Vec<u8> {
    if mode.contains(TermMode::SGR_MOUSE) {
        let fin = if release { 'm' } else { 'M' };
        format!("\x1b[<{};{};{}{}", cb, col + 1, row + 1, fin).into_bytes()
    } else {
        let cb = if release { (cb & !0b11) | 3 } else { cb };
        vec![
            0x1b,
            b'[',
            b'M',
            (32 + cb) as u8,
            32 + (col + 1).min(223) as u8,
            32 + (row + 1).min(223) as u8,
        ]
    }
}

/// 鼠标按下/松开 → 上报序列。`button`：0 左 / 1 中 / 2 右；`modifiers`
/// 按 (shift, alt, ctrl) 给。TUI（claude code）开了鼠标上报后点选项、点焦点
/// 全靠这个——真终端里点击能用就是因为它把点击转成了这串字节。
/// 返回 None = 应用没开鼠标上报（调用方按本地选区处理点击）。
pub fn encode_mouse_button(
    mode: TermMode,
    button: u8,
    pressed: bool,
    col: u16,
    row: u16,
    modifiers: (bool, bool, bool),
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let cb = mouse_cb(button, false, modifiers);
    Some(mouse_report(mode, cb, !pressed, col, row))
}

/// 鼠标移动 → 上报序列。`button` 是按住的键（0/1/2），3 = 没按键。
/// 1002（按键拖动）只在按住时上报；1003（任意移动）没按键也报（按钮号 3）。
/// 只开 1000 不报移动。调用方自行按格点去重，别每个像素都发一条。
pub fn encode_mouse_motion(
    mode: TermMode,
    button: u8,
    col: u16,
    row: u16,
    modifiers: (bool, bool, bool),
) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let held = button < 3;
    let wanted =
        mode.contains(TermMode::MOUSE_MOTION) || (mode.contains(TermMode::MOUSE_DRAG) && held);
    if !wanted {
        return None;
    }
    let cb = mouse_cb(button, true, modifiers);
    Some(mouse_report(mode, cb, false, col, row))
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

// ── 链接识别 ────────────────────────────────────────────────────────────────

/// 一行文本里的一个链接：字符下标 [start, end)，`url` 已剥掉尾部标点。
#[derive(Debug, Clone, PartialEq)]
pub struct UrlSpan {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// 只认带 scheme 的绝对地址。刻意比「看着像域名就算」保守：终端里满屏都是
/// main.rs、Cargo.toml、a.b.c 这种路径与包名，裸域名规则会把它们统统变成点不开
/// 的假链接，比不识别还难用。规则与 Android 端 `Links.kt` 对齐。
const SCHEMES: [&str; 4] = ["http://", "https://", "ftp://", "file://"];

/// 句末标点：URL 出现在中英文句子里时它们几乎不可能是地址的一部分。
const TRAILING_PUNCT: &str = ".,;:!?'\"“”‘’、。，；：！？…";

fn starts_with_ci(hay: &[char], needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(k, nc)| hay.get(k).is_some_and(|hc| hc.to_ascii_lowercase() == nc))
}

/// scheme 前必须是分隔符，否则 `xhttp://` 这种半截也会被认成链接
fn is_boundary(c: char) -> bool {
    !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn is_terminator(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '<' | '>' | '"' | '\'' | '`' | '\\' | '^' | '{' | '}' | '|')
}

/// 剥尾：句末标点直接去掉；成对括号只在不配平时才剥，
/// 这样 Wikipedia 那种 `.../Foo_(bar)` 的地址不会被砍掉半截。
fn trim_url_tail(raw: &str) -> String {
    let mut chars: Vec<char> = raw.chars().collect();
    while let Some(&last) = chars.last() {
        if TRAILING_PUNCT.contains(last) {
            chars.pop();
            continue;
        }
        let open = match last {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            _ => break,
        };
        let opens = chars.iter().filter(|&&c| c == open).count();
        let closes = chars.iter().filter(|&&c| c == last).count();
        if opens < closes {
            chars.pop();
        } else {
            break;
        }
    }
    chars.into_iter().collect()
}

/// 扫出一行文本里的所有链接。下标按**字符**计（调用方要把它换算成终端列，
/// 一个 CJK 字符占两格，下标推不出列号）。
pub fn find_urls(text: &str) -> Vec<UrlSpan> {
    // 整屏逐帧扫描，先用一次子串判断挡掉绝大多数行
    if !text.contains("://") {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let Some(scheme) = SCHEMES.iter().find(|s| starts_with_ci(&chars[i..], s)) else {
            i += 1;
            continue;
        };
        if i > 0 && !is_boundary(chars[i - 1]) {
            i += 1;
            continue;
        }
        let mut end = i + scheme.chars().count();
        while end < chars.len() && !is_terminator(chars[end]) {
            end += 1;
        }
        let url = trim_url_tail(&chars[i..end].iter().collect::<String>());
        let n = url.chars().count();
        // scheme 后面空无一物的不算地址
        if n > scheme.chars().count() {
            out.push(UrlSpan {
                start: i,
                end: i + n,
                url,
            });
        }
        i = end.max(i + 1);
    }
    out
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
    fn finds_schemed_urls_only() {
        let f = |t: &str| find_urls(t).into_iter().map(|s| s.url).collect::<Vec<_>>();
        assert_eq!(f("see https://example.com/a?b=1 ok"), ["https://example.com/a?b=1"]);
        assert_eq!(f("http://127.0.0.1:5173"), ["http://127.0.0.1:5173"]);
        assert_eq!(f("file:///tmp/x.log"), ["file:///tmp/x.log"]);
        // 终端里的常客：路径、包名、裸域名一律不认
        assert!(f("src/ui/mod.rs:475 cargo build").is_empty());
        assert!(f("example.com www.example.com").is_empty());
        assert!(f("Cargo.toml a.b.c").is_empty());
        // scheme 粘在别的词后面不算
        assert!(f("xhttps://a.com").is_empty());
        // scheme 后面什么都没有不算
        assert!(f("https://").is_empty());
    }

    #[test]
    fn trims_sentence_punctuation_but_keeps_balanced_parens() {
        let f = |t: &str| find_urls(t).into_iter().map(|s| s.url).collect::<Vec<_>>();
        assert_eq!(f("详见 https://example.com/a。"), ["https://example.com/a"]);
        assert_eq!(f("see https://example.com/a."), ["https://example.com/a"]);
        assert_eq!(f("(https://example.com/a)"), ["https://example.com/a"]);
        // 地址自带的成对括号要保住
        assert_eq!(
            f("https://w.org/Foo_(bar)"),
            ["https://w.org/Foo_(bar)"]
        );
        assert_eq!(f("[https://a.io/x]"), ["https://a.io/x"]);
    }

    #[test]
    fn url_span_indices_are_char_based() {
        // 前面是 CJK：start/end 必须按字符数，不能是字节数
        let spans = find_urls("打开 https://a.io 看看");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 3);
        assert_eq!(spans[0].end, 3 + "https://a.io".chars().count());
        // 一行两个链接
        let spans = find_urls("a http://x.io b https://y.io c");
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].url, "https://y.io");
    }

    #[test]
    fn alt_screen_mode_flag() {
        let mut tm = TermModel::new(10, 4);
        assert!(!tm.mode().contains(TermMode::ALT_SCREEN));
        tm.advance(b"\x1b[?1049h");
        assert!(tm.mode().contains(TermMode::ALT_SCREEN));
    }

    #[test]
    fn wheel_reporting_follows_claude_code_modes() {
        let mut tm = TermModel::new(10, 4);
        // 未开鼠标上报：不产生序列（调用方滚视口）
        assert!(encode_wheel(tm.mode(), true, 0, 0).is_none());
        // claude code 实测开的组合：1049 + 1000 + 1006（SGR）
        tm.advance(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            encode_wheel(tm.mode(), true, 4, 2).unwrap(),
            b"\x1b[<64;5;3M".to_vec(),
            "SGR 上滚，坐标 1 起"
        );
        assert_eq!(
            encode_wheel(tm.mode(), false, 0, 0).unwrap(),
            b"\x1b[<65;1;1M".to_vec()
        );
        // 只开 1000 不开 1006：X10 编码（32+btn, 32+coord）
        let mut tm = TermModel::new(10, 4);
        tm.advance(b"\x1b[?1000h");
        assert_eq!(
            encode_wheel(tm.mode(), true, 0, 0).unwrap(),
            vec![0x1b, b'[', b'M', 96, 33, 33]
        );
    }

    #[test]
    fn click_reporting_follows_claude_code_modes() {
        let none = (false, false, false);
        let mut tm = TermModel::new(10, 4);
        // 未开鼠标上报：点击归本地选区，不产生序列
        assert!(encode_mouse_button(tm.mode(), 0, true, 0, 0, none).is_none());
        assert!(encode_mouse_motion(tm.mode(), 0, 0, 0, none).is_none());
        // claude code 实测开的组合：1049 + 1000 + 1006（SGR）
        tm.advance(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h");
        assert_eq!(
            encode_mouse_button(tm.mode(), 0, true, 4, 2, none).unwrap(),
            b"\x1b[<0;5;3M".to_vec(),
            "SGR 左键按下，坐标 1 起"
        );
        assert_eq!(
            encode_mouse_button(tm.mode(), 0, false, 4, 2, none).unwrap(),
            b"\x1b[<0;5;3m".to_vec(),
            "SGR 松开用小写 m，按钮号保留"
        );
        assert_eq!(
            encode_mouse_button(tm.mode(), 2, true, 0, 0, none).unwrap(),
            b"\x1b[<2;1;1M".to_vec()
        );
        // 修饰键位：shift 4 / alt 8 / ctrl 16
        assert_eq!(
            encode_mouse_button(tm.mode(), 0, true, 0, 0, (true, false, true)).unwrap(),
            b"\x1b[<20;1;1M".to_vec()
        );
        assert_eq!(
            encode_mouse_button(tm.mode(), 1, true, 0, 0, (false, true, false)).unwrap(),
            b"\x1b[<9;1;1M".to_vec()
        );
        // 只开 1000：拖动不上报
        assert!(encode_mouse_motion(tm.mode(), 0, 4, 2, none).is_none());
        // 对话框里再开 1002：按住拖动上报（按钮 +32），没按键的移动仍不报
        tm.advance(b"\x1b[?1002h");
        assert_eq!(
            encode_mouse_motion(tm.mode(), 0, 4, 2, none).unwrap(),
            b"\x1b[<32;5;3M".to_vec()
        );
        assert!(
            encode_mouse_motion(tm.mode(), 3, 4, 2, none).is_none(),
            "1002 只报按住时的移动"
        );
        // 1003：任何移动都报，没按键时按钮号 3
        tm.advance(b"\x1b[?1003h");
        assert_eq!(
            encode_mouse_motion(tm.mode(), 3, 0, 0, none).unwrap(),
            b"\x1b[<35;1;1M".to_vec()
        );
        // 应用关掉鼠标上报后立刻恢复 None（松开时模式可能已经变了）
        tm.advance(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l");
        assert!(encode_mouse_button(tm.mode(), 0, false, 0, 0, none).is_none());
    }

    #[test]
    fn click_reporting_x10_encoding() {
        let none = (false, false, false);
        let mut tm = TermModel::new(10, 4);
        tm.advance(b"\x1b[?1000h");
        // 按下：32+按钮, 32+col+1, 32+row+1
        assert_eq!(
            encode_mouse_button(tm.mode(), 0, true, 0, 0, none).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
        // 松开不分哪个键，统一按钮 3；修饰键位保留
        assert_eq!(
            encode_mouse_button(tm.mode(), 0, false, 0, 0, none).unwrap(),
            vec![0x1b, b'[', b'M', 35, 33, 33]
        );
        assert_eq!(
            encode_mouse_button(tm.mode(), 2, false, 0, 0, (false, false, true)).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3 + 16, 33, 33]
        );
        // 坐标上限 223（单字节编码放不下更大的）
        assert_eq!(
            &encode_mouse_button(tm.mode(), 0, true, 500, 300, none).unwrap()[3..],
            &[32u8, 255, 255]
        );
        // 1002 拖动：32 + 0 + 32
        tm.advance(b"\x1b[?1002h");
        assert_eq!(
            encode_mouse_motion(tm.mode(), 0, 1, 1, none).unwrap(),
            vec![0x1b, b'[', b'M', 64, 34, 34]
        );
    }

    #[test]
    fn scroll_to_is_absolute() {
        let mut tm = TermModel::new(10, 4);
        for i in 0..40 {
            tm.advance(format!("line {i}\r\n").as_bytes());
        }
        assert!(tm.history_len() >= 30);
        tm.scroll_to(7);
        assert_eq!(tm.display_offset(), 7);
        tm.scroll_to(2);
        assert_eq!(tm.display_offset(), 2);
        tm.scroll_to(0);
        assert_eq!(tm.display_offset(), 0);
    }
}
