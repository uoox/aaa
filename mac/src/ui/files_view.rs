//! v1.30 目录浏览：会话的第三种视图（消息流 / 终端 / 浏览，⌘E 轮换）。
//!
//! 只读，且只在**项目根底下**——守卫在 daemon 那一层（`files.rs`），这边不重判一遍。
//! 点目录进去，点文件打开：`.md` 直接按 CommonMark 渲染（与消息流同一套块渲染器），
//! 其余文本等宽显示，二进制只报大小。**任何文件都能「用默认程序打开」**——daemon 与
//! mac App 在同一台机器上，一个 `open <路径>` 就交给系统了：html 进浏览器、图片进
//! 预览、pdf 进 Preview。渲染 html 这件事 gpui 做不了，也不该做（那是个浏览器）。
//!
//! 打开的文件是**视图状态**而不是另一页：返回目录不重新拉列表，翻文件时目录还在原处。

use gpui::{AnyElement, Context, SharedString, Window, div, prelude::*, px};

use super::kit::{c, ca};
use super::messages_view::{MdIds, md_blocks};
use super::scrollbar::{Scrollbar, scroll_area};
use crate::markdown::{self, Block};
use crate::model::{FileBody, FileEntry};
use crate::net::Net;
use crate::theme;

pub struct FilesView {
    net: Net,
    /// 项目目录：路径面包屑相对它显示，也是「回项目根」那一下的落点
    root: String,
    dir: String,
    parent: Option<String>,
    entries: Vec<FileEntry>,
    truncated: bool,
    /// 正打开的文件（None = 在看目录）
    open: Option<FileBody>,
    /// 打开的是 markdown 时的解析结果
    md: Vec<Block>,
    error: Option<String>,
    loading: bool,
    list_scroll: Scrollbar,
    file_scroll: Scrollbar,
}

/// 文件大小写成人话：目录列表每行右端那一格
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

/// 面包屑：项目目录本身显示成它的名字，底下的显示成 `名字/子/孙`。
pub fn crumb(root: &str, dir: &str) -> String {
    let base = root.rsplit('/').next().unwrap_or(root);
    match dir.strip_prefix(root) {
        Some("") => base.to_string(),
        Some(rest) => format!("{base}{rest}"),
        // 不在项目目录底下（会话开在项目根本身之类）：直接给全路径，别猜
        None => dir.to_string(),
    }
}

impl FilesView {
    pub fn new(net: Net, dir: String, cx: &mut Context<Self>) -> Self {
        let mut v = FilesView {
            net,
            root: dir.clone(),
            dir,
            parent: None,
            entries: Vec::new(),
            truncated: false,
            open: None,
            md: Vec::new(),
            error: None,
            loading: false,
            list_scroll: Scrollbar::default(),
            file_scroll: Scrollbar::default(),
        };
        v.reload(cx);
        v
    }

