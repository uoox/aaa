//! 终端视图：自绘 gpui 元素遍历 alacritty `Term` grid。
//! 等宽 Menlo，前景/背景/粗体/斜体/下划线/光标块/回滚缓冲；
//! IME 走 EntityInputHandler（中文组字预览显示在光标处）；
//! 视图尺寸变化 → 行列重算 → WS resize 控制帧。
//! （独立实现，不含任何 Zed GPL terminal_view 代码。）

use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollDelta,
    ScrollWheelEvent, SharedString, UTF16Selection, Window, canvas, div, fill, point, prelude::*,
    px, size,
};

use super::kit::{c, ca};
use crate::net::{AttachHandle, Net};
use crate::term::{KeyInput, TermModel, encode_key, encode_paste};
use crate::theme;

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT_RATIO: f32 = 1.5;
const PAD: f32 = 8.0;

pub struct TerminalView {
    #[allow(dead_code)]
    pub session_id: String,
    pub model: TermModel,
    attach: AttachHandle,
    focus_handle: FocusHandle,
    marked: Option<String>,
    pub conn_down: bool,
    last_sent: Option<(u16, u16)>,
    cell: Option<(Pixels, Pixels)>, // (cell_w, line_h)
    scroll_accum: f32,
    /// canvas 内容区左上角（窗口坐标，prepaint 时回写；鼠标→格点换算用）
    last_origin: Option<gpui::Point<Pixels>>,
    selecting: bool,
}

// ── 渲染快照（render 时从 grid 提取，canvas 闭包里绘制） ────────────────────

struct Seg {
    col: u16,
    text: String,
    fg: u32,
    alpha: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct LineSnap {
    segs: Vec<Seg>,
    bgs: Vec<(u16, u16, u32)>, // [start_col, end_col) 背景
    sels: Vec<(u16, u16)>,     // [start_col, end_col) 选区高亮
}

struct Snap {
    lines: Vec<LineSnap>,
    cursor: Option<(u16, u16, char, CursorShape)>, // (viewport_row, col, ch, shape)
    display_offset: usize,
    history: usize,
}

impl TerminalView {
    pub fn new(session_id: String, net: &Net, cx: &mut Context<Self>) -> Self {
        let attach = net.attach(&session_id);
        TerminalView {
            session_id,
            model: TermModel::new(80, 24),
            attach,
            focus_handle: cx.focus_handle(),
            marked: None,
            conn_down: false,
            last_sent: None,
            cell: None,
            scroll_accum: 0.,
            last_origin: None,
            selecting: false,
        }
    }

    /// WS 二进制帧 → VT 模型；term 的应答字节写回 PTY
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.conn_down = false;
        for answer in self.model.advance(bytes) {
            self.attach.input(answer);
        }
        cx.notify();
    }

    /// hello 帧给的服务端尺寸（本地还未 resize 前先跟随）
    pub fn set_remote_size(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.last_sent.is_none() && cols > 0 && rows > 0 {
            self.model.resize(cols, rows);
            cx.notify();
        }
    }

    pub fn set_down(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.conn_down != down {
            self.conn_down = down;
            cx.notify();
        }
    }

    pub fn focus_handle_clone(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn apply_view_size(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.last_sent == Some((cols, rows)) {
            return;
        }
        self.last_sent = Some((cols, rows));
        self.model.resize(cols, rows);
        self.attach.resize(cols, rows);
        cx.notify();
    }

    fn send_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.model.scroll_to_bottom();
        self.model.term.selection = None;
        self.attach.input(bytes);
        cx.notify();
    }

