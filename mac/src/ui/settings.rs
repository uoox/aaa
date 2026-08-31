//! 设置页：连接状态 / token / 配对二维码 / macOS 权限一键申请。

use gpui::{
    Bounds, ClipboardItem, Context, SharedString, Window, canvas, div, fill, point, prelude::*, px,
    size,
};

use super::RootView;
use super::kit::*;
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

fn mask_token(token: &str) -> String {
    if token.len() <= 12 {
        return "•".repeat(token.len().max(4));
    }
    format!("{}••••••••{}", &token[..8], &token[token.len() - 4..])
}

impl RootView {
    pub(super) fn render_settings(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let sect_title = |text: &'static str| {
            div()
                .font_family("Menlo")
                .text_size(px(10.))
                .text_color(c(theme::FAINT))
                .pb(px(8.))
                .child(text)
        };
        let kv_row = |k: &'static str, v: String, color: Option<u32>| {
            div()
                .w_full()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(5.))
                .border_b_1()
                .border_color(ca(theme::EDGE, 0.5))
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .font_family("Menlo")
                        .text_size(px(10.))
                        .text_color(c(theme::FAINT))
                        .child(k),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.))
                        .text_color(c(color.unwrap_or(theme::INK)))
                        .child(SharedString::from(v)),
                )
        };

        // ── 连接 ────────────────────────────────────────────────────────
        let ep = self.net.endpoint();
        let (conn_color, conn_label) = match self.conn {
            ConnState::Connected => (theme::GREEN, "已连接"),
            ConnState::Connecting => (theme::AMBER, "连接中…"),
            ConnState::Disconnected => (theme::RED, "未连接"),
        };
        let mut conn_sect = div()
            .w_full()
            .p(px(14.))
            .rounded(px(10.))
            .bg(c(theme::SURFACE))
            .border_1()
            .border_color(c(theme::EDGE))
            .child(sect_title("连接"))
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
                                .child("（未读到 ~/.config/aaa-daemon/config.toml，手动填写）"),
                        )
                    }),
            );
        if let Some(h) = &self.health {
            conn_sect = conn_sect
                .child(kv_row("VERSION", format!("aaa-daemon v{}", h.version), None))
                .child(kv_row(
                    "UPTIME",
                    format!("{} 分钟", h.uptime_s / 60),
                    None,
                ))
                .child(kv_row("项目根", h.project_root.clone(), None))
                .child(kv_row(
                    "SSD",
                    if h.ssd_mounted {
                        "已挂载 ✓".into()
                    } else {
                        "未挂载 ✕（创建/删除被禁用）".into()
                    },
                    Some(if h.ssd_mounted {
                        theme::GREEN
                    } else {
                        theme::RED
                    }),
                ));
        }
        if let Some(ep) = &ep {
            conn_sect = conn_sect
                .child(kv_row("地址", format!("{}:{}", ep.host, ep.port), None))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .py(px(5.))
                        .child(
                            div()
                                .w(px(90.))
                                .flex_none()
                                .font_family("Menlo")
                                .text_size(px(10.))
                                .text_color(c(theme::FAINT))
                                .child("TOKEN"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .font_family("Menlo")
                                .text_size(px(12.))
                                .child(SharedString::from(mask_token(&ep.token))),
                        )
                        .child(tbtn("copy-token", "复制").map(|el| {
                            let token = ep.token.clone();
                            el.on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(token.clone()));
                            }))
                        })),
                );
        }
        // 手动填写（config 缺失或想连别的机器）
        let input_field = |label: &'static str, input: gpui::Entity<super::MiniInput>| {
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
        };
        conn_sect = conn_sect.child(
            div()
                .flex()
                .w_full()
                .items_end()
                .gap(px(10.))
                .mt(px(10.))
                .child(input_field("HOST", self.host_input.clone()).w(px(150.)).flex_none())
                .child(input_field("PORT", self.port_input.clone()).w(px(70.)).flex_none())
                // token 是这行里唯一的变长内容：把剩下的宽度全给它。
                // 固定 240px 时 aaa_tk_ + 32 位十六进制要 293px，字段装不下。
                .child(
                    input_field("TOKEN", self.token_input.clone())
                        .flex_1()
                        .min_w(px(120.)),
                )
                .child(btn_primary("connect-btn", "连接").flex_none().on_click(
                    cx.listener(|this, _, _, cx| {
                        let host = this.host_input.read(cx).text.trim().to_string();
                        let port = this.port_input.read(cx).text.trim().parse().unwrap_or(2730);
                        let token = this.token_input.read(cx).text.trim().to_string();
                        if host.is_empty() {
                            this.set_error("host 不能为空".into(), cx);
                            return;
                        }
                        this.net.set_endpoint(crate::model::Endpoint { host, port, token });
                        this.conn = ConnState::Connecting;
                        cx.notify();
                    }),
                )),
        );

        // ── 配对 ────────────────────────────────────────────────────────
        // 没有二维码时不占那 170×170 的灰方块——一个写着「连接后生成」的空盒子
        // 是这页最大的一块无效面积。
        let pair_sect = div()
            .w_full()
            .p(px(14.))
            .rounded(px(10.))
            .bg(c(theme::SURFACE))
            .border_1()
            .border_color(c(theme::EDGE))
            .child(sect_title("配对 · Android 扫码自动填入地址与 Token"))
            .map(|el| match self.qr_modules.clone() {
                Some((w, modules)) => el.child(
                    div()
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
                                                            bounds.origin.x
                                                                + px(pad + x as f32 * m),
                                                            bounds.origin.y
                                                                + px(pad + y as f32 * m),
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
                ),
                None => el.child(
                    div()
                        .text_size(px(12.))
                        .text_color(c(theme::FAINT))
                        .child("连接 daemon 后生成"),
                ),
            });

        // ── macOS 权限 ──────────────────────────────────────────────────
        // 列表为空不写占位文案：为什么空（未连接）上面那张卡已经说了。
        let mut perm_rows = div().w_full().flex().flex_col();
        for p in &self.permissions {
            let (color, label) = permission_status_style(&p.status);
            perm_rows = perm_rows.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(ca(theme::EDGE, 0.5))
                    .child(dot(color))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.5))
                            .child(SharedString::from(p.label.clone())),
                    )
                    .when(p.status == "needs_settings", |el| {
                        el.child(
                            div()
                                .text_size(px(10.5))
                                .text_color(c(theme::FAINT))
                                .child("无法弹窗，需在系统设置手动授予"),
                        )
                    })
                    .child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(11.))
                            .text_color(c(color))
                            .child(label),
                    ),
            );
        }
        let perm_sect = div()
            .w_full()
            .p(px(14.))
            .rounded(px(10.))
            .bg(c(theme::SURFACE))
            .border_1()
            .border_color(c(theme::EDGE))
            .child(sect_title("MACOS 权限 · 归责到 daemon，弹窗在本机逐个允许"))
            .child(perm_rows)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mt(px(12.))
                    .child(
                        btn_primary("perm-req", "一键申请全部").on_click(cx.listener(
                            |this, _, _, cx| {
                                let fut = this.net.request_permissions();
                                this.spawn_fetch(
                                    fut,
                                    |r, _: serde_json::Value, cx| {
                                        r.fetch_permissions(cx);
                                    },
                                    true,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        tbtn("perm-refresh", "刷新").on_click(cx.listener(|this, _, _, cx| {
                            this.fetch_permissions(cx);
                        })),
                    ),
            );

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
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_size(px(16.))
                            .child("设置"),
                    )
                    .child(conn_sect)
                    .child(pair_sect)
                    .child(perm_sect),
            )
    }
}
