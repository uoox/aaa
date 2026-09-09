//! 极简单行文本输入（gpui 无内置 input）：
//! EntityInputHandler 走 macOS IME 通道（中文可用）。编辑核心是纯的 [`Editor`]
//! （光标 + 选区 + 增删），不碰 gpui，单测直接跑。
//!
//! 2026-09-07 之前只有光标没有选区：⌘X / ⌘C / ⌘A / Shift+方向键统统不存在，
//! 用户报「新建项目的框不能剪切」就是这个原因。现在：Shift+← → Home End 拉选区，
//! ⌘A 全选，⌘C 复制（没选区就复制整行），⌘X 剪切选区，⌘V 粘贴替换选区，打字 /
//! 退格 / 输入法组字都先吃掉选区。
//!
//! 2026-09-10 用户：「输入框最好也可以用 command/ctrl+A/V/C」——⌘ 那套一直在，Ctrl 那套
//! 补上（从 Linux / Windows 过来的手指记的是 Ctrl；这里是文本框不是终端，Ctrl-C 没有
//! 中断可言）。处理掉的快捷键顺手 `stop_propagation`：不然键等价事件一路冒到窗口都算
//! 「没人要」，macOS 会给一声提示音。
//!
//! 2026-09-08 用户报「消息流的输入框很奇怪，无法选中文字，无法通过鼠标移动光标」：
//! 那之前鼠标只有两个动作——单击收起选区、双击全选，**按哪儿都一样**，因为
//! 画字的 canvas 里拿得到字形位置，事件回调里拿不到。现在每帧把 shape 出来的
//! [`ShapedLine`] 连同它的左缘与横向滚动量记在 [`Metrics`] 里（`Rc<RefCell<…>>`，
//! 渲染闭包写、事件回调读），鼠标就能按 x 反查字节下标：**单击定位光标**（按住
//! Shift 是拉到这儿）、**按住拖拽拉选区**、**双击选一个词**、**三击全选**。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseMoveEvent, Pixels, SharedString,
    ShapedLine, UTF16Selection, Window, canvas, div, fill, point, prelude::*, px, size,
};

use super::kit::{c, ca};
use crate::theme;

/// 文本超出字段宽度时的水平偏移：优先保证光标可见，其次不在右边留白。
/// （光标右侧留一个字符的余量，否则光标会压在边框上看不见。）
fn text_scroll_shift(text_w: f32, caret: f32, view_w: f32) -> f32 {
    if text_w <= view_w {
        return 0.0;
    }
    (caret + 8.0 - view_w).clamp(0.0, text_w - view_w)
}

/// 单行编辑核心：字节偏移的光标，`anchor` 是选区另一端（None = 无选区）。
/// 所有偏移都落在 char 边界上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Editor {
    pub text: String,
    pub cursor: usize,
    pub anchor: Option<usize>,
}

impl Editor {
    pub fn with_text(text: impl Into<String>) -> Self {
        let text = text.into();
        Editor { cursor: text.len(), text, anchor: None }
    }

    /// 选区（左小右大）；anchor 与光标重合不算选区
    pub fn selection(&self) -> Option<Range<usize>> {
        let a = self.anchor?;
        if a == self.cursor {
            return None;
        }
        Some(a.min(self.cursor)..a.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().and_then(|r| self.text.get(r))
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|ch| self.cursor + ch.len_utf8())
            .unwrap_or(self.cursor)
    }

