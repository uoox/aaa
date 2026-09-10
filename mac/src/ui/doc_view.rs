//! v1.35 读一份产物：详情栏「产物」里点开一份项目 Markdown，正文铺在会话区。
//!
//! 这一屏是 v1.30 那个目录 explorer（会话的第三种看法「浏览」）的**剩下那一半**：
//! 2026-09-10 用户拍板不要文件管理器，要的是「项目生成的 Markdown 排进产物里，
//! 并且能读」。翻目录那半截整个删了，读文件这半截留下来——它本来就是那一屏唯一
//! 被用到的部分。
//!
//! 只读，且只在**项目根底下**——守卫在 daemon 那一层（`files.rs`），这边不重判一遍。
//! `.md` 按 CommonMark 渲染（与消息流同一套块渲染器），其余文本等宽 + 横向滚动，
//! 二进制只报大小。**任何文件都能「用默认程序打开」**——daemon 与 mac App 在同一台
//! 机器上，一个 `open <路径>` 就交给系统了：html 进浏览器、图片进预览、pdf 进 Preview。

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};

use super::kit::{c, ca};
use super::messages_view::{MdIds, md_blocks};
use super::scrollbar::{Scrollbar, scroll_area};
use crate::markdown::{self, Block};
use crate::model::FileBody;
use crate::net::Net;
use crate::theme;

pub struct DocView {
    net: Net,
    /// 项目目录：标题里的路径相对它显示
    root: String,
    /// 正在读的那份（None = 还没读到）
    open: Option<FileBody>,
    /// 是 markdown 时的解析结果
    md: Vec<Block>,
    error: Option<String>,
    loading: bool,
    scroll: Scrollbar,
}

/// 文件大小写成人话
pub fn human_size(n: u64) -> String {
    const K: f64 = 1024.;
    let n = n as f64;
    if n < K {
        return format!("{n:.0} B");
    }
    for (i, unit) in ["KB", "MB", "GB"].iter().enumerate() {
        let v = n / K.powi(i as i32 + 1);
        if v < K || i == 2 {
            return if v < 10. { format!("{v:.1} {unit}") } else { format!("{v:.0} {unit}") };
        }
    }
    unreachable!()
}

/// 标题那一行：项目底下的文件显示成 `项目名/子/文件.md`，不在底下就给全路径。
pub fn crumb(root: &str, path: &str) -> String {
    let base = root.rsplit('/').next().unwrap_or(root);
    match path.strip_prefix(root) {
        Some("") => base.to_string(),
        Some(rest) => format!("{base}{rest}"),
        None => path.to_string(),
    }
}

impl DocView {
    pub fn new(net: Net, root: String, path: String, cx: &mut Context<Self>) -> Self {
        let mut v = DocView {
            net,
            root,
            open: None,
            md: Vec::new(),
            error: None,
            loading: false,
            scroll: Scrollbar::default(),
        };
        v.open_file(path, cx);
        v
    }

