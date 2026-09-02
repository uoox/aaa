//! 终端视图：自绘 gpui 元素遍历 alacritty `Term` grid。
//! 等宽 Menlo，前景/背景/粗体/斜体/下划线/光标块/回滚缓冲；
//! IME 走 EntityInputHandler（中文组字预览显示在光标处）；
//! 视图尺寸变化 → 行列重算 → WS resize 控制帧；
//! 鼠标：拖选 + 松手即复制、右键菜单（复制/粘贴）、链接悬停下划线并点击打开；
//! TUI 开了鼠标上报（claude code）时左键点击/拖动按鼠标协议转发给应用。
//! （独立实现，不含任何 Zed GPL terminal_view 代码。）

use alacritty_terminal::index::{Column, Line, Point as TermPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, CursorShape, NamedColor};
use gpui::{
    App, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollDelta, ScrollWheelEvent, SharedString, UTF16Selection, Window, canvas, div, fill, point,
    prelude::*, px, size,
};

use super::kit::{c, ca};
use crate::net::{AttachHandle, Net};
use crate::term::{
    KeyInput, TermModel, encode_key, encode_mouse_button, encode_mouse_motion, encode_paste,
    find_urls,
};
use crate::theme;

const FONT_SIZE: f32 = 12.5;
const LINE_HEIGHT_RATIO: f32 = 1.5;
const PAD: f32 = 8.0;

pub struct TerminalView {
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
    /// canvas 尺寸（prepaint 时回写；scrollbar 命中测试用）
    last_size: Option<gpui::Size<Pixels>>,
    /// scrollbar 拖拽中：抓住点相对 thumb 顶部的 y 偏移（px）
    sb_drag: Option<f32>,
    selecting: bool,
    /// 拖选自动滚动：+n 向上翻历史 / -n 向下 / 0 停（指针在视口内）
    sel_autoscroll: i32,
    /// 自动滚动循环是否在跑（避免重复 spawn）
    sel_scrolling: bool,
    /// 拖选中最后一次指针位置（自动滚动时延伸选区用）
    last_drag_pos: Option<gpui::Point<Pixels>>,
    /// 上一帧算出的可点链接（鼠标命中用；snapshot 时回写）
    links: Vec<LinkSpan>,
    /// 悬停中的链接（存跨度本身而非下标：重算后下标会错位）
    hover_link: Option<LinkSpan>,
    /// 左键按下已按鼠标协议转发给了应用（值 = 按钮号）：拖动/松开跟着走同一条路，
    /// 不碰本地选区。None = 这次手势是本地的（选区 / 链接）。
    mouse_reporting_press: Option<u8>,
    /// 上一次转发的移动事件落在哪一格（同格不重发——像素级事件会把 PTY 灌满）
    last_motion_cell: Option<(u16, u16)>,
}

/// 一段可点链接：viewport 行 + 列区间 [start, end) + 目标地址
#[derive(Clone, PartialEq)]
struct LinkSpan {
    row: u16,
    start: u16,
    end: u16,
    url: String,
}

/// scrollbar 命中带（右缘往里这么多像素内按下算抓滚动条，不开始选区）
const SB_ZONE: f32 = 20.0;
/// thumb 最小高度：历史很长时仍留得住指针
const SB_MIN_THUMB: f32 = 24.0;

/// 滚动条 thumb 几何：返回 (top, height)，px、相对视图顶部。
/// 没有回滚历史时无滚动条（备用屏 history 恒为 0，自然隐藏）。
fn scrollbar_thumb(view_h: f32, rows: usize, history: usize, offset: usize) -> Option<(f32, f32)> {
    if history == 0 || view_h <= 0. {
        return None;
    }
    let total = (history + rows) as f32;
    let h = (view_h * rows as f32 / total).clamp(SB_MIN_THUMB.min(view_h), view_h);
    let range = view_h - h;
    let top = range * (history - offset.min(history)) as f32 / history as f32;
    Some((top, h))
}

/// 拖拽反解：thumb 顶部 y → 回看深度（offset，0 = 底部）
fn offset_for_thumb_top(view_h: f32, rows: usize, history: usize, top: f32) -> usize {
    let Some((_, h)) = scrollbar_thumb(view_h, rows, history, 0) else {
        return 0;
    };
    let range = view_h - h;
    if range <= 0. {
        return 0;
    }
    let frac = (top / range).clamp(0., 1.);
    (history as f32 * (1. - frac)).round() as usize
}

#[derive(Debug, PartialEq)]
enum MouseUpAction {
    Copy,
    OpenLink,
    Nothing,
}

/// 左键松手到底算什么。选区优先：一次手势要么是「选中复制」，要么是「点链接」，
/// 不能两件事都办——否则拖选到链接上松手会顺手把浏览器也打开。
/// 空选区（只是点了一下）不动剪贴板，所以单击链接仍然能走到 OpenLink。
fn resolve_mouse_up(has_selection: bool, over_link: bool) -> MouseUpAction {
    match (has_selection, over_link) {
        (true, _) => MouseUpAction::Copy,
        (false, true) => MouseUpAction::OpenLink,
        (false, false) => MouseUpAction::Nothing,
    }
}

