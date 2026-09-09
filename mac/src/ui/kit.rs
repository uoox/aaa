//! 共享 UI 小件：颜色转换、状态点、按钮底座。颜色一律是设计令牌常量
//! （`theme::XXX`），实心按钮上的字色由底色亮度推出（`theme::text_on`）。

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
        .border_color(c(theme::EDGE_LIGHT))
        .bg(c(theme::SURFACE_RAISED))
        .text_size(px(12.))
        .text_color(c(theme::INK))
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::ACCENT)))
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
        .bg(c(theme::ACCENT))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(theme::ON_ACCENT))
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
        .border_color(c(theme::EDGE_LIGHT))
        .bg(c(theme::SURFACE_RAISED))
        .text_size(px(12.5))
        .text_color(c(theme::INK))
        .cursor_pointer()
        .hover(|s| s.border_color(c(theme::DIM)))
        .child(label.into())
}

/// 危险按钮
pub fn btn_danger(id: impl Into<gpui::ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(14.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(c(theme::RED))
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(theme::text_on(theme::RED)))
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
}

// ── 侧栏行 ──────────────────────────────────────────────────────────────

const MARK_W: f32 = 2.0;
const MARK_H: f32 = 14.0;

/// 竖线本体：看板卡片标题前「在跑」那一根。项目行 2026-09-10 起不再画它——
/// 行的状态改成整行淡底（`theme::ROW_RUNNING` / `ROW_UNREAD`），选中是标题下划线。
pub fn mark_bar(color: u32) -> Div {
    div().flex_none().w(px(MARK_W)).h(px(MARK_H)).rounded(px(MARK_W / 2.)).bg(c(color))
}

/// 侧栏一行的底子：项目行和终端行共用——同一套内边距和圆角，才看得出是平级的。
pub fn sidebar_row(id: gpui::ElementId) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(10.))
        .py(px(5.))
        .mx(px(6.))
        .rounded(px(6.))
        .cursor_pointer()
}

/// 侧栏行尾的悬停小按钮（项目行的「删 / ✕」、终端行的「✕」）：
/// 三处骨架逐字相同——非当前行平时不画（invisible 连命中盒一起去掉），
/// 整行悬停才现身。`always_visible` 是「这是当前行」。
///
/// 只收骨架：hover 配色各处不同（红 = 终止 / 删），
/// 硬塞成参数就得再传两个颜色，不如让调用方接着 `.hover(..)` 写清楚。
pub fn row_btn(id: impl Into<gpui::ElementId>, always_visible: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(3.))
        .rounded(px(4.))
        .text_size(px(10.))
        .text_color(c(theme::FAINT))
        .when(!always_visible, |el| {
            el.invisible().group_hover("sb-row", |st| st.visible())
        })
}

// ── 文字与容器 ──────────────────────────────────────────────────────────

/// 元信息小字：段标题、版本号、时间、路径这类「不是内容、只是标注」的一行。
/// 设置页、详情面板、看板、侧栏终端小节共十处逐字相同（详情面板原来还留着
/// 「与设置页同款」的注释），字号/字族/颜色一次定死。
///
/// 不带 `.child()`：一半调用点后面还要接 `.pb()` / `.pr()` / `.truncate()`，
/// 收进来就得再加一串参数。
pub fn meta() -> Div {
    div()
        .font_family("Menlo")
        .text_size(px(10.))
        .text_color(c(theme::FAINT))
}

/// 卡片底：设置页三个小节和看板卡片共用的「surface 底 + edge 描边 + 10 圆角」。
/// 内边距和内部排布各处不同（设置页 14、看板 12 且要 id），所以不收进来。
pub fn card() -> Div {
    div()
        .rounded(px(10.))
        .bg(c(theme::SURFACE))
        .border_1()
        .border_color(c(theme::EDGE))
}
