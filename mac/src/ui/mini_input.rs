//! 极简单行文本输入（gpui 无内置 input）：
//! EntityInputHandler 走 macOS IME 通道（中文可用），光标移动/删除自理，无选区。

use std::ops::Range;

use gpui::{
    App, Bounds, ContentMask, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    Focusable, KeyDownEvent, MouseButton, SharedString, UTF16Selection, Window, canvas, div, fill,
    point, prelude::*, px, size,
};

use super::kit::c;
use crate::theme;

/// 文本超出字段宽度时的水平偏移：优先保证光标可见，其次不在右边留白。
/// （光标右侧留一个字符的余量，否则光标会压在边框上看不见。）
fn text_scroll_shift(text_w: f32, caret: f32, view_w: f32) -> f32 {
    if text_w <= view_w {
        return 0.0;
    }
    (caret + 8.0 - view_w).clamp(0.0, text_w - view_w)
}

pub struct MiniInput {
    pub text: String,
    cursor: usize, // 字节偏移
    marked: Option<Range<usize>>,
    placeholder: SharedString,
    pub focus_handle: FocusHandle,
}

impl MiniInput {
    pub fn new(cx: &mut Context<Self>, placeholder: impl Into<SharedString>) -> Self {
        MiniInput {
            text: String::new(),
            cursor: 0,
            marked: None,
            placeholder: placeholder.into(),
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.marked = None;
        cx.notify();
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|ch| self.cursor + ch.len_utf8())
            .unwrap_or(self.text.len())
    }

    /// IME 组字中（有 marked text）——根节点的回车快捷键要避开这个状态
    pub fn composing(&self) -> bool {
        self.marked.is_some()
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.is_some() {
            return; // IME 组字中不干预
        }
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        match ks.key.as_str() {
            "backspace" => {
                if self.cursor > 0 {
                    let p = self.prev_boundary();
                    self.text.replace_range(p..self.cursor, "");
                    self.cursor = p;
                    cx.notify();
                }
            }
            "delete" => {
                if self.cursor < self.text.len() {
                    let n = self.next_boundary();
                    self.text.replace_range(self.cursor..n, "");
                    cx.notify();
                }
            }
            "left" => {
                self.cursor = self.prev_boundary();
                cx.notify();
            }
            "right" => {
                self.cursor = self.next_boundary();
                cx.notify();
            }
            "home" => {
                self.cursor = 0;
                cx.notify();
            }
            "end" => {
                self.cursor = self.text.len();
                cx.notify();
            }
            "v" if m.platform => {
                if let Some(item) = cx.read_from_clipboard()
                    && let Some(text) = item.text()
                {
                    let t: String = text.replace('\n', " ");
                    self.text.insert_str(self.cursor, &t);
                    self.cursor += t.len();
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    // ── UTF-16 偏移换算 ──────────────────────────────────────────────────

    fn offset_to_utf16(&self, byte: usize) -> usize {
        self.text[..byte.min(self.text.len())]
            .chars()
            .map(|c| c.len_utf16())
            .sum()
    }

    fn offset_from_utf16(&self, u16_off: usize) -> usize {
        let mut acc = 0usize;
        for (i, ch) in self.text.char_indices() {
            if acc >= u16_off {
                return i;
            }
            acc += ch.len_utf16();
        }
        self.text.len()
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
        self.text.get(r).map(|s| s.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let c = self.offset_to_utf16(self.cursor);
        Some(UTF16Selection {
            range: c..c,
            reversed: false,
        })
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
        let range = self
            .marked
            .take()
            .or_else(|| range_utf16.as_ref().map(|r| self.range_from_utf16(r)))
            .unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
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
            .unwrap_or(self.cursor..self.cursor);
        self.text.replace_range(range.clone(), new_text);
        if new_text.is_empty() {
            self.marked = None;
        } else {
            self.marked = Some(range.start..range.start + new_text.len());
        }
        self.cursor = range.start + new_text.len();
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
        let text = self.text.clone();
        let placeholder = self.placeholder.clone();
        let cursor_byte = self.cursor;
        let marked = self.marked.clone();

        div()
            .id("mini-input")
            .h(px(28.))
            .w_full()
            .px(px(8.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                c(theme::accent())
            } else {
                c(theme::edge_light())
            })
            .bg(c(theme::inset()))
            .cursor_text()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus_handle.focus(window, cx);
                cx.notify();
            }))
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
                        let color = if empty { theme::faint() } else { theme::ink() };
                        let runs = [gpui::TextRun {
                            len: display.len(),
                            font: gpui::font("Menlo"),
                            color: c(color).into(),
                            background_color: None,
                            underline: marked.as_ref().map(|_| gpui::UnderlineStyle {
                                thickness: px(1.),
                                color: Some(c(theme::accent()).into()),
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
                        window.with_content_mask(Some(ContentMask { bounds }), |window| {
                            let origin =
                                point(bounds.origin.x - shift, bounds.origin.y + px(6.));
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
                                    c(theme::accent()),
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
        assert_eq!(text_scroll_shift(60.0, 60.0, 222.0), 0.0);
        assert_eq!(text_scroll_shift(222.0, 0.0, 222.0), 0.0);
    }

    #[test]
    fn long_text_follows_caret() {
        // 设置页 token 字段的真实数值：文本 293.5，可视 222
        let (text_w, view_w) = (293.5, 222.0);
        // 光标在行首 → 不滚
        assert_eq!(text_scroll_shift(text_w, 0.0, view_w), 0.0);
        // 光标在行尾 → 滚到底，文本右端贴齐字段右边缘（不多滚，右边不留白）
        assert_eq!(text_scroll_shift(text_w, text_w, view_w), text_w - view_w);
        // 光标在中间偏右 → 恰好把光标带进视野并留 8px 余量
        assert_eq!(text_scroll_shift(text_w, 250.0, view_w), 36.0);
    }
}