/// 左键按下归谁：TUI 开了鼠标上报（1000/1002/1003 任一）就是应用的——点选项、
/// 点焦点在真终端里都能用，靠的就是这个。Shift+点击按 xterm 惯例绕过应用，
/// 仍走本地选区，给用户留一条在 TUI 里复制文字的路。
fn click_goes_to_app(mode: TermMode, shift: bool) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !shift
}

/// gpui 修饰键 → 鼠标协议要的 (shift, alt, ctrl)。cmd 不参与（xterm 没这一位）。
fn mouse_mods(m: &gpui::Modifiers) -> (bool, bool, bool) {
    (m.shift, m.alt, m.control)
}

/// 一行的链接：OSC 8（终端自己声明的）优先，正则扫出来的只补它没盖到的段落。
/// `cols[i]` 是第 i 个字符的起始列——CJK 占两格，字符下标推不出列号，只能查表。
fn line_links(
    row: u16,
    text: &str,
    cols: &[u16],
    osc8: &[(u16, u16, String)],
) -> Vec<LinkSpan> {
    let mut out: Vec<LinkSpan> = osc8
        .iter()
        .map(|(s, e, url)| LinkSpan {
            row,
            start: *s,
            end: *e,
            url: url.clone(),
        })
        .collect();
    for span in find_urls(text) {
        let last = span.end - 1;
        let start = cols[span.start];
        // 结束列 = 末字符起始列 + 它占的格数；行尾没有下一格可比时按 1 格算
        let end = cols.get(last + 1).copied().unwrap_or(cols[last] + 1);
        if osc8.iter().any(|(s, e, _)| start < *e && *s < end) {
            continue;
        }
        out.push(LinkSpan {
            row,
            start,
            end,
            url: span.url,
        });
    }
    out
}

// ── 渲染快照（render 时从 grid 提取，canvas 闭包里绘制） ────────────────────

#[derive(Clone, Copy, PartialEq)]
struct SegStyle {
    fg: u32,
    alpha: f32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

struct Seg {
    col: u16,
    /// 每字符占用格数（1 窄 / 2 宽），seg 内统一；shape 时按 cell_w×此值强制推进
    cell_w: u16,
    /// 字符数（≠ 字节数；光标命中/反色判断用）
    chars: u16,
    text: String,
    style: SegStyle,
}

struct LineSnap {
    segs: Vec<Seg>,
    bgs: Vec<(u16, u16, u32)>, // [start_col, end_col) 背景
    sels: Vec<(u16, u16)>,     // [start_col, end_col) 选区高亮
}

impl LineSnap {
    /// 追加一格：与上一 seg 同风格、同字宽且列连续则并入。
    /// 宽字符（CJK）也整段合并、一次 shape_line 按 2 格强制推进——
    /// 每字符单独 shape 是已知热点（一行中文 ≈ 60 次 shape → 1 次）。
    fn push_cell(&mut self, col: u16, ch: char, wide: bool, style: SegStyle) {
        let cell_w = if wide { 2 } else { 1 };
        match self.segs.last_mut() {
            Some(s)
                if s.cell_w == cell_w
                    && s.style == style
                    && s.col + s.chars * s.cell_w == col =>
            {
                s.text.push(ch);
                s.chars += 1;
            }
            _ => self.segs.push(Seg {
                col,
                cell_w,
                chars: 1,
                text: ch.to_string(),
                style,
            }),
        }
    }
}

struct Snap {
    lines: Vec<LineSnap>,
    cursor: Option<(u16, u16, char, CursorShape)>, // (viewport_row, col, ch, shape)
    display_offset: usize,
    history: usize,
}

/// 该帧是不是「裸清屏」：最后一个 2J/3J 之后只剩 ≤8 字节的光标残尾
/// （典型：`\x1b[2J\x1b[3J\x1b[H` 独占一帧）。这样的帧渲染出来是全黑。
fn batch_is_bare_clear(bytes: &[u8]) -> bool {
    let find_last = |pat: &[u8]| {
        bytes
            .windows(pat.len())
            .rposition(|w| w == pat)
            .map(|p| p + pat.len())
    };
    let end2 = find_last(b"\x1b[2J");
    let end3 = find_last(b"\x1b[3J");
    match end2.max(end3) {
        Some(end) => bytes.len() - end <= 8,
        None => false,
    }
}

/// Cmd-V 和 Ctrl-V 都粘贴：从 Linux/Windows 过来的手指记的是后者，而终端里
/// 裸 Ctrl-V 本来是 literal-next，几乎没人用得上。Ctrl-C 不在此列——那是中断
/// 信号，抢过来会让 agent 停不下来。
fn is_paste_chord(key: &str, platform: bool, control: bool) -> bool {
    key == "v" && (platform || control)
}

impl TerminalView {
    pub fn new(session_id: String, net: &Net, cx: &mut Context<Self>) -> Self {
        let attach = net.attach(&session_id);
        TerminalView {
            model: TermModel::new(80, 24),
            attach,
            focus_handle: cx.focus_handle(),
            marked: None,
            conn_down: false,
            last_sent: None,
            cell: None,
            scroll_accum: 0.,
            last_origin: None,
            last_size: None,
            sb_drag: None,
            selecting: false,
            sel_autoscroll: 0,
            sel_scrolling: false,
            last_drag_pos: None,
            links: Vec::new(),
            hover_link: None,
            mouse_reporting_press: None,
            last_motion_cell: None,
        }
    }

