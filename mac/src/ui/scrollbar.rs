//! 竖滚动条：gpui 的 `overflow_y_scroll()` 只管滚，不画任何东西——2026-09-08 用户报
//! 「看板这边要加个滑动条」，说的就是这个：瀑布流全展开之后一屏装不下，滚起来完全
//! 不知道自己在哪儿、还剩多少。
//!
//! 做法是一层薄的：可滚区照旧是那个 `overflow_y_scroll` 的 div，只是外面套一个
//! `relative()` 的壳，右缘绝对定位一根 thumb。几何全部从 [`gpui::ScrollHandle`] 上
//! 一帧一帧读（`bounds()` = 视口、`max_offset().y` = 还能滚多少、`offset().y` = 已经
//! 滚了多少，向下为负），所以没有第二份状态要维护；拖拽时反过来 `set_offset`。
//!
//! 用 [`scroll_area`] 一句话套上，`Scrollbar` 只存在于 view 里（拖拽中抓住点的偏移）。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, Div, ElementId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, ScrollHandle,
    Stateful, canvas, div, fill, point, prelude::*, px, size,
};

use super::kit::ca;
use crate::theme;

/// thumb 的宽与最小高：细到不占地方，短到最长的内容也还抓得住
const THUMB_W: f32 = 6.0;
const MIN_THUMB_H: f32 = 28.0;
/// 命中带：thumb 只有 6px 宽，指针差一点点也算按在条上
const HIT_W: f32 = 14.0;

/// thumb 几何：返回 (top, height)，px、相对视口顶部。内容装得下就没有条。
fn thumb_geometry(view_h: f32, max_offset: f32, scrolled: f32) -> Option<(f32, f32)> {
    if view_h <= 0. || max_offset <= 0.5 {
        return None;
    }
    let content_h = view_h + max_offset;
    let h = (view_h * view_h / content_h).clamp(MIN_THUMB_H.min(view_h), view_h);
    let top = (view_h - h) * (scrolled / max_offset).clamp(0., 1.);
    Some((top, h))
}

/// 拖拽反解：thumb 顶部 y → 滚动量（0 = 顶部）
fn scrolled_for_thumb_top(view_h: f32, max_offset: f32, thumb_h: f32, top: f32) -> f32 {
    let span = view_h - thumb_h;
    if span <= 0. {
        return 0.;
    }
    (top / span * max_offset).clamp(0., max_offset)
}

/// 一个可滚区的滚动条状态：滚动句柄 + 拖拽中抓住点相对 thumb 顶部的偏移。
/// 放进 view 里（每个可滚区一个），渲染时 clone 进闭包。
#[derive(Clone, Default)]
pub struct Scrollbar {
    pub handle: ScrollHandle,
    grab: Rc<Cell<Option<f32>>>,
}

impl Scrollbar {
    /// 当前 thumb 几何（相对视口顶部）
    fn thumb(&self) -> Option<(f32, f32)> {
        thumb_geometry(
            f32::from(self.handle.bounds().size.height),
            f32::from(self.handle.max_offset().y),
            -f32::from(self.handle.offset().y),
        )
    }

    /// 把 thumb 顶部挪到 `top`（视口坐标）并滚过去
    fn scroll_to_thumb_top(&self, top: f32) {
        let Some((_, h)) = self.thumb() else { return };
        let view_h = f32::from(self.handle.bounds().size.height);
        let max = f32::from(self.handle.max_offset().y);
        let scrolled = scrolled_for_thumb_top(view_h, max, h, top);
        self.handle
            .set_offset(point(self.handle.offset().x, px(-scrolled)));
    }

    /// 按下：点在 thumb 上就抓住它，点在轨道上先把 thumb 中心跳过来再进入拖拽
    fn press(&self, y_in_view: f32) {
        let Some((top, h)) = self.thumb() else { return };
        let grab = if y_in_view >= top && y_in_view <= top + h { y_in_view - top } else { h / 2. };
        self.grab.set(Some(grab));
        self.scroll_to_thumb_top(y_in_view - grab);
    }

    fn drag_to(&self, y_in_view: f32) {
        if let Some(grab) = self.grab.get() {
            self.scroll_to_thumb_top(y_in_view - grab);
        }
    }

    /// 拖拽结束（松手，或在别处松的手）；返回「刚才确实在拖」
    fn release(&self) -> bool {
        self.grab.take().is_some()
    }
}