    /// 换一份读（详情栏里点了另一行）
    pub fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.loading = true;
        let fut = self.net.file_read(&path);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut DocView, cx| {
                v.loading = false;
                match res {
                    Ok(b) => {
                        v.error = None;
                        v.md = if b.kind == "markdown" { markdown::parse(&b.text) } else { Vec::new() };
                        v.open = Some(b);
                        v.scroll.handle.set_offset(gpui::point(px(0.), px(0.)));
                    }
                    Err(e) => v.error = Some(short_err(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_root(&mut self, root: String) {
        if !root.is_empty() {
            self.root = root;
        }
    }

    // ── 画 ────────────────────────────────────────────────────────────────

    /// 顶栏：文件名（相对项目）+ 默认程序打开 + 刷新
    fn header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let label: SharedString = match &self.open {
            Some(f) => crumb(&self.root, &f.path).into(),
            None => "读取中…".into(),
        };
        let path = self.open.as_ref().map(|f| f.path.clone());
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .border_b_1()
            .border_color(c(theme::EDGE))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_family("Menlo")
                    .text_size(px(11.))
                    .text_color(c(theme::DIM))
                    .child(label),
            )
            .when_some(path.clone(), |el, path| {
                el.child(
                    div()
                        .id("doc-open-ext")
                        .px(px(6.))
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .text_color(c(theme::FAINT))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::SURFACE_RAISED)).text_color(c(theme::ACCENT)))
                        .on_click(cx.listener(move |v: &mut Self, _, _, cx| {
                            // 文件就在本机（daemon 和 App 同一台），交给系统默认程序
                            if let Err(e) = std::process::Command::new("open").arg(&path).spawn() {
                                v.error = Some(format!("打开失败：{e}"));
                                cx.notify();
                            }
                        }))
                        .child("默认程序打开"),
                )
            })
            .when_some(path, |el, path| {
                el.child(
                    div()
                        .id("doc-reload")
                        .px(px(6.))
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .text_color(c(theme::FAINT))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::SURFACE_RAISED)).text_color(c(theme::ACCENT)))
                        .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.open_file(path.clone(), cx)))
                        .child("刷新"),
                )
            })
    }

    fn body(&self, f: &FileBody, window: &Window) -> AnyElement {
        let body: AnyElement = if f.kind == "binary" {
            div()
                .p(px(16.))
                .text_size(px(12.))
                .text_color(c(theme::FAINT))
                .child(SharedString::from(format!(
                    "{}：二进制文件，{}",
                    f.name,
                    human_size(f.size)
                )))
                .into_any_element()
        } else if f.kind == "markdown" {
            let mut ids = MdIds { seq: 0, next: 0 };
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .px(px(16.))
                .py(px(12.))
                .children(md_blocks(&self.md, &mut ids, Some(window.text_system())))
                .into_any_element()
        } else {
            div()
                .id("doc-text")
                .w_full()
                .overflow_x_scroll()
                .px(px(16.))
                .py(px(12.))
                .child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .text_color(c(theme::TERM_FG))
                        .whitespace_nowrap()
                        .child(SharedString::from(f.text.clone())),
                )
                .into_any_element()
        };
        let mut col = div().flex().flex_col().child(body);
        if f.truncated {
            col = col.child(
                div()
                    .px(px(16.))
                    .py(px(8.))
                    .text_size(px(10.5))
                    .text_color(c(theme::AMBER))
                    .child(SharedString::from(format!(
                        "文件太大，只读了前面一段（共 {}）",
                        human_size(f.size)
                    ))),
            );
        }
        scroll_area("doc-body", &self.scroll, col).into_any_element()
    }
}

/// 网络错误只取第一行：整串 anyhow 链在一行小字里读不出东西
fn short_err(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or("读取失败").to_string()
}

impl Render for DocView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.open {
            Some(f) => self.body(f, window),
            None => div()
                .p(px(16.))
                .text_size(px(12.))
                .text_color(c(theme::FAINT))
                .child(if self.loading { "读取中…" } else { "没有内容" })
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(theme::BG))
            .child(self.header(cx))
            .when_some(self.error.clone(), |el, e| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(12.))
                        .py(px(5.))
                        .bg(ca(theme::RED, 0.1))
                        .text_size(px(11.))
                        .text_color(c(theme::RED))
                        .child(SharedString::from(e)),
                )
            })
            .child(div().flex_1().min_h(px(0.)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(812), "812 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(90_000), "88 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    /// 标题相对项目：底下的接在项目名后面，不在底下就给全路径。
    #[test]
    fn crumbs_are_relative_to_the_project() {
        assert_eq!(crumb("/p/aaa", "/p/aaa/NOTE.md"), "aaa/NOTE.md");
        assert_eq!(crumb("/p/aaa", "/p/aaa/docs/api.md"), "aaa/docs/api.md");
        assert_eq!(crumb("/p/aaa", "/elsewhere/x.md"), "/elsewhere/x.md", "不在底下就给全路径");
    }
}