    /// WS 二进制帧 → VT 模型；term 的应答字节写回 PTY
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.conn_down = false;
        for answer in self.model.advance(bytes) {
            self.attach.input(answer);
        }
        // agy 这类 TUI 每几秒把「清屏」和「整屏重绘」拆成两个帧发：立刻渲染
        // 清屏帧就是肉眼可见地黑一下（一直闪）。清屏帧压 50ms 与紧随的重绘
        // 合帧；真正的 clear 也只是晚 50ms 显示。
        if batch_is_bare_clear(bytes) {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = this.update(cx, |_, cx| cx.notify());
            })
            .detach();
            return;
        }
        cx.notify();
    }

    /// attach（重）连接就绪：hello 后紧跟整屏 replay（含数百行历史）。
    /// 不清模型的话每次断线重连都会往回滚缓冲再叠一份同样的历史（审查 P1）。
    pub fn reset_for_replay(&mut self, cx: &mut Context<Self>) {
        self.model = TermModel::new(self.model.cols, self.model.rows);
        self.links.clear();
        self.hover_link = None;
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

    /// 窗口坐标 → viewport 行列（链接命中用；越界返回 None，
    /// 不像 `grid_point` 那样把点夹到边上——夹了就会误开边缘的链接）
    fn viewport_cell(&self, pos: gpui::Point<Pixels>) -> Option<(u16, u16)> {
        let (cell_w, line_h) = self.cell?;
        let origin = self.last_origin?;
        let x = f32::from(pos.x) - f32::from(origin.x) - PAD;
        let y = f32::from(pos.y) - f32::from(origin.y) - PAD;
        if x < 0. || y < 0. {
            return None;
        }
        let col = (x / f32::from(cell_w)).floor();
        let row = (y / f32::from(line_h)).floor();
        if col >= self.model.cols as f32 || row >= self.model.rows as f32 {
            return None;
        }
        Some((row as u16, col as u16))
    }

    fn link_at(&self, pos: gpui::Point<Pixels>) -> Option<LinkSpan> {
        let (row, col) = self.viewport_cell(pos)?;
        self.links
            .iter()
            .find(|l| l.row == row && col >= l.start && col < l.end)
            .cloned()
    }

    /// 窗口坐标 → 鼠标上报用的 (row, col)。落在 padding 里的点夹到贴边那一格
    /// （真终端也这么报：菜单第一列左边差两个像素照样算点中），
    /// 只有度量还没算出来时才 None。
    fn report_cell(&self, pos: gpui::Point<Pixels>) -> Option<(u16, u16)> {
        if let Some(cell) = self.viewport_cell(pos) {
            return Some(cell);
        }
        let (cell_w, line_h) = self.cell?;
        let origin = self.last_origin?;
        let x = f32::from(pos.x) - f32::from(origin.x) - PAD;
        let y = f32::from(pos.y) - f32::from(origin.y) - PAD;
        let col = (x / f32::from(cell_w))
            .floor()
            .clamp(0., (self.model.cols - 1) as f32) as u16;
        let row = (y / f32::from(line_h))
            .floor()
            .clamp(0., (self.model.rows - 1) as f32) as u16;
        Some((row, col))
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(item) = cx.read_from_clipboard()
            && let Some(text) = item.text()
        {
            let bracketed = self.model.mode().contains(TermMode::BRACKETED_PASTE);
            let bytes = encode_paste(&text, bracketed);
            self.send_input(bytes, cx);
        }
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        if self.sb_mouse_down(ev.position, cx) {
            return;
        }
        let mode = self.model.mode();
        if click_goes_to_app(mode, ev.modifiers.shift) {
            // TUI 开了鼠标上报（claude code：1000+1006）：这一下是应用的，
            // 不开选区；已有的选区也清掉——屏幕接下来归应用重绘。
            // 直发 PTY：不走 send_input（那会重置视口）。
            self.model.term.selection = None;
            self.selecting = false;
            if let Some((row, col)) = self.report_cell(ev.position)
                && let Some(bytes) =
                    encode_mouse_button(mode, 0, true, col, row, mouse_mods(&ev.modifiers))
            {
                self.attach.input(bytes);
                self.mouse_reporting_press = Some(0);
                self.last_motion_cell = Some((row, col));
            }
            cx.notify();
            return;
        }
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
        if self.sb_drag.is_some() && ev.pressed_button == Some(MouseButton::Left) {
            if let Some(origin) = self.last_origin {
                let y = f32::from(ev.position.y) - f32::from(origin.y);
                self.sb_drag_to(y, cx);
            }
            return;
        }
        if let Some(btn) = self.mouse_reporting_press {
            // 按下已经交给了应用：拖动也归它（1002/1003 才报，encode 自己判），
            // 不碰选区、不管链接悬停。
            let mode = self.model.mode();
            let mods = mouse_mods(&ev.modifiers);
            let Some((row, col)) = self.report_cell(ev.position) else {
                return;
            };
            if ev.pressed_button != Some(MouseButton::Left) {
                // 松开发生在视图外（gpui 只把 mouse_up 派给悬停中的元素）：
                // 在这儿补一个 release，别让应用一直以为键还按着
                self.mouse_reporting_press = None;
                self.last_motion_cell = None;
                if let Some(bytes) = encode_mouse_button(mode, btn, false, col, row, mods) {
                    self.attach.input(bytes);
                }
                return;
            }
            if self.last_motion_cell != Some((row, col)) {
                self.last_motion_cell = Some((row, col));
                if let Some(bytes) = encode_mouse_motion(mode, btn, col, row, mods) {
                    self.attach.input(bytes);
                }
            }
            return;
        }
        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            // 拖选进行中不去管链接：一次手势只干一件事
            self.last_drag_pos = Some(ev.position);
            if let Some((p, side)) = self.grid_point(ev.position)
                && let Some(sel) = self.model.term.selection.as_mut()
            {
                sel.update(p, side);
                cx.notify();
            }
            // 拖出上/下边界 → 自动滚动翻历史（标准终端行为）；越远越快
            if let (Some(origin), Some(size)) = (self.last_origin, self.last_size) {
                let y = f32::from(ev.position.y);
                let top = f32::from(origin.y);
                let bot = top + f32::from(size.height);
                let line_h = self.cell.map(|(_, h)| f32::from(h)).unwrap_or(18.);
                self.sel_autoscroll = if y < top {
                    (1. + (top - y) / line_h).min(6.) as i32
                } else if y > bot {
                    -((1. + (y - bot) / line_h).min(6.) as i32)
                } else {
                    0
                };
                if self.sel_autoscroll != 0 {
                    self.spawn_sel_autoscroll(cx);
                }
            }
            return;
        }
        // 悬停高亮只在跨度变化时 notify，否则鼠标一动就整屏重绘
        let hit = self.link_at(ev.position);
        if hit != self.hover_link {
            self.hover_link = hit;
            cx.notify();
        }
    }

    /// 60ms 一拍的自动滚动循环：拖选压在边界外时持续翻页并把选区端点
    /// 推到视口边缘。松手（selecting=false）或回到视口内自然停。
    fn spawn_sel_autoscroll(&mut self, cx: &mut Context<Self>) {
        if self.sel_scrolling {
            return;
        }
        self.sel_scrolling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(60))
                    .await;
                let cont = this
                    .update(cx, |t: &mut TerminalView, cx| {
                        if !t.selecting || t.sel_autoscroll == 0 {
                            t.sel_scrolling = false;
                            return false;
                        }
                        t.model.scroll_display(t.sel_autoscroll);
                        // grid_point 会把边界外的点夹到视口边缘行；offset 变了，
                        // 同一屏幕点对应的绝对行随之前移/后移，选区就跟着长
                        if let Some(pos) = t.last_drag_pos
                            && let Some((p, side)) = t.grid_point(pos)
                            && let Some(sel) = t.model.term.selection.as_mut()
                        {
                            sel.update(p, side);
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
            }
        })
        .detach();
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.sb_drag.take().is_some() {
            cx.notify();
            return;
        }
        if let Some(btn) = self.mouse_reporting_press.take() {
            // 这次手势是应用的：只补 release，不复制、不开链接（本来就没选区）。
            // 应用可能在按下和松开之间关了鼠标上报，encode 返回 None 就算了。
            self.last_motion_cell = None;
            let mode = self.model.mode();
            if let Some((row, col)) = self.report_cell(ev.position)
                && let Some(bytes) =
                    encode_mouse_button(mode, btn, false, col, row, mouse_mods(&ev.modifiers))
            {
                self.attach.input(bytes);
            }
            cx.notify();
            return;
        }
        self.selecting = false;
        self.sel_autoscroll = 0;
        self.last_drag_pos = None;
        let link = self.link_at(ev.position);
        match resolve_mouse_up(self.has_selection(), link.is_some()) {
            MouseUpAction::Copy => {
                self.copy_selection(cx);
            }
            MouseUpAction::OpenLink => {
                if let Some(l) = link {
                    cx.open_url(&l.url);
                }
            }
            MouseUpAction::Nothing => {}
        }
        cx.notify();
    }

    /// 右键 = 直接粘贴，不弹菜单。终端里右键几乎只为了这一件事，多一次点击
    /// 就多一次打断；复制已经由「选中即复制」承担了。
    /// 右键不碰选区（左键才注册了 on_mouse_down），所以粘贴不会清掉刚选的东西。
    fn on_right_down(&mut self, _ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_handle.focus(window, cx);
        self.paste_from_clipboard(cx);
        cx.stop_propagation();
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

    fn has_selection(&self) -> bool {
        self.model
            .term
            .selection_to_string()
            .is_some_and(|s| !s.is_empty())
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        if is_paste_chord(ks.key.as_str(), m.platform, m.control) {
            self.paste_from_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        if m.platform {
            if ks.key == "c" && self.copy_selection(cx) {
                cx.stop_propagation();
            }
            return; // 其余 cmd 组合留给 App
        }
        if m.control && ks.key == "tab" {
            return; // Ctrl-Tab 留给 App 级「切换激活会话」，不进 PTY
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
        self.scroll_accum += dy;
        let n = self.scroll_accum.trunc() as i32;
        if n == 0 {
            return;
        }
        self.scroll_accum -= n as f32;
        let mode = self.model.mode();
        // TUI 开了鼠标上报（claude code：1000+1006）→ 滚轮按鼠标协议转发，
        // 内容由应用自己滚。这不是「合成方向键」：wheel 事件只表达滚动，
        // TUI 的菜单/选项不会因此乱跳。真终端里 claude code 能滚全靠这个。
        if mode.intersects(TermMode::MOUSE_MODE) && !ev.modifiers.shift {
            let (row, col) = self.viewport_cell(ev.position).unwrap_or((0, 0));
            let up = n > 0;
            let mut bytes = Vec::new();
            for _ in 0..n.unsigned_abs() {
                if let Some(seq) = crate::term::encode_wheel(mode, up, col, row) {
                    bytes.extend(seq);
                }
            }
            // 直发 PTY：不走 send_input（那会重置视口/清选区）
            self.attach.input(bytes);
            return;
        }
        if mode.contains(TermMode::ALT_SCREEN) {
            // 备用屏、无鼠标上报（less / man 这类分页器）：按 1007 alternate
            // scroll 惯例转 ↑↓——分页器里这就是滚动。开了鼠标上报的 TUI 走不到
            // 这里，不会出现「滚轮变方向键乱跳」。
            if mode.contains(TermMode::ALTERNATE_SCROLL) {
                let app_cursor = mode.contains(TermMode::APP_CURSOR);
                let seq: &[u8] = match (n > 0, app_cursor) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                let mut bytes = Vec::new();
                for _ in 0..n.unsigned_abs() {
                    bytes.extend_from_slice(seq);
                }
                self.attach.input(bytes);
            }
            // 备用屏没有回滚缓冲，除此之外滚了就是没动静——诚实的行为
            return;
        }
        // 主屏：滚视口（回滚缓冲）
        self.model.scroll_display(n);
        cx.notify();
    }

    // ── scrollbar ───────────────────────────────────────────────────────

    /// 当前 scrollbar thumb 几何（窗口坐标系的 top 相对视图顶部）。
    /// 备用屏 history=0 → None，滚动条自然隐藏。
    fn sb_thumb(&self) -> Option<(f32, f32)> {
        let view_h = f32::from(self.last_size?.height);
        scrollbar_thumb(
            view_h,
            self.model.rows as usize,
            self.model.history_len(),
            self.model.display_offset(),
        )
    }

    /// 按下点若落在 scrollbar 带内则接管（返回 true，不开始选区）
    fn sb_mouse_down(&mut self, pos: gpui::Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let (Some(origin), Some(size)) = (self.last_origin, self.last_size) else {
            return false;
        };
        let x = f32::from(pos.x) - f32::from(origin.x);
        let y = f32::from(pos.y) - f32::from(origin.y);
        if x < f32::from(size.width) - SB_ZONE {
            return false;
        }
        let Some((top, h)) = self.sb_thumb() else {
            return false;
        };
        let grab = if y >= top && y <= top + h {
            y - top
        } else {
            // 点在轨道上：thumb 中心跳到点击处再进入拖拽
            h / 2.
        };
        self.sb_drag = Some(grab);
        self.sb_drag_to(y, cx);
        true
    }

    fn sb_drag_to(&mut self, y_in_view: f32, cx: &mut Context<Self>) {
        let Some(grab) = self.sb_drag else { return };
        let Some(size) = self.last_size else { return };
        let offset = offset_for_thumb_top(
            f32::from(size.height),
            self.model.rows as usize,
            self.model.history_len(),
            y_in_view - grab,
        );
        self.model.scroll_to(offset);
        cx.notify();
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

    /// 提取渲染快照，顺带把这一屏的可点链接回写到 `self.links`
    /// （鼠标事件里没有 grid 可遍历，命中测试只能吃这份缓存）
    fn snapshot(&mut self) -> Snap {
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
                        theme::palette().indexed_color(*i)
                    }
                }
                AnsiColor::Named(n) => {
                    let idx = *n as usize;
                    if idx < 256 && let Some(rgb) = colors[idx] {
                        return ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32;
                    }
                    match n {
                        NamedColor::Foreground | NamedColor::BrightForeground => theme::term_fg(),
                        NamedColor::Background => theme::term_bg(),
                        NamedColor::Cursor => theme::accent(),
                        _ if idx < 16 => theme::palette().ansi[idx],
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
        // 链接扫描的两个原料：整行文本（含空格）+ 每字符起始列。
        // 列必须查表——一个 CJK 占两格，字符下标推不出列号。
        let mut raw: Vec<(String, Vec<u16>)> =
            (0..rows).map(|_| (String::new(), Vec::new())).collect();
        // OSC 8 显式超链接：终端自己声明的地址，优先于按文本猜的
        let mut osc8: Vec<Vec<(u16, u16, String)>> = (0..rows).map(|_| Vec::new()).collect();

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
            let wide = flags.contains(CellFlags::WIDE_CHAR);
            {
                let (text, cols) = &mut raw[vrow as usize];
                text.push(if cell.c == '\0' { ' ' } else { cell.c });
                cols.push(col);
            }
            if let Some(h) = cell.hyperlink() {
                let end = col + if wide { 2 } else { 1 };
                let runs = &mut osc8[vrow as usize];
                match runs.last_mut() {
                    Some((_, e, uri)) if *e == col && uri == h.uri() => *e = end,
                    _ => runs.push((col, end, h.uri().to_string())),
                }
            }
            let mut fg = resolve(&cell.fg, theme::term_fg());
            let mut bg = resolve(&cell.bg, theme::term_bg());
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
                fg = theme::palette().ansi[n as usize + 8];
            }
            // 背景 span
            if bg != theme::term_bg() {
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
                let end = col + if wide { 2 } else { 1 };
                match line.sels.last_mut() {
                    Some((_, e)) if *e == col => *e = end,
                    _ => line.sels.push((col, end)),
                }
            }
            // 文本 seg（空格不画字）
            if cell.c != ' ' && cell.c != '\0' {
                let style = SegStyle {
                    fg,
                    alpha: if flags.intersects(CellFlags::DIM) { 0.55 } else { 1.0 },
                    bold,
                    italic: flags.intersects(CellFlags::ITALIC),
                    underline: flags.intersects(CellFlags::UNDERLINE),
                    strike: flags.contains(CellFlags::STRIKEOUT),
                };
                line.push_cell(col, cell.c, wide, style);
            }
        }

        let links: Vec<LinkSpan> = raw
            .iter()
            .enumerate()
            .flat_map(|(vrow, (text, cols))| line_links(vrow as u16, text, cols, &osc8[vrow]))
            .collect();

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
        let history = self.model.history_len();

        // 屏幕滚过之后旧的悬停跨度可能已经不在了，别留着画一条无主的下划线
        if self.hover_link.as_ref().is_some_and(|h| !links.contains(h)) {
            self.hover_link = None;
        }
        self.links = links;

        Snap {
            lines,
            cursor,
            display_offset,
            history,
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
        let last_size = self.last_size;
        let sb_dragging = self.sb_drag.is_some();
        let term_rows = self.model.rows as usize;
        let conn_down = self.conn_down;
        let hover = self
            .hover_link
            .as_ref()
            .map(|l| (l.row, l.start, l.end));

        // 4 个字体变体一次构建（seg 循环内只 clone，不重复走 font()/SharedString 分配）
        let fonts: [gpui::Font; 4] = std::array::from_fn(|i| {
            let mut f = gpui::font("Menlo");
            if i & 1 != 0 {
                f.weight = gpui::FontWeight::BOLD;
            }
            if i & 2 != 0 {
                f.style = gpui::FontStyle::Italic;
            }
            f
        });
        let mono =
            move |bold: bool, italic: bool| fonts[(bold as usize) | ((italic as usize) << 1)].clone();

        div()
            .id("terminal")
            .size_full()
            .bg(c(theme::term_bg()))
            .track_focus(&self.focus_handle)
            .when(hover.is_some(), |el| el.cursor_pointer())
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_right_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
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
                        let bsize = bounds.size;
                        if last_sent != Some((cols, rows))
                            || last_origin != Some(origin)
                            || last_size != Some(bsize)
                        {
                            let e = entity2.clone();
                            cx.defer(move |cx| {
                                e.update(cx, |t, cx| {
                                    t.last_origin = Some(origin);
                                    t.last_size = Some(bsize);
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
                                    ca(theme::accent(), 0.24),
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
                                    ca(theme::accent(), if focused { 0.9 } else { 0.35 }),
                                ),
                                CursorShape::Beam => (
                                    Bounds::new(point(x, y), size(px(2.), line_h)),
                                    ca(theme::accent(), 0.9),
                                ),
                                CursorShape::Underline => (
                                    Bounds::new(
                                        point(x, y + line_h - px(2.)),
                                        size(cell_w, px(2.)),
                                    ),
                                    ca(theme::accent(), 0.9),
                                ),
                                _ => (
                                    Bounds::new(point(x, y), size(cell_w, line_h)),
                                    ca(theme::accent(), 0.35),
                                ),
                            };
                            window.paint_quad(fill(b, color));
                        }
                        // 文本（lines 按值消费：seg.text 直接转 SharedString，不再逐帧 clone）
                        for (row, line) in snap.lines.into_iter().enumerate() {
                            let y = oy + line_h * (row as f32);
                            for seg in line.segs {
                                let cursor_here = matches!(snap.cursor,
                                    Some((crow, ccol, _, CursorShape::Block))
                                        if crow as usize == row
                                            && ccol >= seg.col
                                            && ccol < seg.col + seg.chars * seg.cell_w);
                                let mut color: gpui::Hsla = c(seg.style.fg).into();
                                color.a = seg.style.alpha;
                                if cursor_here && focused && seg.chars == 1 {
                                    // 单字符 seg 且光标在其上：反色
                                    color = c(theme::term_bg()).into();
                                }
                                let run = gpui::TextRun {
                                    len: seg.text.len(),
                                    font: mono(seg.style.bold, seg.style.italic),
                                    color,
                                    background_color: None,
                                    underline: seg.style.underline.then(|| gpui::UnderlineStyle {
                                        thickness: px(1.),
                                        color: Some(color),
                                        wavy: false,
                                    }),
                                    strikethrough: seg.style.strike.then(|| {
                                        gpui::StrikethroughStyle {
                                            thickness: px(1.),
                                            color: Some(color),
                                        }
                                    }),
                                };
                                // 全部按格宽强制推进（窄 1 格 / 宽 2 格）：
                                // ASCII 消除长串累计漂移，CJK 合并段逐字对齐格点
                                let force = Some(cell_w * (seg.cell_w as f32));
                                let line_shaped = window.text_system().shape_line(
                                    seg.text.into(),
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
                        // 悬停链接的下划线（画在格底，不动 seg 的 TextRun：
                        // 一改 style 整段就得重新合并，悬停不值这个代价）
                        if let Some((row, s, e)) = hover {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        ox + cell_w * (s as f32),
                                        oy + line_h * (row as f32) + line_h - px(2.),
                                    ),
                                    size(cell_w * ((e - s) as f32), px(1.)),
                                ),
                                c(theme::accent()),
                            ));
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
                                color: c(theme::ink()).into(),
                                background_color: Some(c(theme::surface_raised()).into()),
                                underline: Some(gpui::UnderlineStyle {
                                    thickness: px(1.5),
                                    color: Some(c(theme::accent()).into()),
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
                                c(theme::surface_raised()),
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
                        // 右侧滚动条（有回滚历史才画；备用屏 history=0 自然无）
                        if let Some((top, h)) = scrollbar_thumb(
                            f32::from(bounds.size.height),
                            term_rows,
                            snap.history,
                            snap.display_offset,
                        ) {
                            let track_x =
                                bounds.origin.x + bounds.size.width - px(16.);
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(track_x, bounds.origin.y),
                                    size(px(14.), bounds.size.height),
                                ),
                                ca(theme::edge_light(), 0.35),
                            ));
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(track_x + px(2.), bounds.origin.y + px(top)),
                                    size(px(10.), px(h)),
                                ),
                                ca(theme::dim(), if sb_dragging { 0.85 } else { 0.45 }),
                            ));
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
                                color: c(theme::amber()).into(),
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
                                ca(theme::surface_raised(), 0.92),
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
                        .bg(ca(theme::red(), 0.15))
                        .border_1()
                        .border_color(c(theme::red()))
                        .text_size(px(11.))
                        .text_color(c(theme::red()))
                        .child("连接已断开 · 自动重连中…"),
                )
            })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn both_paste_chords_work_and_ctrl_c_is_left_alone() {
        assert!(is_paste_chord("v", true, false), "cmd-V");
        assert!(is_paste_chord("v", false, true), "ctrl-V");
        assert!(!is_paste_chord("c", false, true), "ctrl-C 必须留给中断信号");
        assert!(!is_paste_chord("v", false, false), "裸 v 是普通输入");
    }
    use super::*;

    fn style(fg: u32) -> SegStyle {
        SegStyle {
            fg,
            alpha: 1.0,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    fn empty_line() -> LineSnap {
        LineSnap {
            segs: Vec::new(),
            bgs: Vec::new(),
            sels: Vec::new(),
        }
    }

    #[test]
    fn merges_ascii_and_wide_runs_separately() {
        // "ab中文x"：a(0) b(1) 中(2,宽) 文(4,宽) x(6)
        let mut line = empty_line();
        let s = style(0xffffff);
        line.push_cell(0, 'a', false, s);
        line.push_cell(1, 'b', false, s);
        line.push_cell(2, '中', true, s);
        line.push_cell(4, '文', true, s);
        line.push_cell(6, 'x', false, s);
        let segs = &line.segs;
        assert_eq!(segs.len(), 3, "窄/宽切换处分段");
        assert_eq!((segs[0].col, segs[0].cell_w, segs[0].text.as_str()), (0, 1, "ab"));
        assert_eq!((segs[1].col, segs[1].cell_w, segs[1].text.as_str()), (2, 2, "中文"));
        assert_eq!(segs[1].chars, 2);
        assert_eq!((segs[2].col, segs[2].cell_w, segs[2].text.as_str()), (6, 1, "x"));
    }

    #[test]
    fn style_change_and_gap_break_merge() {
        let mut line = empty_line();
        line.push_cell(0, 'a', false, style(0xffffff));
        line.push_cell(1, 'b', false, style(0xff0000)); // 换色
        line.push_cell(3, 'c', false, style(0xff0000)); // 列不连续（跳过空格）
        assert_eq!(line.segs.len(), 3);
        // 宽字符列连续性按 2 格推进：中(0) 文(2) 连续，文(2) 后跳到 5 断开
        let mut line = empty_line();
        let s = style(0xffffff);
        line.push_cell(0, '中', true, s);
        line.push_cell(2, '文', true, s);
        line.push_cell(5, '字', true, s);
        assert_eq!(line.segs.len(), 2);
        assert_eq!(line.segs[0].text, "中文");
        assert_eq!(line.segs[1].col, 5);
    }

    /// 每字符一格的行（终端里的 ASCII 行）
    fn ascii_cols(text: &str) -> Vec<u16> {
        (0..text.chars().count() as u16).collect()
    }

    #[test]
    fn link_columns_account_for_wide_chars() {
        // "打开 https://a.io" —— 两个 CJK 各占 2 格，URL 从第 5 列起
        let text = "打开 https://a.io";
        let mut cols = vec![0u16, 2, 4]; // 打(0,宽) 开(2,宽) 空格(4)
        cols.extend(5..5 + "https://a.io".chars().count() as u16);
        let links = line_links(3, text, &cols, &[]);
        assert_eq!(links.len(), 1);
        assert_eq!((links[0].row, links[0].start), (3, 5));
        assert_eq!(links[0].end, 5 + "https://a.io".chars().count() as u16);
        assert_eq!(links[0].url, "https://a.io");
    }

    #[test]
    fn link_at_line_end_gets_a_column() {
        // URL 顶到行尾：末字符后面没有下一格可查，按 1 格算
        let text = "x https://a.io";
        let links = line_links(0, text, &ascii_cols(text), &[]);
        assert_eq!((links[0].start, links[0].end), (2, 14));
    }

    #[test]
    fn osc8_wins_over_text_scan() {
        // 终端自己声明的地址与显示文本不一致时，以 OSC 8 为准，不叠加第二条
        let text = "click https://decoy.example here";
        let osc8 = vec![(6u16, 27u16, "https://real.example/x".to_string())];
        let links = line_links(0, text, &ascii_cols(text), &osc8);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].url, "https://real.example/x");
        // 不重叠的正则结果照样保留
        let text = "a https://b.io  and more";
        let osc8 = vec![(0u16, 1u16, "https://osc.io".to_string())];
        let links = line_links(0, text, &ascii_cols(text), &osc8);
        assert_eq!(links.len(), 2);
    }

    #[test]
    fn bare_clear_frames_are_coalesced() {
        // agy 实测帧：清屏独占一帧 → 压帧；带内容的帧照常立即渲染
        assert!(batch_is_bare_clear(b"\x1b[2J\x1b[3J\x1b[H"));
        assert!(batch_is_bare_clear(b"\x1b[2J"));
        assert!(!batch_is_bare_clear(b"\x1b[2J\x1b[Hhello world, repaint"));
        assert!(!batch_is_bare_clear(b"plain output"));
        assert!(!batch_is_bare_clear(b""));
    }

    #[test]
    fn scrollbar_geometry_roundtrips() {
        // 视图 600px、24 行、历史 976 行 → total 1000
        let (top, h) = scrollbar_thumb(600., 24, 976, 0).unwrap();
        assert!(h >= SB_MIN_THUMB);
        // offset=0（底部）→ thumb 贴底
        assert!((top + h - 600.).abs() < 0.5, "底部时贴底，top={top} h={h}");
        // offset=history（顶部）→ thumb 贴顶
        let (top2, _) = scrollbar_thumb(600., 24, 976, 976).unwrap();
        assert!(top2.abs() < 0.5);
        // 反解往返：任一 offset → top → offset 回到原值
        for off in [0usize, 1, 488, 975, 976] {
            let (t, _) = scrollbar_thumb(600., 24, 976, off).unwrap();
            assert_eq!(offset_for_thumb_top(600., 24, 976, t), off, "off={off}");
        }
        // 无历史 → 无滚动条（备用屏）
        assert!(scrollbar_thumb(600., 24, 0, 0).is_none());
    }

    #[test]
    fn clicks_go_to_the_app_only_when_it_asked_for_the_mouse() {
        // 没开鼠标上报：点击是本地选区
        assert!(!click_goes_to_app(TermMode::empty(), false));
        // claude code 组合：1049 + 1000 + 1006
        let cc = TermMode::ALT_SCREEN | TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        assert!(click_goes_to_app(cc, false));
        assert!(
            !click_goes_to_app(cc, true),
            "Shift+点击绕过应用（xterm 惯例）"
        );
        // 1002 / 1003 单独开也算
        assert!(click_goes_to_app(TermMode::MOUSE_DRAG, false));
        assert!(click_goes_to_app(TermMode::MOUSE_MOTION, false));
        // 只有编码位没有上报位不算开；less/man 那种备用屏不抢点击
        assert!(!click_goes_to_app(TermMode::SGR_MOUSE, false));
        assert!(!click_goes_to_app(
            TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL,
            false
        ));
    }

    #[test]
    fn mouse_mods_follow_xterm_order_and_ignore_cmd() {
        let m = gpui::Modifiers {
            shift: true,
            control: true,
            ..Default::default()
        };
        assert_eq!(mouse_mods(&m), (true, false, true));
        let m = gpui::Modifiers {
            alt: true,
            platform: true,
            ..Default::default()
        };
        assert_eq!(mouse_mods(&m), (false, true, false), "cmd 不进鼠标协议");
    }

    #[test]
    fn selection_wins_over_link() {
        // 拖选结束时松手：复制，绝不顺带开链接
        assert_eq!(resolve_mouse_up(true, true), MouseUpAction::Copy);
        assert_eq!(resolve_mouse_up(true, false), MouseUpAction::Copy);
        // 单击（空选区）落在链接上才算点链接
        assert_eq!(resolve_mouse_up(false, true), MouseUpAction::OpenLink);
        assert_eq!(resolve_mouse_up(false, false), MouseUpAction::Nothing);
    }
}
