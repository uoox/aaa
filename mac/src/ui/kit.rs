//! 共享 UI 小件：颜色转换、状态点、按钮底座。颜色一律取自当前主题
//! （`theme::xxx()`），实心按钮上的字色由底色亮度推出（`theme::text_on`）。

use gpui::{Div, Rgba, SharedString, Stateful, div, prelude::*, px, rgb, rgba};

use crate::theme;

/// 0xRRGGBB → gpui Rgba
pub fn c(hex: u32) -> Rgba {
    rgb(hex)
}

/// 0xRRGGBB + alpha
pub fn ca(hex: u32, alpha: f32) -> Rgba {
    rgba((hex << 8) | ((alpha.clamp(0., 1.) * 255.0) as u32))
}

/// 状态点（绿=运行 黄=等待 灰=空闲 红=退出）
pub fn dot(color: u32) -> Div {
    div()
        .w(px(7.))
        .h(px(7.))
        .flex_none()
        .rounded_full()
        .bg(c(color))
}

/// 工具栏按钮底座（调用方自行加 .on_click）
pub fn tbtn(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::edge_light()))
        .bg(c(theme::surface_raised()))
        .text_size(px(12.))
        .text_color(c(theme::ink()))
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::accent())))
        .child(label.into())
}

/// 主操作按钮
pub fn btn_primary(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(c(theme::accent()))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(theme::on_accent()))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

/// 次要按钮
pub fn btn_secondary(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .border_1()
        .border_color(c(theme::edge_light()))
        .bg(c(theme::surface_raised()))
        .text_size(px(12.5))
        .text_color(c(theme::ink()))
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::dim())))
        .child(label.into())
}

/// 危险按钮
pub fn btn_danger(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(c(theme::red()))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(theme::text_on(theme::red())))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}