/// 可滚区 + 右缘滚动条。`content` 是滚动内容，`id` 给外壳。
///
/// thumb 画在一个 canvas 里而不是普通的绝对定位 div：几何要读 `handle` 的 `bounds` /
/// `max_offset`，这两个值是**这一帧的 prepaint** 里才写进去的。canvas 排在可滚区之后，
/// 它的 paint 也就在之后——同一帧就能画对；用 div 则要等下一帧才追上，而「内容变长了」
/// 本身不会引发下一帧，条会一直不出现。canvas 不建命中盒，压在内容上也不挡点击。
///
/// 鼠标事件挂在**外壳**上而不是那根细条上：拖着 thumb 往左边甩出去是常事，gpui 只把
/// move 事件给悬停的元素，挂在条上一出界就断。外壳不拦事件（不 stop_propagation），
/// 按下点不在右缘那条命中带里就当没看见，底下的内容照旧点得着——所以调用方要让
/// 可滚区右缘留出这条带子（看板就是把 16px 的页边距留在滚动区里）。
pub fn scroll_area(
    id: impl Into<ElementId>,
    sb: &Scrollbar,
    content: impl IntoElement,
) -> Stateful<Div> {
    let (down, mv, up, paint) = (sb.clone(), sb.clone(), sb.clone(), sb.clone());
    // 每个改了 offset 的地方都要 window.refresh()：改 ScrollHandle 只是改一个 Rc 里的数，
    // gpui 不知道有东西变了——鼠标事件本身只在命中盒 / hover 翻转时才引发重画，所以少了
    // 这一下，按住 thumb 拖动时画面完全不动，松手（或指针跨过某个元素边界）才跳过去。
    div()
        .id(id)
        .relative()
        .size_full()
        .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, _| {
            let b = down.handle.bounds();
            if f32::from(ev.position.x) >= f32::from(b.origin.x + b.size.width) - HIT_W {
                down.press(f32::from(ev.position.y - b.origin.y));
                window.refresh();
            }
        })
        .on_mouse_move(move |ev: &MouseMoveEvent, window, _| {
            if ev.pressed_button != Some(MouseButton::Left) {
                if mv.release() {
                    window.refresh(); // 在别处松的手：thumb 要回到不拖的颜色
                }
                return;
            }
            if mv.grab.get().is_some() {
                mv.drag_to(f32::from(ev.position.y - mv.handle.bounds().origin.y));
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, window, _| {
            if up.release() {
                window.refresh();
            }
        })
        .child(
            div()
                .id("scroll-body")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&sb.handle)
                .child(content),
        )
        .child(
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let Some((top, h)) = paint.thumb() else { return };
                    let x = bounds.origin.x + bounds.size.width - px(THUMB_W + 2.);
                    window.paint_quad(
                        fill(
                            Bounds::new(point(x, bounds.origin.y + px(top)), size(px(THUMB_W), px(h))),
                            ca(theme::DIM, if paint.grab.get().is_some() { 0.85 } else { 0.4 }),
                        )
                        .corner_radii(px(THUMB_W / 2.)),
                    );
                },
            )
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(HIT_W)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bar_when_everything_fits() {
        assert!(thumb_geometry(400., 0., 0.).is_none());
        assert!(thumb_geometry(0., 100., 0.).is_none(), "还没量到视口");
    }

    #[test]
    fn thumb_tracks_the_offset_and_never_leaves_the_track() {
        // 视口 400、还能滚 400 → 内容 800，thumb 占一半
        let (top, h) = thumb_geometry(400., 400., 0.).unwrap();
        assert_eq!((top, h), (0., 200.));
        let (top, _) = thumb_geometry(400., 400., 400.).unwrap();
        assert_eq!(top, 200., "滚到底 thumb 贴底");
        let (top, _) = thumb_geometry(400., 400., 200.).unwrap();
        assert_eq!(top, 100.);
        // 内容极长时 thumb 不小于 MIN_THUMB_H，滚到底仍贴底
        let (top, h) = thumb_geometry(400., 100_000., 100_000.).unwrap();
        assert_eq!(h, MIN_THUMB_H);
        assert_eq!(top, 400. - MIN_THUMB_H);
    }

    #[test]
    fn dragging_the_thumb_round_trips() {
        let (view_h, max) = (400., 400.);
        for scrolled in [0., 37., 200., 400.] {
            let (top, h) = thumb_geometry(view_h, max, scrolled).unwrap();
            assert!((scrolled_for_thumb_top(view_h, max, h, top) - scrolled).abs() < 0.01);
        }
        // 拖出轨道两头都夹住
        let (_, h) = thumb_geometry(view_h, max, 0.).unwrap();
        assert_eq!(scrolled_for_thumb_top(view_h, max, h, -999.), 0.);
        assert_eq!(scrolled_for_thumb_top(view_h, max, h, 999.), max);
    }
}