    /// 会话换了项目（同一个视图被复用）时重新指到新目录
    pub fn set_dir(&mut self, dir: String, cx: &mut Context<Self>) {
        if dir.is_empty() || dir == self.root {
            return;
        }
        self.root = dir.clone();
        self.dir = dir;
        self.open = None;
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let fut = self.net.files(&self.dir);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut FilesView, cx| {
                v.loading = false;
                match res {
                    Ok(r) => {
                        v.error = None;
                        v.dir = r.path;
                        v.parent = r.parent;
                        v.entries = r.entries;
                        v.truncated = r.truncated;
                        v.list_scroll.handle.set_offset(gpui::point(px(0.), px(0.)));
                    }
                    Err(e) => v.error = Some(short_err(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn enter(&mut self, entry: &FileEntry, cx: &mut Context<Self>) {
        if entry.dir {
            self.dir = entry.path.clone();
            self.open = None;
            self.reload(cx);
        } else {
            self.open_file(entry.path.clone(), cx);
        }
    }

    fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        self.loading = true;
        let fut = self.net.file_read(&path);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut FilesView, cx| {
                v.loading = false;
                match res {
                    Ok(b) => {
                        v.error = None;
                        v.md = if b.kind == "markdown" { markdown::parse(&b.text) } else { Vec::new() };
                        v.open = Some(b);
                        v.file_scroll.handle.set_offset(gpui::point(px(0.), px(0.)));
                    }
                    Err(e) => v.error = Some(short_err(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.open = None;
            cx.notify();
        } else if let Some(p) = self.parent.clone() {
            self.dir = p;
            self.reload(cx);
        }
    }

    // ── 画 ────────────────────────────────────────────────────────────────

    /// 顶栏：上一级 + 路径 + 刷新。打开文件时路径那一格换成文件名。
    fn header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let can_up = self.open.is_some() || self.parent.is_some();
        let label: SharedString = match &self.open {
            Some(f) => format!("{} · {}", crumb(&self.root, &self.dir), f.name).into(),
            None => crumb(&self.root, &self.dir).into(),
        };
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
                    .id("files-up")
                    .px(px(6.))
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .text_color(c(if can_up { theme::ACCENT } else { theme::FAINT }))
                    .when(can_up, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(c(theme::SURFACE_RAISED)))
                            .on_click(cx.listener(|v: &mut Self, _, _, cx| v.up(cx)))
                    })
                    .child("↰ 上一级"),
            )
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
            .when_some(self.open.as_ref().map(|f| f.path.clone()), |el, path| {
                el.child(
                    div()
                        .id("files-open-ext")
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
            .child(
                div()
                    .id("files-reload")
                    .px(px(6.))
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(c(theme::FAINT))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::SURFACE_RAISED)).text_color(c(theme::ACCENT)))
                    .on_click(cx.listener(|v: &mut Self, _, _, cx| {
                        // 文件开着就重读这个文件，否则重列目录——「刷新」永远指当前看的东西
                        match v.open.as_ref().map(|f| f.path.clone()) {
                            Some(p) => v.open_file(p, cx),
                            None => v.reload(cx),
                        }
                    }))
                    .child("刷新"),
            )
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = div().flex().flex_col().py(px(4.));
        for (ix, e) in self.entries.iter().enumerate() {
            let entry = e.clone();
            let size: SharedString = if e.dir { "".into() } else { human_size(e.size).into() };
            col = col.child(
                div()
                    .id(("files-row", ix))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.enter(&entry, cx)))
                    .child(
                        div()
                            .flex_none()
                            .w(px(16.))
                            .text_size(px(11.))
                            .text_color(c(theme::FAINT))
                            .child(if e.dir { "▸" } else { "·" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(12.5))
                            .text_color(c(if e.dir {
                                theme::INK
                            } else if e.kind == "binary" {
                                theme::FAINT
                            } else {
                                theme::DIM
                            }))
                            .child(SharedString::from(e.name.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family("Menlo")
                            .text_size(px(10.))
                            .text_color(c(theme::FAINT))
                            .child(size),
                    ),
            );
        }
        if self.entries.is_empty() && !self.loading {
            col = col.child(
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .text_size(px(12.))
                    .text_color(c(theme::FAINT))
                    .child("空目录"),
            );
        }
        if self.truncated {
            col = col.child(
                div()
                    .px(px(12.))
                    .py(px(6.))
                    .text_size(px(10.5))
                    .text_color(c(theme::AMBER))
                    .child("条目太多，只列了前面一部分"),
            );
        }
        scroll_area("files-list", &self.list_scroll, col).into_any_element()
    }

    fn file(&self, f: &FileBody, window: &Window) -> AnyElement {
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
                .id("files-text")
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
        scroll_area("files-file", &self.file_scroll, col).into_any_element()
    }
}

/// 网络错误只取第一行：整串 anyhow 链在一行小字里读不出东西
fn short_err(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or("读取失败").to_string()
}

impl Render for FilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match &self.open {
            Some(f) => self.file(f, window),
            None => self.list(cx),
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

    /// 面包屑相对项目目录：根显示成项目名，底下的接在后面。
    #[test]
    fn crumbs_are_relative_to_the_project() {
        assert_eq!(crumb("/p/aaa", "/p/aaa"), "aaa");
        assert_eq!(crumb("/p/aaa", "/p/aaa/src/ui"), "aaa/src/ui");
        assert_eq!(crumb("/p/aaa", "/elsewhere"), "/elsewhere", "不在底下就给全路径");
    }
}