    /// 窗口坐标 → 终端格点 + 半格方位（选区用）
    fn grid_point(&self, pos: gpui::Point<Pixels>) -> Option<(TermPoint, Side)> {
        let (cell_w, line_h) = self.cell?;
        let origin = self.last_origin?;
        let x = f32::from(pos.x) - f32::from(origin.x) - PAD;
        let y = f32::from(pos.y) - f32::from(origin.y) - PAD;
        let colf = (x / f32::from(cell_w)).clamp(0., (self.model.cols - 1) as f32);
        let row = (y / f32::from(line_h))
            .floor()
            .clamp(0., (self.model.rows - 1) as f32) as i32;
        let line = row - self.model.display_offset() as i32;
        let side = if colf.fract() < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        Some((TermPoint::new(Line(line), Column(colf as usize)), side))
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        if let Some((p, side)) = self.grid_point(ev.position) {
            if ev.modifiers.shift && self.model.term.selection.is_some() {
                if let Some(sel) = self.model.term.selection.as_mut() {
                    sel.update(p, side);
                }
            } else {
                let ty = match ev.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                self.model.term.selection = Some(Selection::new(ty, p, side));
            }
            self.selecting = true;
        }
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting
            && ev.pressed_button == Some(MouseButton::Left)
            && let Some((p, side)) = self.grid_point(ev.position)
        {
            if let Some(sel) = self.model.term.selection.as_mut() {
                sel.update(p, side);
            }
            cx.notify();
        }
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.model.term.selection_to_string() {
            Some(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if m.platform {
            if ks.key == "v" {
                if let Some(item) = cx.read_from_clipboard()
                    && let Some(text) = item.text()
                {
                    let bracketed = self.model.mode().contains(TermMode::BRACKETED_PASTE);
                    let bytes = encode_paste(&text, bracketed);
                    self.send_input(bytes, cx);
                    cx.stop_propagation();
                }
            } else if ks.key == "c" && self.copy_selection(cx) {
                cx.stop_propagation();
            }
            return; // 其余 cmd 组合留给 App
        }
        if self.marked.is_some() {
            return; // IME 组字中
        }
        let app_cursor = self.model.mode().contains(TermMode::APP_CURSOR);
        let ki = KeyInput {
            key: ks.key.as_str(),
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
        };
        if let Some(bytes) = encode_key(ki, app_cursor) {
            self.send_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let line_h = self.cell.map(|(_, h)| f32::from(h)).unwrap_or(18.);
        let dy = match ev.delta {
            ScrollDelta::Pixels(p) => f32::from(p.y) / line_h,
            ScrollDelta::Lines(l) => l.y,
        };
        if self.model.mode().contains(TermMode::ALT_SCREEN) {
            // 备用屏（TUI）：滚轮转方向键
            let n = dy.round() as i32;
            if n != 0 {
                let seq: &[u8] = if n > 0 { b"\x1b[A" } else { b"\x1b[B" };
                let bytes = seq.repeat(n.unsigned_abs().min(3) as usize);
                self.attach.input(bytes);
            }
            return;
        }
        self.scroll_accum += dy;
        let n = self.scroll_accum.trunc() as i32;
        if n != 0 {
            self.scroll_accum -= n as f32;
            self.model.scroll_display(n);
            cx.notify();
        }
    }

    fn metrics(&mut self, window: &mut Window) -> (Pixels, Pixels) {
        if let Some(m) = self.cell {
            return m;
        }
        let run = gpui::TextRun {
            len: 10,
            font: gpui::font("Menlo"),
            color: gpui::white(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line =
            window
                .text_system()
                .shape_line("MMMMMMMMMM".into(), px(FONT_SIZE), &[run], None);
        let cell_w = px((f32::from(line.width) / 10.0).max(1.0));
        let line_h = px((FONT_SIZE * LINE_HEIGHT_RATIO).round());
        self.cell = Some((cell_w, line_h));
        (cell_w, line_h)
    }

    // ── grid → 快照 ─────────────────────────────────────────────────────

    fn snapshot(&self) -> Snap {
        let term = &self.model.term;
        let content = term.renderable_content();
        let display_offset = content.display_offset;
        let rows = self.model.rows as usize;
        let colors = content.colors;

        let resolve = |color: &AnsiColor, default: u32| -> u32 {
            match color {
                AnsiColor::Spec(rgb) => {
                    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
                }
                AnsiColor::Indexed(i) => {
                    if let Some(rgb) = colors[*i as usize] {
                        ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
                    } else {
                        theme::indexed_color(*i)
                    }
                }
                AnsiColor::Named(n) => {
                    let idx = *n as usize;
                    if idx < 256 && let Some(rgb) = colors[idx] {
                        return ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32;
                    }
                    match n {
                        NamedColor::Foreground | NamedColor::BrightForeground => theme::TERM_FG,
                        NamedColor::Background => theme::TERM_BG,
                        NamedColor::Cursor => theme::CYAN,
                        _ if idx < 16 => theme::ANSI[idx],
                        _ => default,
                    }
                }
            }
        };

        let mut lines: Vec<LineSnap> = (0..rows)
            .map(|_| LineSnap {
                segs: Vec::new(),
                bgs: Vec::new(),
                sels: Vec::new(),
            })
            .collect();

        for indexed in content.display_iter {
            let vrow = indexed.point.line.0 + display_offset as i32;
            if vrow < 0 || vrow >= rows as i32 {
                continue;
            }
            let line = &mut lines[vrow as usize];
            let col = indexed.point.column.0 as u16;
            let cell = &*indexed;
            let flags = cell.flags;
            if flags.contains(CellFlags::WIDE_CHAR_SPACER) || flags.contains(CellFlags::HIDDEN) {
                continue;
            }
            let mut fg = resolve(&cell.fg, theme::TERM_FG);
            let mut bg = resolve(&cell.bg, theme::TERM_BG);
            if flags.contains(CellFlags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let bold = flags.intersects(CellFlags::BOLD);
            // 传统行为：粗体 + 基础 8 色 → 亮色
            if bold
                && let AnsiColor::Named(n) = cell.fg
                && (n as usize) < 8
                && !flags.contains(CellFlags::INVERSE)
            {
                fg = theme::ANSI[n as usize + 8];
            }
            // 背景 span
            if bg != theme::TERM_BG {
                let wide = flags.contains(CellFlags::WIDE_CHAR);
                let end = col + if wide { 2 } else { 1 };
                match line.bgs.last_mut() {
                    Some((_, e, color)) if *e == col && *color == bg => *e = end,
                    _ => line.bgs.push((col, end, bg)),
                }
            }
            // 选区 span
            if let Some(sr) = &content.selection
                && sr.contains(indexed.point)
            {
                let end = col + if flags.contains(CellFlags::WIDE_CHAR) { 2 } else { 1 };
                match line.sels.last_mut() {
                    Some((_, e)) if *e == col => *e = end,
                    _ => line.sels.push((col, end)),
                }
            }
            // 文本 seg（空格不画字）
            if cell.c != ' ' && cell.c != '\0' {
                let alpha = if flags.intersects(CellFlags::DIM) {
                    0.55
                } else {
                    1.0
                };
                let italic = flags.intersects(CellFlags::ITALIC);
                let underline = flags.intersects(CellFlags::UNDERLINE);
                let strike = flags.contains(CellFlags::STRIKEOUT);
                let wide = flags.contains(CellFlags::WIDE_CHAR);
                let can_merge = !wide
                    && cell.c.is_ascii()
                    && match line.segs.last() {
                        Some(s) => {
                            s.fg == fg
                                && s.alpha == alpha
                                && s.bold == bold
                                && s.italic == italic
                                && s.underline == underline
                                && s.strike == strike
                                && s.col + (s.text.len() as u16) == col
                                && s.text.is_ascii()
                        }
                        None => false,
                    };
                if can_merge {
                    line.segs.last_mut().unwrap().text.push(cell.c);
                } else {
                    line.segs.push(Seg {
                        col,
                        text: cell.c.to_string(),
                        fg,
                        alpha,
                        bold,
                        italic,
                        underline,
                        strike,
                    });
                }
            }
        }

        // 光标
        let cur = content.cursor;
        let cursor = if matches!(cur.shape, CursorShape::Hidden) {
            None
        } else {
            let vrow = cur.point.line.0 + display_offset as i32;
            if vrow >= 0 && vrow < rows as i32 {
                let ch = term.grid()[TermPoint::new(cur.point.line, cur.point.column)].c;
                Some((vrow as u16, cur.point.column.0 as u16, ch, cur.shape))
            } else {
                None
            }
        };

        Snap {
            lines,
            cursor,
            display_offset,
            history: self.model.history_len(),
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.marked
            .as_ref()
            .map(|m| 0..m.chars().map(|c| c.len_utf16()).sum())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = None;
        self.send_input(text.as_bytes().to_vec(), cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_string())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 光标格附近，IME 候选窗跟随
        let (cell_w, line_h) = self.cell?;
        let cur = self.model.term.grid().cursor.point;
        let x = element_bounds.origin.x + px(PAD) + cell_w * (cur.column.0 as f32);
        let y = element_bounds.origin.y + px(PAD) + line_h * (cur.line.0.max(0) as f32 + 1.0);
        Some(Bounds::new(point(x, y), size(cell_w, line_h)))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (cell_w, line_h) = self.metrics(window);
        let snap = self.snapshot();
        let entity = cx.entity();
        let entity2 = cx.entity();
        let handle = self.focus_handle.clone();
        let focused = self.focus_handle.is_focused(window);
        let marked = self.marked.clone();
        let last_sent = self.last_sent;
        let last_origin = self.last_origin;
        let conn_down = self.conn_down;

        let mono = |bold: bool, italic: bool| {
            let mut f = gpui::font("Menlo");
            if bold {
                f.weight = gpui::FontWeight::BOLD;
            }
            if italic {
                f.style = gpui::FontStyle::Italic;
            }
            f
        };

        div()
            .id("terminal")
            .size_full()
            .bg(c(theme::TERM_BG))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.selecting = false;
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _window, cx| {
                        // 尺寸 → 行列，变化则 defer 到绘制结束后应用 + 发 resize 帧；
                        // 同时回写内容区原点（鼠标选区换算用）
                        let cols =
                            (((f32::from(bounds.size.width) - PAD * 2.0) / f32::from(cell_w)).floor() as i32).max(2)
                                as u16;
                        let rows =
                            (((f32::from(bounds.size.height) - PAD * 2.0) / f32::from(line_h)).floor() as i32).max(2)
                                as u16;
                        let origin = bounds.origin;
                        if last_sent != Some((cols, rows)) || last_origin != Some(origin) {
                            let e = entity2.clone();
                            cx.defer(move |cx| {
                                e.update(cx, |t, cx| {
                                    t.last_origin = Some(origin);
                                    t.apply_view_size(cols, rows, cx);
                                });
                            });
                        }
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(&handle, ElementInputHandler::new(bounds, entity), cx);
                        let ox = bounds.origin.x + px(PAD);
                        let oy = bounds.origin.y + px(PAD);
                        let font_size = px(FONT_SIZE);

                        // 背景块
                        for (row, line) in snap.lines.iter().enumerate() {
                            let y = oy + line_h * (row as f32);
                            for (s, e, color) in &line.bgs {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(ox + cell_w * (*s as f32), y),
                                        size(cell_w * ((*e - *s) as f32), line_h),
                                    ),
                                    c(*color),
                                ));
                            }
                            // 选区高亮
                            for (s, e) in &line.sels {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(ox + cell_w * (*s as f32), y),
                                        size(cell_w * ((*e - *s) as f32), line_h),
                                    ),
                                    ca(theme::CYAN, 0.24),
                                ));
                            }
                        }
                        // 光标块（字下面先铺色）
                        if let Some((row, col, _, shape)) = snap.cursor {
                            let x = ox + cell_w * (col as f32);
                            let y = oy + line_h * (row as f32);
                            let (b, color) = match shape {
                                CursorShape::Block => (
                                    Bounds::new(point(x, y), size(cell_w, line_h)),
                                    ca(theme::CYAN, if focused { 0.9 } else { 0.35 }),
                                ),
                                CursorShape::Beam => (
                                    Bounds::new(point(x, y), size(px(2.), line_h)),
                                    ca(theme::CYAN, 0.9),
                                ),
                                CursorShape::Underline => (
                                    Bounds::new(
                                        point(x, y + line_h - px(2.)),
                                        size(cell_w, px(2.)),
                                    ),
                                    ca(theme::CYAN, 0.9),
                                ),
                                _ => (
                                    Bounds::new(point(x, y), size(cell_w, line_h)),
                                    ca(theme::CYAN, 0.35),
                                ),
                            };
                            window.paint_quad(fill(b, color));
                        }
                        // 文本
                        for (row, line) in snap.lines.iter().enumerate() {
                            let y = oy + line_h * (row as f32);
                            for seg in &line.segs {
                                let cursor_here = matches!(snap.cursor,
                                    Some((crow, ccol, _, CursorShape::Block))
                                        if crow as usize == row
                                            && ccol >= seg.col
                                            && (ccol as usize) < seg.col as usize + seg.text.chars().count());
                                let mut color: gpui::Hsla = c(seg.fg).into();
                                color.a = seg.alpha;
                                if cursor_here && focused && seg.text.chars().count() == 1 {
                                    // 单字符 seg 且光标在其上：反色
                                    color = c(theme::TERM_BG).into();
                                }
                                let run = gpui::TextRun {
                                    len: seg.text.len(),
                                    font: mono(seg.bold, seg.italic),
                                    color,
                                    background_color: None,
                                    underline: seg.underline.then(|| gpui::UnderlineStyle {
                                        thickness: px(1.),
                                        color: Some(color),
                                        wavy: false,
                                    }),
                                    strikethrough: seg.strike.then(|| gpui::StrikethroughStyle {
                                        thickness: px(1.),
                                        color: Some(color),
                                    }),
                                };
                                let shaped: SharedString = seg.text.clone().into();
                                // ASCII run 强制格宽，消除长串累计漂移；CJK 单字符 seg 本就按格定位
                                let force = seg.text.is_ascii().then_some(cell_w);
                                let line_shaped = window.text_system().shape_line(
                                    shaped,
                                    font_size,
                                    &[run],
                                    force,
                                );
                                let _ = line_shaped.paint(
                                    point(ox + cell_w * (seg.col as f32), y),
                                    line_h,
                                    gpui::TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }
                        // IME 组字预览
                        if let Some(m) = &marked
                            && let Some((row, col, _, _)) = snap.cursor
                        {
                            let x = ox + cell_w * (col as f32);
                            let y = oy + line_h * (row as f32);
                            let run = gpui::TextRun {
                                len: m.len(),
                                font: mono(false, false),
                                color: c(theme::INK).into(),
                                background_color: Some(c(theme::SURFACE_RAISED).into()),
                                underline: Some(gpui::UnderlineStyle {
                                    thickness: px(1.5),
                                    color: Some(c(theme::CYAN).into()),
                                    wavy: false,
                                }),
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(
                                m.clone().into(),
                                font_size,
                                &[run],
                                None,
                            );
                            window.paint_quad(fill(
                                Bounds::new(point(x, y), size(shaped.width, line_h)),
                                c(theme::SURFACE_RAISED),
                            ));
                            let _ = shaped.paint(
                                point(x, y),
                                line_h,
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                        // 回看指示
                        if snap.display_offset > 0 {
                            let label: SharedString = format!(
                                "回看 {}/{} 行 · 任意输入回到底部",
                                snap.display_offset, snap.history
                            )
                            .into();
                            let run = gpui::TextRun {
                                len: label.len(),
                                font: mono(false, false),
                                color: c(theme::AMBER).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(
                                label,
                                px(11.),
                                &[run],
                                None,
                            );
                            let x = bounds.origin.x + bounds.size.width - shaped.width - px(16.);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(x - px(8.), bounds.origin.y + px(4.)),
                                    size(shaped.width + px(16.), px(20.)),
                                ),
                                ca(theme::SURFACE_RAISED, 0.92),
                            ));
                            let _ = shaped.paint(
                                point(x, bounds.origin.y + px(7.)),
                                px(14.),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    },
                )
                .size_full(),
            )
            .when(conn_down, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .left(px(8.))
                        .px(px(10.))
                        .py(px(3.))
                        .rounded(px(6.))
                        .bg(ca(theme::RED, 0.15))
                        .border_1()
                        .border_color(c(theme::RED))
                        .text_size(px(11.))
                        .text_color(c(theme::RED))
                        .child("连接已断开 · 自动重连中…"),
                )
            })
    }
}
