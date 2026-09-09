//! 设置页（精简版）：① 配对二维码 + 连接信息 ② 可编辑配置（host/port/token/项目目录；
//! 保存写入 daemon 并触发它自我重启，项目目录变更先弹迁移确认）。
//! 外观一节 2026-09-10 拿掉：只剩一套主题，没有可切的。

use gpui::{
    Bounds, ClipboardItem, Context, SharedString, Window, canvas, div, fill, point, prelude::*, px,
    size,
};

use super::kit::*;
use super::{Modal, RootView};
use crate::net::ConnState;
use crate::theme;

/// 配对 payload → 二维码模块位图（fetch 时调用一次；渲染帧不重复编码）
pub(super) fn qr_encode(payload: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(payload.as_bytes()).ok()?;
    let w = code.width();
    let modules = code
        .to_colors()
        .iter()
        .map(|c| *c == qrcode::Color::Dark)
        .collect();
    Some((w, modules))
}

/// 设置页的一节：kit 的卡片底 + 这一页统一的 14 内边距
fn sect() -> gpui::Div {
    card().w_full().p(px(14.))
}

/// 一节的标题：元信息小字 + 与正文之间的留白
fn sect_title(text: &'static str) -> gpui::Div {
    meta().pb(px(8.)).child(text)
}

/// 连接信息里的一行「键 · 值」。值可以带颜色（SSD 未挂载是红的）。
fn kv_row(k: &'static str, v: String, color: Option<u32>) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(4.))
        .child(meta().w(px(64.)).flex_none().child(k))
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(c(color.unwrap_or(theme::INK)))
                .child(SharedString::from(v)),
        )
}

/// 配置区的一个输入格：一行标签压在输入框上面
fn field(label: &'static str, input: gpui::Entity<super::MiniInput>) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .font_family("Menlo")
                .text_size(px(9.5))
                .text_color(c(theme::FAINT))
                .child(label),
        )
        .child(input)
}

impl RootView {
    /// 「保存到 daemon」：目录变了先问迁移，其余直接提交（daemon 会自我重启）
    fn save_daemon_config(&mut self, cx: &mut Context<Self>) {
        // 不做 unwrap_or 静默兜底：填错端口却显示保存成功，比报错糟得多
        let port_text = self.port_input.read(cx).text().trim().to_string();
        let port: u16 = match port_text.parse() {
            Ok(p) if p > 0 => p,
            _ => {
                self.set_error(format!("端口无效：「{port_text}」（1–65535）"), cx);
                return;
            }
        };
        let token = self.token_input.read(cx).text().trim().to_string();
        if token.is_empty() {
            self.set_error("token 不能为空".into(), cx);
            return;
        }
        let new_root = self.root_input.read(cx).text().trim().to_string();
        let cur_root = self
            .health
            .as_ref()
            .map(|h| h.project_root.clone())
            .unwrap_or_default();
        if !new_root.is_empty() && !cur_root.is_empty() && new_root != cur_root {
            self.modal = Modal::ConfirmConfig {
                port,
                token,
                old_root: cur_root,
                new_root,
            };
            cx.notify();
        } else {
            self.apply_daemon_config(port, token, None, false, cx);
        }
    }

    /// 两节（配对 / 配置）各自一张卡，页面本身只管把它们竖着摞起来。
    pub(super) fn render_settings(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        div()
            .id("settings-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .p(px(18.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .max_w(px(720.))
                    .child(self.render_pair_section(cx))
                    .child(self.render_config_section(cx)),
            )
    }