    /// 移到 `pos`。`extend`（按着 Shift）= 拉选区，否则收起选区
    fn move_to(&mut self, pos: usize, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
        } else {
            self.anchor = None;
        }
        self.cursor = pos.min(self.text.len());
    }

    pub fn left(&mut self, extend: bool) {
        match (extend, self.selection()) {
            // 有选区、不按 Shift：← 收到选区左端（macOS 惯例）
            (false, Some(r)) => self.move_to(r.start, false),
            _ => {
                let p = self.prev_boundary();
                self.move_to(p, extend)
            }
        }
    }

    pub fn right(&mut self, extend: bool) {
        match (extend, self.selection()) {
            (false, Some(r)) => self.move_to(r.end, false),
            _ => {
                let p = self.next_boundary();
                self.move_to(p, extend)
            }
        }
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.text.len(), extend);
    }

    pub fn select_all(&mut self) {
        if self.text.is_empty() {
            return;
        }
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    /// 删掉选区；没有选区返回 false 什么都不做
    pub fn delete_selection(&mut self) -> bool {
        let Some(r) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(r.clone(), "");
        self.cursor = r.start;
        self.anchor = None;
        true
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor == 0 {
            return false;
        }
        let p = self.prev_boundary();
        self.text.replace_range(p..self.cursor, "");
        self.cursor = p;
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.cursor >= self.text.len() {
            return false;
        }
        let n = self.next_boundary();
        self.text.replace_range(self.cursor..n, "");
        true
    }

    /// 在光标处插入（有选区先吃掉选区）——打字、粘贴都走这里
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// 把 `range` 换成 `s`（输入法 / 系统给的绝对范围），光标落在新文本末尾
    pub fn replace_range(&mut self, range: Range<usize>, s: &str) {
        self.anchor = None;
        self.text.replace_range(range.clone(), s);
        self.cursor = range.start + s.len();
    }

    /// 剪切：取走选区文字；没有选区返回 None（macOS 惯例，不动整行）
    pub fn cut(&mut self) -> Option<String> {
        let s = self.selected_text()?.to_string();
        self.delete_selection();
        Some(s)
    }

    /// 复制：选区，没有选区就是整行（这种小框里常见的就是「把 token 抄走」）
    pub fn copy(&self) -> Option<String> {
        match self.selected_text() {
            Some(s) => Some(s.to_string()),
            None if !self.text.is_empty() => Some(self.text.clone()),
            None => None,
        }
    }

    /// 落回最近的 char 边界（往左退）。鼠标反查来的下标由字形给出，本该已经在边界上；
    /// 这里兜一道，多字节字符上切一刀就是 panic。
    fn boundary(&self, i: usize) -> usize {
        let mut i = i.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    /// 鼠标点在 `pos`（字节偏移）：定位光标，`extend`（按着 Shift）= 从原处拉到这儿
    pub fn click_at(&mut self, pos: usize, extend: bool) {
        let pos = self.boundary(pos);
        self.move_to(pos, extend);
    }

    /// 双击：选中 `pos` 处的一个「词」。三类字符各成一片——字母数字（含中日韩，
    /// `is_alphanumeric` 覆盖）、空白、其余标点；点在两片之间取右边那片（末尾取左边）。
    pub fn select_word_at(&mut self, pos: usize) {
        if self.text.is_empty() {
            return;
        }
        let pos = self.boundary(pos);
        let class = |c: char| {
            if c.is_alphanumeric() || c == '_' {
                2
            } else if c.is_whitespace() {
                1
            } else {
                0
            }
        };
        // 点在末尾就看左边那个字符，否则看右边那个
        let here = self.text[pos..].chars().next().or_else(|| self.text[..pos].chars().next_back());
        let Some(k) = here.map(class) else { return };
        let mut start = pos;
        for (i, ch) in self.text[..pos].char_indices().rev() {
            if class(ch) != k {
                break;
            }
            start = i;
        }
        let mut end = pos;
        for (i, ch) in self.text[pos..].char_indices() {
            if class(ch) != k {
                break;
            }
            end = pos + i + ch.len_utf8();
        }
        if start == end {
            return; // 不该发生（点在末尾时取的是左边那片），真发生了就当没点
        }
        self.anchor = Some(start);
        self.cursor = end;
    }
}

/// 这一帧画出来的那行字：反查「鼠标点在第几个字节」要的全部东西。
/// 渲染闭包（canvas 的 paint）写，鼠标回调读——两边都在主线程，`Rc<RefCell<…>>` 足够。
struct Metrics {
    line: ShapedLine,
    /// 文本区左缘（窗口坐标）
    left: Pixels,
    /// 长文本时的横向滚动量（见 [`text_scroll_shift`]）
    shift: Pixels,
}

pub struct MiniInput {
    pub ed: Editor,
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    pub focus_handle: FocusHandle,
    /// 上一帧的字形位置；空文本（画的是 placeholder）为 None
    metrics: Rc<RefCell<Option<Metrics>>>,
    /// 鼠标按住拖选中
    dragging: bool,
}

/// 单行框的事件：输入法以文本形式送来的回车 = 提交（键盘回车由根节点直接接）
pub enum InputEvent {
    Submit,
}

impl gpui::EventEmitter<InputEvent> for MiniInput {}

impl MiniInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        MiniInput {
            ed: Editor::default(),
            marked: None,
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
            metrics: Rc::new(RefCell::new(None)),
            dragging: false,
        }
    }

    /// 窗口坐标 x → 字节偏移；这一帧还没画过（或框是空的）就没有答案
    fn index_at(&self, x: Pixels) -> Option<usize> {
        let m = self.metrics.borrow();
        let m = m.as_ref()?;
        Some(self.ed.boundary(m.line.closest_index_for_x(x - m.left + m.shift)))
    }

    pub fn text(&self) -> &str {
        &self.ed.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.ed = Editor::with_text(text);
        self.marked = None;
        cx.notify();
    }

    pub fn composing(&self) -> bool {
        self.marked.is_some()
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // IME 组字中不干预
        }
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        // ⌘ 与 Ctrl 同义（2026-09-10 用户要求）
        let shortcut = m.platform || m.control;
        let changed = match ks.key.as_str() {
            "backspace" => self.ed.backspace(),
            "delete" => self.ed.delete_forward(),
            // ⌘← / ⌘→ = 行首 / 行尾（macOS 惯例）
            "left" if m.platform => {
                self.ed.home(m.shift);
                true
            }
            "right" if m.platform => {
                self.ed.end(m.shift);
                true
            }
            "left" => {
                self.ed.left(m.shift);
                true
            }
            "right" => {
                self.ed.right(m.shift);
                true
            }
            "home" => {
                self.ed.home(m.shift);
                true
            }
            "end" => {
                self.ed.end(m.shift);
                true
            }
            "a" if shortcut => {
                self.ed.select_all();
                true
            }
            "c" if shortcut => {
                if let Some(s) = self.ed.copy() {
                    cx.write_to_clipboard(ClipboardItem::new_string(s));
                }
                false
            }
            "x" if shortcut => match self.ed.cut() {
                Some(s) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(s));
                    true
                }
                None => false,
            },
            "v" if shortcut => {
                if let Some(item) = cx.read_from_clipboard()
                    && let Some(text) = item.text()
                {
                    let t: String = text.replace('\n', " ");
                    self.ed.insert(&t);
                    true
                } else {
                    false
                }
            }
            _ => false,
        };
        if changed {
            cx.notify();
        }
        // 这框吃掉的键不再往上冒（复制没改文本，但也是吃掉了）
        if changed || (shortcut && matches!(ks.key.as_str(), "a" | "c" | "x" | "v")) {
            cx.stop_propagation();
        }
    }

    // ── UTF-16 偏移换算 ──────────────────────────────────────────────────

    fn offset_to_utf16(&self, byte: usize) -> usize {
        self.ed.text[..byte.min(self.ed.text.len())]
            .chars()
            .map(|c| c.len_utf16())
            .sum()
    }

    fn offset_from_utf16(&self, u16_off: usize) -> usize {
        let mut acc = 0usize;
        for (i, ch) in self.ed.text.char_indices() {
            if acc >= u16_off {
                return i;
            }
            acc += ch.len_utf16();
        }
        self.ed.text.len()
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

impl EntityInputHandler for MiniInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _adjusted: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_utf16(&range_utf16);
        self.ed.text.get(r).map(|s| s.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        match self.ed.selection() {
            Some(r) => Some(UTF16Selection {
                range: self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end),
                reversed: self.ed.anchor.is_some_and(|a| a > self.ed.cursor),
            }),
            None => {
                let c = self.offset_to_utf16(self.ed.cursor);
                Some(UTF16Selection { range: c..c, reversed: false })
            }
        }
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 输入法把回车当文本送进来（中文输入法确认后的那一下常是这样）：单行框没有
        // 换行可言，这就是「提交」——发事件给根节点，和键盘回车同一个出口
        if matches!(text, "\n" | "\r" | "\r\n") {
            self.marked = None;
            cx.emit(InputEvent::Submit);
            return;
        }
        // 组字中 → 替换组字区；系统给了范围 → 按范围；否则 = 普通打字，吃掉选区插入
        match self.marked.take().or_else(|| range_utf16.as_ref().map(|r| self.range_from_utf16(r))) {
            Some(range) => self.ed.replace_range(range, text),
            None => self.ed.insert(text),
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self
            .marked
            .take()
            .or_else(|| range_utf16.as_ref().map(|r| self.range_from_utf16(r)))
            .or_else(|| self.ed.selection())
            .unwrap_or(self.ed.cursor..self.ed.cursor);
        self.ed.replace_range(range.clone(), new_text);
        self.marked = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<gpui::Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Focusable for MiniInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MiniInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focused = self.focus_handle.is_focused(window);
        let handle = self.focus_handle.clone();
        let text = self.ed.text.clone();
        let placeholder = self.placeholder.clone();
        let cursor_byte = self.ed.cursor;
        let selection = self.ed.selection();
        let marked = self.marked.clone();
        let metrics = self.metrics.clone();

        div()
            .id("mini-input")
            .h(px(28.))
            .w_full()
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                c(theme::ACCENT)
            } else {
                c(theme::EDGE_LIGHT)
            })
            .bg(c(theme::INSET))
            .cursor_text()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            // 单击定位光标（Shift = 拉到这儿）、双击选词、三击全选；按住不放接着拖就是拉选区
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &gpui::MouseDownEvent, window, cx| {
                this.focus_handle.focus(window, cx);
                let at = this.index_at(ev.position.x);
                match (ev.click_count, at) {
                    (1, Some(i)) => {
                        this.ed.click_at(i, ev.modifiers.shift);
                        this.dragging = true;
                    }
                    (2, Some(i)) => this.ed.select_word_at(i),
                    // 三击 = 整行；空框 / 还没画过就只收起选区
                    (n, _) if n >= 3 => this.ed.select_all(),
                    _ => this.ed.anchor = None,
                }
                cx.notify();
            }))
            // 拖到哪儿选到哪儿。指针出了框就收不到 move 了（gpui 只把事件给悬停的元素），
            // 松开时的位置因此不一定是最后一次 move——够用：框就这么宽，里面拖得完。
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if !this.dragging {
                    return;
                }
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.dragging = false;
                    return;
                }
                if let Some(i) = this.index_at(ev.position.x) {
                    this.ed.click_at(i, true);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.dragging = false))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        window.handle_input(&handle, ElementInputHandler::new(bounds, entity), cx);
                        let font_size = px(12.5);
                        let empty = text.is_empty();
                        let display: SharedString = if empty {
                            placeholder.clone()
                        } else {
                            text.clone().into()
                        };
                        let color = if empty { theme::FAINT } else { theme::INK };
                        let runs = [gpui::TextRun {
                            len: display.len(),
                            font: gpui::font("Menlo"),
                            color: c(color).into(),
                            background_color: None,
                            underline: marked.as_ref().map(|_| gpui::UnderlineStyle {
                                thickness: px(1.),
                                color: Some(c(theme::ACCENT).into()),
                                wavy: false,
                            }),
                            strikethrough: None,
                        }];
                        let line =
                            window
                                .text_system()
                                .shape_line(display, font_size, &runs, None);
                        let line_h = bounds.size.height;
                        let caret = if empty {
                            px(0.)
                        } else {
                            line.x_for_index(cursor_byte)
                        };
                        // canvas 不受父元素 overflow 约束：不自己裁，超宽文本会一路画到
                        // 相邻控件上去（设置页 40 字符的 token 就这么盖住过「连接」按钮）。
                        // 裁的同时按光标滚动，否则长值只能看见开头、根本没法编辑。
                        let shift = px(text_scroll_shift(
                            f32::from(line.width),
                            f32::from(caret),
                            f32::from(bounds.size.width),
                        ));
                        // 鼠标反查要的就是这三样（空框画的是 placeholder，不能拿它反查）
                        *metrics.borrow_mut() = (!empty).then(|| Metrics {
                            line: line.clone(),
                            left: bounds.origin.x,
                            shift,
                        });
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            let origin =
                                point(bounds.origin.x - shift, bounds.origin.y + px(6.));
                            // 选区底色画在文字下面
                            if let Some(r) = selection.as_ref().filter(|_| !empty) {
                                let x0 = line.x_for_index(r.start);
                                let x1 = line.x_for_index(r.end);
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(bounds.origin.x + x0 - shift, bounds.origin.y + px(5.)),
                                        size(x1 - x0, bounds.size.height - px(10.)),
                                    ),
                                    ca(theme::ACCENT, if focused { 0.30 } else { 0.16 }),
                                ));
                            }
                            let _ = line.paint(
                                origin,
                                line_h - px(12.),
                                gpui::TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                            if focused {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(
                                            bounds.origin.x + caret - shift,
                                            bounds.origin.y + px(5.),
                                        ),
                                        size(px(1.5), bounds.size.height - px(10.)),
                                    ),
                                    c(theme::ACCENT),
                                ));
                            }
                        });
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_never_scrolls() {
        assert_eq!(text_scroll_shift(50., 30., 100.), 0.);
    }

    #[test]
    fn long_text_follows_caret() {
        // 光标在末尾：右边不留白
        assert_eq!(text_scroll_shift(300., 300., 100.), 200.);
        // 光标在中间：光标可见
        assert_eq!(text_scroll_shift(300., 150., 100.), 58.);
        // 光标在开头：不滚
        assert_eq!(text_scroll_shift(300., 0., 100.), 0.);
    }

    // 2026-09-07 用户报「新建项目的框不能剪切」：以前根本没有选区
    #[test]
    fn shift_arrows_build_a_selection_and_cut_takes_it() {
        let mut e = Editor::with_text("新项目 abc");
        assert!(e.selection().is_none());
        e.left(true);
        e.left(true);
        assert_eq!(e.selected_text(), Some("bc"));
        assert_eq!(e.cut().as_deref(), Some("bc"));
        assert_eq!(e.text, "新项目 a");
        assert!(e.selection().is_none() && e.cursor == e.text.len());
        // 没选区：剪切什么都不做（macOS 惯例），复制拿整行
        assert_eq!(e.cut(), None);
        assert_eq!(e.copy().as_deref(), Some("新项目 a"));
    }

    #[test]
    fn select_all_then_type_replaces_everything() {
        let mut e = Editor::with_text("旧名字");
        e.select_all();
        assert_eq!(e.selected_text(), Some("旧名字"));
        e.insert("n");
        assert_eq!(e.text, "n");
        assert_eq!(e.cursor, 1);
        // 空文本全选无事发生
        let mut empty = Editor::default();
        empty.select_all();
        assert!(empty.selection().is_none());
    }

    #[test]
    fn arrows_collapse_selection_to_its_ends() {
        let mut e = Editor::with_text("abcd");
        e.home(false);
        e.right(true);
        e.right(true); // 选中 ab，光标在 2
        assert_eq!(e.selected_text(), Some("ab"));
        e.left(false); // 收到左端
        assert_eq!((e.cursor, e.selection()), (0, None));
        e.right(true);
        e.right(false); // 收到右端
        assert_eq!((e.cursor, e.selection()), (1, None));
        // Shift+End 从当前位置选到末尾；Home 不按 Shift 收起
        e.end(true);
        assert_eq!(e.selected_text(), Some("bcd"));
        e.home(false);
        assert!(e.selection().is_none() && e.cursor == 0);
    }

    #[test]
    fn backspace_and_delete_eat_the_selection_first() {
        let mut e = Editor::with_text("héllo");
        e.home(false);
        e.right(true);
        e.right(true); // 选中 hé（é 两字节，边界要对）
        assert!(e.backspace());
        assert_eq!(e.text, "llo");
        assert_eq!(e.cursor, 0);
        assert!(!e.backspace(), "开头退格没东西可删");
        e.end(false);
        assert!(!e.delete_forward(), "末尾 delete 没东西可删");
        e.left(false);
        assert!(e.delete_forward());
        assert_eq!(e.text, "ll");
    }

    // 2026-09-08 用户报「消息流的输入框无法选中文字、无法用鼠标移动光标」：
    // 鼠标按 x 反查到的字节偏移交给这两个方法，它们是纯的，这里直接验。
    #[test]
    fn click_places_the_caret_and_shift_click_extends() {
        let mut e = Editor::with_text("hello 世界");
        e.click_at(2, false);
        assert_eq!((e.cursor, e.selection()), (2, None));
        e.click_at(5, true); // Shift+点：从 2 拉到 5
        assert_eq!(e.selected_text(), Some("llo"));
        e.click_at(0, false); // 不按 Shift：收起选区
        assert_eq!((e.cursor, e.selection()), (0, None));
        // 落在多字节字符中间的下标退回边界，不 panic（"世" 在 6..9）
        e.click_at(7, false);
        assert_eq!(e.cursor, 6);
        e.click_at(999, false);
        assert_eq!(e.cursor, e.text.len());
    }

    #[test]
    fn double_click_selects_one_word() {
        let mut e = Editor::with_text("aaa bbb-ccc 中文字");
        e.select_word_at(1);
        assert_eq!(e.selected_text(), Some("aaa"));
        e.select_word_at(3); // 空白自成一片（macOS 惯例）
        assert_eq!(e.selected_text(), Some(" "));
        e.select_word_at(5);
        assert_eq!(e.selected_text(), Some("bbb"));
        e.select_word_at(7); // 标点自成一片
        assert_eq!(e.selected_text(), Some("-"));
        e.select_word_at(12); // 中日韩按 is_alphanumeric 归到「字」那一片
        assert_eq!(e.selected_text(), Some("中文字"));
        e.select_word_at(e.text.len()); // 点在末尾：选左边那片
        assert_eq!(e.selected_text(), Some("中文字"));
        // 空框双击什么都不选
        let mut empty = Editor::default();
        empty.select_word_at(0);
        assert!(empty.selection().is_none());
    }

    #[test]
    fn paste_replaces_selection_and_ime_ranges_are_absolute() {
        let mut e = Editor::with_text("a[b]c");
        e.home(false);
        e.right(false);
        e.right(true);
        e.right(true);
        e.right(true); // 选中 [b]
        e.insert("X");
        assert_eq!(e.text, "aXc");
        // 输入法给的绝对范围：替换后光标在新文本末尾，选区清掉
        e.select_all();
        e.replace_range(1..2, "中文");
        assert_eq!(e.text, "a中文c");
        assert_eq!(e.cursor, 1 + "中文".len());
        assert!(e.selection().is_none());
    }
}