    /// ① 配对：左边二维码，右边连接信息 + 重启按钮
    fn render_pair_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        sect()
            .child(sect_title("配对 · Android 扫码自动填入地址与 TOKEN"))
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(self.render_qr_box())
                    .child(self.render_conn_info(cx)),
            )
    }

    /// 配对二维码本身；还没连上（没有 payload）时画一个同尺寸的空框占住位置
    fn render_qr_box(&self) -> gpui::Div {
        match self.qr_modules.clone() {
            Some((w, modules)) => div()
                .w(px(170.))
                .h(px(170.))
                .flex_none()
                .rounded(px(6.))
                .bg(gpui::white())
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                            let pad = 8.0f32;
                            let avail = f32::from(bounds.size.width) - pad * 2.0;
                            let m = avail / w as f32;
                            for y in 0..w {
                                for x in 0..w {
                                    if modules[y * w + x] {
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(
                                                    bounds.origin.x + px(pad + x as f32 * m),
                                                    bounds.origin.y + px(pad + y as f32 * m),
                                                ),
                                                size(px(m + 0.5), px(m + 0.5)),
                                            ),
                                            gpui::black(),
                                        ));
                                    }
                                }
                            }
                        },
                    )
                    .size_full(),
                ),
            None => div()
                .w(px(170.))
                .h(px(170.))
                .flex_none()
                .rounded(px(6.))
                .border_1()
                .border_color(c(theme::EDGE))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(c(theme::FAINT))
                .child("连接后生成"),
        }
    }

    /// 配对卡右半：连接状态一行 + daemon 报上来的几项 + 重启按钮
    fn render_conn_info(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (conn_color, conn_label) = match self.conn {
            ConnState::Connected => (theme::GREEN, "已连接"),
            ConnState::Connecting => (theme::AMBER, "连接中…"),
            ConnState::Disconnected => (theme::RED, "未连接"),
        };
        let mut info = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pb(px(6.))
                    .child(dot(conn_color))
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::BOLD)
                            .child(conn_label),
                    )
                    .when(!self.endpoint_from_config, |el| {
                        el.child(
                            div()
                                .text_size(px(11.))
                                .text_color(c(theme::AMBER))
                                .child("（未读到本机 config.toml）"),
                        )
                    }),
            );
        if let Some(h) = &self.health {
            info = info
                .child(kv_row("版本", format!("aaa-daemon v{}", h.version), None))
                .when(h.update_pending, |el| {
                    el.child(kv_row("更新", "有新构建，需重启才生效".into(), Some(theme::AMBER)))
                })
                .child(kv_row("运行", format!("{} 分钟", h.uptime_s / 60), None))
                .child(kv_row("项目根", h.project_root.clone(), None))
                .child(kv_row(
                    "SSD",
                    if h.ssd_mounted {
                        "已挂载 ✓".into()
                    } else {
                        "未挂载 ✕（创建/删除被禁用）".into()
                    },
                    Some(if h.ssd_mounted { theme::GREEN } else { theme::RED }),
                ));
        }
        if let Some(ep) = &self.net.endpoint() {
            info = info.child(kv_row("地址", format!("{}:{}", ep.host, ep.port), None));
        }
        // 重启：有新构建时用主色提醒，其余时候是个普通次要按钮
        if self.health.is_some() {
            let pending = self.health.as_ref().is_some_and(|h| h.update_pending);
            let btn = if pending {
                btn_primary("restart-daemon", "重启 daemon")
            } else {
                btn_secondary("restart-daemon", "重启 daemon")
            };
            info = info.child(
                div().pt(px(8.)).child(
                    btn.on_click(cx.listener(|this, _, _, cx| this.request_restart_daemon(cx))),
                ),
            );
        }
        info
    }

    /// ② 可编辑配置：host/port/token 一行、项目目录一行，外加两个提交按钮
    fn render_config_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let token_for_copy = self.token_input.read(cx).text().to_string();
        sect()
            .child(sect_title("配置 · 保存会写入 config.toml 并重启 daemon"))
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_end()
                    .gap(px(10.))
                    .child(field("HOST", self.host_input.clone()).w(px(150.)).flex_none())
                    .child(field("PORT", self.port_input.clone()).w(px(70.)).flex_none())
                    // token 是这行里唯一的变长内容：剩余宽度全给它
                    .child(field("TOKEN", self.token_input.clone()).flex_1().min_w(px(120.)))
                    .child(
                        tbtn("copy-token", "复制").flex_none().on_click(cx.listener(
                            move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    token_for_copy.clone(),
                                ));
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_end()
                    .gap(px(10.))
                    .mt(px(10.))
                    .child(field("项目目录", self.root_input.clone()).flex_1().min_w(px(200.)))
                    .child(
                        btn_secondary("connect-btn", "连接").flex_none().on_click(cx.listener(
                            |this, _, _, cx| {
                                let host = this.host_input.read(cx).text().trim().to_string();
                                let port =
                                    this.port_input.read(cx).text().trim().parse().unwrap_or(2730);
                                let token = this.token_input.read(cx).text().trim().to_string();
                                if host.is_empty() {
                                    this.set_error("host 不能为空".into(), cx);
                                    return;
                                }
                                this.net.set_endpoint(crate::model::Endpoint { host, port, token });
                                this.conn = ConnState::Connecting;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        btn_primary("save-cfg", "保存到 daemon").flex_none().on_click(
                            cx.listener(|this, _, _, cx| {
                                this.save_daemon_config(cx);
                            }),
                        ),
                    ),
            )
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(10.5))
                    .text_color(c(theme::FAINT))
                    .child("「连接」只改本机指向；「保存」写入 daemon 配置并重启它（有存活会话会被拒绝）。修改项目目录时会先询问是否迁移现有项目。"),
            )
    }
}
