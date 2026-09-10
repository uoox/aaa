//! 消息流视图：daemon `/sessions/:id/messages` 的气泡列表，与 Android 同源同构。
//! 终端仍是权威视图；消息流里能直接对 agent 说话（底部 composer → POST /input），
//! claude 的 AskUserQuestion 表单在这里画出来，但**只读**（v1.39，见 REMOVED.md）：
//! 题面、选项、末尾那条「其它…」都看得见，待答时卡上给一个「去终端答」——那道题
//! 只能在终端里答，那个对话框是 Claude Code 自己画的。⌘E 切换。
//!
//! 拉取模型：打开时全量补齐（seq 游标循环直到追平 last_seq），此后靠
//! /events 的 messages_changed 帧增量拉。`supported:false`（shell）或 404
//! （v1 daemon）→ 上层自动回落终端并隐藏切换入口。
//!
//! 谁在说话一眼可辨：用户一侧是靠右的主色淡底气泡（上方一行时间小字）；Claude
//! 一侧通栏、无底。**不写「Claude」三个字**（2026-09-10 用户拍板：气泡本身已经把
//! 两边分开了，再挂个署名只是噪音）。
//!
//! 复制：每条消息右上角一个常显的「复制」。gpui 这一版的文本元素没有选区
//! （`InteractiveText` 只有点击），跨消息拖选要在渲染层自己做一套命中测试，
//! 所以这里给的是「整条复制」，不是任意选区。

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, ElementId, Entity, FontStyle, FontWeight,
    HighlightStyle, InteractiveText, KeyDownEvent, Pixels, ScrollHandle, SharedString,
    StrikethroughStyle, StyledText, UnderlineStyle, Window, div, prelude::*, px, relative,
};

use super::kit::{c, ca};
use super::mini_input::MiniInput;
use super::stream_fold::{self, StreamItem};
use crate::markdown::{self, Block, Span};
use crate::model::{ChatMessage, PermissionPrompt, QuestionItem, QuestionSpec};
use crate::net::Net;
use crate::theme;

/// 内存里最多留这么多条：够回看，不至于无限膨胀
const KEEP: usize = 2000;

/// 离底部不到这么多像素就算「在底部」：新消息到来时跟到底，浮动 ↓ 也不出现
const BOTTOM_SLACK: f32 = 40.;

/// API 报错文案：409 = daemon 没法替你按（对话框已经不在了 / 没吃下）→ 去终端收尾
fn describe_api_error(e: &anyhow::Error) -> String {
    let msg = match e.downcast_ref::<crate::net::ApiFailure>() {
        Some(f) if !f.message.is_empty() => f.message.clone(),
        _ => e.to_string(),
    };
    if crate::net::http_status(e) == Some(409) {
        format!("{msg} · 去终端处理")
    } else {
        msg
    }
}

/// 某条 question 之后紧跟的是不是 answer（已答态标签用）
fn answered_after(msgs: &[ChatMessage], seq: u64) -> bool {
    msgs.iter()
        .filter(|m| m.seq > seq)
        .find(|m| m.kind == "question" || m.kind == "answer")
        .is_some_and(|m| m.kind == "answer")
}

pub struct MessagesView {
    sid: String,
    net: Net,
    pub msgs: Vec<ChatMessage>,
    last_seq: u64,
    /// None = 首拉未回；Some(false) = 该会话不支持消息流（回落终端）
    pub supported: Option<bool>,
    loading: bool,
    /// 已展开的 thinking / tool 消息 seq（点击切换）
    expanded: HashSet<u64>,
    /// 已展开的过程折叠，按轮 key（轮首条消息的 seq）记；live 尾轮也默认折叠
    fold_open: HashSet<u64>,
    /// assistant text 消息的 Markdown 块缓存（seq → blocks）：fetch 到达时解析一次，
    /// 渲染帧只读。seq 的正文不会变（dedup 保留首次到达的版本），所以不需要失效逻辑。
    md: HashMap<u64, Vec<Block>>,
    /// 刚按过「复制」的那一条：按钮原地写「已复制」，两秒后自己变回去
    copied: Option<u64>,
    scroll: ScrollHandle,
    /// 底部输入框：消息流里直接对 agent 说话（POST /input，text+回车）
    input: Entity<MiniInput>,
    /// 下一帧渲染时把焦点放到输入框（切进消息流视图时置位）
    wants_focus: bool,
    sending: bool,
    /// 会话进程是否还活着（上层按 session 事件同步）；退出后表单一律只读
    alive: bool,
    /// 窗口的排版系统：Markdown 表格按实际排版量列宽。render 开头刷新（元素树是在
    /// render 里建的，那时才有 window）
    text_sys: Option<std::sync::Arc<gpui::WindowTextSystem>>,
    /// v1.16：正在等的权限对话框；画成「允许 / 拒绝」卡片挂在流末尾
    permission: Option<PermissionPrompt>,
    perm_busy: bool,
    perm_error: Option<String>,
    /// 会话进程在跑（`running`）——与 Android 的 `live = state == "running"` 同一口径。
    /// 「进行中」的过程行按它画：`waiting` 的会话（被 Esc 打断、工具报错后停下）末尾
    /// 哪怕是 tool_result，也不该一直脉动。
    running: bool,
    /// 项目目录：附件上传的去处（`_inbox/`）
    project_path: String,
    uploading: bool,
    /// v1.22：待答的是消息流里的哪一条（daemon 给的 `asking_seq`，客户端不自己找）
    asking_seq: Option<u64>,
    /// 此刻排着还没送进去的消息（会话的 `queued`）：Claude Code 自己的队列，只画不管
    queued: Vec<crate::model::QueuedMsg>,
}

/// 消息流往外说的话。现在只有一句：**把我换成终端**——待答表单上那个「去终端答」
/// 按下去时发出来，`RootView` 订阅着它，收到就 `set_view(Terminal)`。视图归谁管这件事
/// 只有一个答案（RootView 的 `view_mode`），消息流自己不去改它。
pub enum MsgEvent {
    GoTerminal,
}

impl gpui::EventEmitter<MsgEvent> for MessagesView {}

impl MessagesView {
    pub fn new(sid: String, net: Net, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| MiniInput::new(cx, "对 Claude 说点什么… (回车发送)"));
        let mut v = MessagesView {
            sid,
            net,
            msgs: Vec::new(),
            last_seq: 0,
            supported: None,
            loading: false,
            expanded: HashSet::new(),
            fold_open: HashSet::new(),
            md: HashMap::new(),
            copied: None,
            scroll: ScrollHandle::new(),
            input,
            wants_focus: true,
            sending: false,
            alive: true,
            text_sys: None,
            permission: None,
            perm_busy: false,
            perm_error: None,
            running: false,
            project_path: String::new(),
            uploading: false,
            asking_seq: None,
            queued: Vec::new(),
        };
        v.fetch(cx);
        v
    }

    /// 切进消息流视图时调用：下一帧把焦点交给输入框
    pub fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.wants_focus = true;
        cx.notify();
    }

    /// 上层在 session / snapshot 事件里同步：进程退了，表单就不能再交互
    pub fn set_project_path(&mut self, p: String) {
        self.project_path = p;
    }

    /// 📎：选一个文件上传进项目 `_inbox/`，路径以 `@路径 ` 追加到 composer——Claude Code
    /// 的 @ 引用，图片直接看、文件直接读。发送仍由用户按回车。
    fn attach(&mut self, cx: &mut Context<Self>) {
        if self.uploading || self.project_path.is_empty() {
            return;
        }
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("上传到项目收件箱".into()),
        });
        let net = self.net.clone();
        let project = self.project_path.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".into());
            let Ok(bytes) = std::fs::read(&path) else {
                log::warn!("读不了 {}", path.display());
                return;
            };
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.uploading = true;
                cx.notify();
            });
            let res = net.upload(&project, &name, bytes).await;
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.uploading = false;
                match res {
                    Ok(saved) => {
                        let cur = v.input.read(cx).text().to_string();
                        let mut next = cur.trim_end().to_string();
                        if !next.is_empty() {
                            next.push(' ');
                        }
                        next.push('@');
                        next.push_str(&saved);
                        next.push(' ');
                        v.input.update(cx, |i, cx| i.set_text(next, cx));
                        v.wants_focus = true;
                    }
                    Err(e) => log::warn!("上传失败: {e}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_session(
        &mut self,
        alive: bool,
        running: bool,
        permission: Option<PermissionPrompt>,
        asking_seq: Option<u64>,
        queued: Vec<crate::model::QueuedMsg>,
        cx: &mut Context<Self>,
    ) {
        if self.alive == alive
            && self.running == running
            && self.asking_seq == asking_seq
            && self.permission == permission
            && self.queued == queued
        {
            return;
        }
        self.queued = queued;
        if self.permission != permission {
            // 对话框换了 / 没了：上一次的忙碌与错误都作废
            self.perm_busy = false;
            self.perm_error = None;
        }
        self.permission = permission;
        self.alive = alive;
        self.running = running;
        self.asking_seq = asking_seq;
        cx.notify();
    }

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let text = self.input.read(cx).text().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.sending = true;
        let fut = self.net.session_input(&self.sid, text.clone());
        self.input.update(cx, |i, cx| i.set_text("", cx));
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.sending = false;
                if let Err(e) = res {
                    // 发送失败把话塞回输入框，别丢
                    log::warn!("消息流发送失败: {e}");
                    v.input.update(cx, |i, cx| i.set_text(text, cx));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if ev.keystroke.key != "enter" {
            return;
        }
        // MiniInput 不消费回车；composer 有焦点且不在组字才算「发送」
        let composer = self.input.read(cx);
        if composer.focus_handle.is_focused(window) && !composer.composing() {
            self.send(cx);
            cx.stop_propagation();
        }
    }

    /// 增量拉取；没追平 last_seq 就继续拉（打开旧会话时的补齐循环）
    pub fn fetch(&mut self, cx: &mut Context<Self>) {
        if self.loading || self.supported == Some(false) {
            return;
        }
        self.loading = true;
        let fut = self.net.messages(&self.sid, self.last_seq);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.loading = false;
                match res {
                    Ok(r) => {
                        v.supported = Some(r.supported);
                        if !r.messages.is_empty() {
                            // 跟不跟到底看的是变化**之前**的位置：用户正在翻历史时新消息不拽人
                            let was_at_bottom = v.near_bottom();
                            let had = !v.msgs.is_empty();
                            // assistant 文本按 CommonMark 解析一次进缓存；其余角色 / 种类保持纯文本
                            for m in r.messages.iter().filter(|m| is_markdown(m)) {
                                v.md.entry(m.seq).or_insert_with(|| markdown::parse(&m.text));
                            }
                            v.msgs.extend(r.messages);
                            v.msgs.sort_by_key(|m| m.seq);
                            v.msgs.dedup_by_key(|m| m.seq);
                            if v.msgs.len() > KEEP {
                                let cut = v.msgs.len() - KEEP;
                                v.msgs.drain(..cut);
                                let min_seq = v.msgs.first().map(|m| m.seq).unwrap_or(0);
                                v.md.retain(|seq, _| *seq >= min_seq);
                            }
                            v.last_seq = v.msgs.last().map(|m| m.seq).unwrap_or(0);
                            if stream_fold::should_follow_tail(was_at_bottom, had, !v.msgs.is_empty()) {
                                v.scroll.scroll_to_bottom();
                            }
                        }
                        if r.supported && v.last_seq < r.last_seq {
                            v.fetch(cx);
                        }
                    }
                    Err(e) => {
                        // v1 daemon 无此端点 → 永久回落终端；其余错误留待下次事件重试
                        if crate::net::http_status(&e) == Some(404) || e.to_string().contains("404")
                        {
                            v.supported = Some(false);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// messages_changed 帧带的 last_seq 没超过已见位置就不拉（自身回显去重）
    pub fn fetch_if_behind(&mut self, seq: u64, cx: &mut Context<Self>) {
        if seq == 0 || self.last_seq < seq {
            self.fetch(cx);
        }
    }

    fn toggle_expand(&mut self, seq: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&seq) {
            self.expanded.insert(seq);
        }
        cx.notify();
    }

    fn toggle_fold(&mut self, key: u64, cx: &mut Context<Self>) {
        if !self.fold_open.remove(&key) {
            self.fold_open.insert(key);
        }
        cx.notify();
    }

    /// 列表此刻是否在底部（含 BOTTOM_SLACK 的余量）。ScrollHandle 的 offset.y 向下滚
    /// 越负、max_offset.y 是可滚的总量，两者相加就是离底距离；还没排过版（都是 0）
    /// 或内容装得下时也算在底部——没东西可滚，浮动按钮不该出现。
    fn near_bottom(&self) -> bool {
        distance_to_bottom(self.scroll.offset().y, self.scroll.max_offset().y) <= px(BOTTOM_SLACK)
    }

    fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    /// 权限对话框：允许 / 拒绝 → daemon 驱动 PTY 作答；失败（对话框还在）把话留在卡片上
    fn decide_permission(&mut self, behavior: &'static str, cx: &mut Context<Self>) {
        if self.perm_busy {
            return;
        }
        self.perm_busy = true;
        self.perm_error = None;
        let fut = self.net.session_permission(&self.sid, behavior);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |v: &mut MessagesView, cx| {
                v.perm_busy = false;
                match res {
                    Ok(_) => v.fetch(cx),
                    Err(e) => v.perm_error = Some(describe_api_error(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn permission_card(&self, p: &PermissionPrompt, cx: &mut Context<Self>) -> gpui::AnyElement {
        let btn = |id: &'static str, label: &'static str, color: u32, cx: &mut Context<Self>, behavior: &'static str| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(5.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(c(theme::INK))
                .bg(ca(color, 0.18))
                .border_1()
                .border_color(ca(color, 0.6))
                .cursor_pointer()
                .hover(|st| st.bg(ca(color, 0.3)))
                .on_click(cx.listener(move |this, _, _, cx| this.decide_permission(behavior, cx)))
                .child(label)
        };
        div()
            .w_full()
            .mt(px(10.))
            .p(px(12.))
            .rounded(px(10.))
            .border_1()
            .border_color(ca(theme::AMBER, 0.7))
            .bg(ca(theme::AMBER, 0.08))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(c(theme::AMBER))
                    .child(SharedString::from(if p.kind == "elicitation" { "⚠ 有个表单在等你".to_string() } else { format!("⚠ Claude 请求授权 · {}", if p.tool_name.is_empty() { "工具" } else { p.tool_name.as_str() }) })),
            )
            .when(!p.summary.is_empty(), |el| {
                el.child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .text_color(c(theme::INK))
                        .whitespace_normal()
                        .child(SharedString::from(p.summary.clone())),
                )
            })
            .child(if p.kind == "elicitation" {
                div().text_size(px(11.)).text_color(c(theme::DIM)).child("MCP 服务器要你填表单：这个只能在终端里答（⌘E 切过去）")
            } else {
                div()
                    .flex()
                    .gap(px(8.))
                    .items_center()
                    .child(btn("perm-allow", "允许", theme::GREEN, cx, "allow"))
                    .child(btn("perm-deny", "拒绝", theme::RED, cx, "deny"))
                    .when(self.perm_busy, |el| el.child(div().text_size(px(11.)).text_color(c(theme::DIM)).child("…")))
                    .child(div().text_size(px(10.5)).text_color(c(theme::FAINT)).child("在终端里作答也一样"))
            })
            .when_some(self.perm_error.clone(), |el, e| el.child(div().text_size(px(11.)).text_color(c(theme::RED)).child(SharedString::from(e))))
            .into_any_element()
    }


    // ── 渲染 ───────────────────────────────────────────────────────────

    fn row(
        &self,
        m: &ChatMessage,
        pending: Option<u64>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let text: SharedString = m.text.clone().into();
        let copy = self.copy_btn(m.seq, m.text.clone(), cx);
        match () {
            _ if m.kind == "thinking" => self.thinking_row(m, cx),
            _ if m.kind == "tool_use" || m.kind == "tool_result" => self.tool_row(m, cx),
            // claude 的 AskUserQuestion：结构化表单原生画；没带 question（不该发生）
            // 就退回普通 assistant 气泡，至少把题面露出来
            _ if m.kind == "question" => match &m.question {
                Some(spec) => self.question_card(m, spec, pending, cx),
                None => assistant_block(assistant_text(text), copy),
            },
            // 表单的回答画在用户一侧，加一行「回答」小字与普通输入区分
            _ if m.kind == "answer" => {
                let err = m.tool.as_ref().is_some_and(|t| t.status == "err");
                let (caption, color) = if err {
                    ("回答 · 出错", theme::RED)
                } else {
                    ("回答", theme::FAINT)
                };
                user_column(Some((caption.into(), color)), user_bubble(text), copy)
            }
            // 用户的话：靠右气泡，上方压一行发出时间（transcript 带 ts 才有）
            _ if m.role == "user" => user_column(
                time_caption(&m.ts).map(|t| (SharedString::from(t), theme::FAINT)),
                user_bubble(text),
                copy,
            ),
            _ if m.role == "system" => div()
                .w_full()
                .text_size(px(10.5))
                .text_color(c(theme::FAINT))
                .child(text)
                .into_any_element(),
            // assistant 文本 = CommonMark：通栏块列，右上角一个「复制」
            _ => assistant_block(self.assistant_body(m, 12.5, theme::INK), copy),
        }
    }

    /// 一条消息的「复制」：常显、最淡的一档，点完原地变「已复制」两秒。
    /// gpui 这一版没有文本选区，所以复制的粒度是**整条**（见模块头）。
    fn copy_btn(&self, seq: u64, text: String, cx: &mut Context<Self>) -> AnyElement {
        let done = self.copied == Some(seq);
        div()
            .id(("copy", seq as usize))
            .flex_none()
            .px(px(4.))
            .rounded(px(4.))
            .text_size(px(9.5))
            .text_color(c(if done { theme::ACCENT } else { theme::FAINT }))
            .cursor_pointer()
            .hover(|st| st.text_color(c(theme::ACCENT)).bg(c(theme::EDGE_LIGHT)))
            .on_click(cx.listener(move |v: &mut Self, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
                v.copied = Some(seq);
                cx.notify();
                // 两秒后自己变回「复制」——不留一个永远亮着的「已复制」
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let _ = this.update(cx, |v: &mut Self, cx| {
                        if v.copied == Some(seq) {
                            v.copied = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }))
            .child(if done { "已复制" } else { "复制" })
            .into_any_element()
    }

    /// 「思考」块：折起来只留第一行，点一下展开全文。
    fn thinking_row(&self, m: &ChatMessage, cx: &mut Context<Self>) -> gpui::AnyElement {
        let expanded = self.expanded.contains(&m.seq);
        let seq = m.seq;
        let first_line: SharedString = if expanded {
            m.text.clone().into()
        } else {
            m.text.lines().next().unwrap_or("").to_string().into()
        };
        div()
            .id(("think", m.seq as usize))
            .w_full()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(8.))
            .bg(c(theme::INSET))
            .cursor_pointer()
            .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.toggle_expand(seq, cx)))
            .child(
                div()
                    .text_size(px(10.5))
                    .italic()
                    .text_color(c(theme::FAINT))
                    .child(if expanded { "▾ 思考" } else { "▸ 思考" }),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .italic()
                    .text_color(c(theme::FAINT))
                    .when(!expanded, |el| {
                        el.overflow_hidden().text_ellipsis().whitespace_nowrap()
                    })
                    .child(first_line),
            )
            .into_any_element()
    }

    /// 工具调用 / 工具结果一行：状态点 + 名字 + 摘要，点开才把正文露出来。
    fn tool_row(&self, m: &ChatMessage, cx: &mut Context<Self>) -> gpui::AnyElement {
        let text: SharedString = m.text.clone().into();
        let (name, summary, status) = m
            .tool
            .as_ref()
            .map(|t| (t.name.clone(), t.summary.clone(), t.status.clone()))
            .unwrap_or_default();
        let status_color = match status.as_str() {
            "ok" => theme::GREEN,
            "err" => theme::RED,
            "running" => theme::AMBER,
            _ => theme::DIM,
        };
        let label: SharedString = if m.kind == "tool_result" {
            "⎿ 结果".into()
        } else if name.is_empty() {
            "tool".into()
        } else {
            name.into()
        };
        let expanded = self.expanded.contains(&m.seq) && !m.text.is_empty();
        let seq = m.seq;
        div()
            .id(("tool", m.seq as usize))
            .w_full()
            .cursor_pointer()
            .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.toggle_expand(seq, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(c(status_color)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_family("Menlo")
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(c(theme::INK))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .font_family("Menlo")
                            .text_color(c(theme::DIM))
                            .child(SharedString::from(summary)),
                    ),
            )
            .when(expanded, |el| {
                el.child(
                    div()
                        .ml(px(13.))
                        .mt(px(3.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(8.))
                        .bg(c(theme::INSET))
                        .text_size(px(11.))
                        .font_family("Menlo")
                        .text_color(c(theme::DIM))
                        .child(text),
                )
            })
            .into_any_element()
    }

    /// assistant 文本的正文：缓存里的 Markdown 块列；缓存未命中（理论上不会）就现场
    /// 解析；解析结果为空 → 回落纯文本。回复用 ink 12.5，折叠里的中途文本压成 dim 12。
    fn assistant_body(&self, m: &ChatMessage, size: f32, color: u32) -> AnyElement {
        let parsed;
        let blocks: &[Block] = if is_markdown(m) {
            match self.md.get(&m.seq) {
                Some(b) => b,
                None => {
                    parsed = markdown::parse(&m.text);
                    &parsed
                }
            }
        } else {
            &[]
        };
        if blocks.is_empty() {
            return div()
                .w_full()
                .text_size(px(size))
                .text_color(c(color))
                .child(SharedString::from(m.text.clone()))
                .into_any_element();
        }
        let mut ids = MdIds { seq: m.seq, next: 0 };
        div()
            .w_full()
            .text_size(px(size))
            .text_color(c(color))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(md_blocks(blocks, &mut ids, self.text_sys.as_deref()))
            .into_any_element()
    }

    /// 过程折叠行：一行紧凑淡字，▸/▾ 小箭头，工具名等宽；live 尾轮用脉动的主色点
    /// 代替箭头，折叠着也报最近一步在干什么。整行可点，悬停淡底。
    fn fold_row(
        &self,
        fold_key: u64,
        steps: &[&ChatMessage],
        live_tail: Option<&ChatMessage>,
        live: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.fold_open.contains(&fold_key);
        let n = stream_fold::step_count(steps);
        let label: SharedString = match (live, open) {
            (true, true) => format!("进行中 · {n} 步").into(),
            (true, false) => stream_fold::live_label(steps, live_tail).into(),
            (false, true) => format!("过程 · {n} 步").into(),
            (false, false) => stream_fold::fold_label(steps).into(),
        };
        let marker: AnyElement = if live {
            div()
                .w(px(6.))
                .h(px(6.))
                .rounded_full()
                .bg(c(theme::ACCENT))
                .with_animation(
                    ElementId::from(format!("fold-pulse-{fold_key}")),
                    Animation::new(Duration::from_millis(1100)).repeat(),
                    |el, t| {
                        // 0→1→0 的呼吸：t 过半往回走
                        let breath = if t < 0.5 { t * 2. } else { 2. - t * 2. };
                        el.opacity(0.35 + 0.65 * breath)
                    },
                )
                .into_any_element()
        } else {
            div()
                .text_size(px(10.))
                .text_color(c(theme::FAINT))
                .child(if open { "▾" } else { "▸" })
                .into_any_element()
        };
        div()
            .id(("fold", fold_key as usize))
            .w_full()
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(6.))
            .py(px(3.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(ca(theme::INK, 0.05)))
            .on_click(cx.listener(move |v: &mut Self, _, _, cx| v.toggle_fold(fold_key, cx)))
            .child(div().flex_none().w(px(10.)).flex().justify_center().child(marker))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_size(px(11.))
                    .font_family("Menlo")
                    .text_color(c(if live { theme::DIM } else { theme::FAINT }))
                    .child(label),
            )
            .into_any_element()
    }

    /// 展开后的单步：思考 / 工具 / system 复用原有行（各自的展开开关照旧），
    /// 中途的 assistant 文本压成 dim 小字、不带「✻ Claude」头。整体缩进 12px。
    fn step_row(&self, m: &ChatMessage, pending: Option<u64>, cx: &mut Context<Self>) -> AnyElement {
        let inner = if is_markdown(m) {
            self.assistant_body(m, 12., theme::DIM)
        } else {
            self.row(m, pending, cx)
        };
        div().w_full().pl(px(12.)).child(inner).into_any_element()
    }

    /// 表单卡片：琥珀描边，逐题画选项，待答时底部有「提交」
    fn question_card(
        &self,
        m: &ChatMessage,
        spec: &QuestionSpec,
        pending: Option<u64>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let seq = m.seq;
        // **待答 = daemon 说待答的正是这一条**（会话的 `asking_seq`）且会话还活着。
        // 待答不等于「能在这儿答」：v1.39 起这张卡一律只读（见本文件头），待答时
        // 露出的是一个「去终端答」，不是提交按钮。
        let waiting = pending == Some(seq) && self.alive;
        let tag: Option<&'static str> = if waiting {
            None
        } else if answered_after(&self.msgs, seq) {
            Some("已回答")
        } else if !self.alive {
            Some("已结束")
        } else {
            // 活着、没答、后面却又来了新问题：上一张被 agent 自己跳过了
            Some("已过期")
        };

        let mut card = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .rounded(px(10.))
            .border_1()
            .border_color(ca(theme::AMBER, 0.55))
            .bg(ca(theme::AMBER, 0.08))
            .when(!waiting, |el| el.opacity(0.6));

        // 顶行：表单标识 + 已答态标签
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(10.5))
                        .font_family("Menlo")
                        .text_color(c(theme::AMBER))
                        .child(if waiting { "? 等你选择" } else { "? 表单" }),
                )
                .when_some(tag, |el, t| {
                    el.child(
                        div()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(c(theme::DIM))
                            .bg(ca(theme::INK, 0.06))
                            .child(t),
                    )
                }),
        );

        for q in spec.questions.iter() {
            card = card.child(self.question_block(q));
        }

        // 待答：一个「去终端答」。**这张卡自己不收答案**——那个对话框是 Claude Code
        // 画在终端里的，去按它自己的键，按什么就是什么。⌘E 也切得过去。
        if waiting {
            card = card.child(
                div().flex().items_center().gap(px(8.)).justify_end().child(
                    div()
                        .flex_1()
                        .text_size(px(10.5))
                        .font_family("Menlo")
                        .text_color(c(theme::FAINT))
                        .child("在终端里答（⌘E）"),
                ).child(
                    accent_btn(("q-terminal", seq))
                        .cursor_pointer()
                        .hover(|s| s.opacity(0.85))
                        .on_click(cx.listener(|_: &mut Self, _, _, cx| cx.emit(MsgEvent::GoTerminal)))
                        .child("去终端答"),
                ),
            );
        }

        div().w_full().flex().flex_col().gap(px(4.)).child(card).into_any_element()
    }

    /// 一道题：header / 题面 / 选项行 / 末尾固定一条「其它」。**只画，不收**
    /// （v1.39 起表单一律只读，答在终端里）：所以没有勾选态，选项前面那个记号
    /// 一律是空的那一个，只用来说明这题是单选还是多选。
    fn question_block(&self, q: &QuestionItem) -> gpui::AnyElement {
        let glyph = if q.multi_select { "☐" } else { "○" };
        let glyph_el = || {
            div()
                .flex_none()
                .w(px(16.))
                .text_size(px(12.))
                .font_family("Menlo")
                .text_color(c(theme::DIM))
                .child(glyph)
        };

        let mut block = div().flex().flex_col().gap(px(3.));
        if !q.header.is_empty() {
            block = block.child(
                div()
                    .text_size(px(10.5))
                    .font_family("Menlo")
                    .text_color(c(theme::FAINT))
                    .child(SharedString::from(q.header.clone())),
            );
        }
        block = block.child(
            div()
                .text_size(px(13.))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(c(theme::INK))
                .child(SharedString::from(q.question.clone())),
        );

        for opt in q.options.iter() {
            let row = div()
                .flex()
                .items_start()
                .gap(px(8.))
                .px(px(6.))
                .py(px(3.))
                .rounded(px(6.))
                .child(glyph_el())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(c(theme::INK))
                                .child(SharedString::from(opt.label.clone())),
                        )
                        .when(!opt.description.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c(theme::DIM))
                                    .child(SharedString::from(opt.description.clone())),
                            )
                        }),
                );
            block = block.child(row);
        }

        // 末尾那条「其它…」：Claude Code 的对话框固定有它，这里照画，说明这题
        // 除了列出的选项还能自己写
        let other_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(6.))
            .py(px(3.))
            .child(glyph_el())
            .child(div().text_size(px(12.5)).text_color(c(theme::DIM)).child("其它…"));
        block.child(other_row).into_any_element()
    }
}

/// Claude 的一段：左侧、通栏、不再有气泡底；上方一行「✻ Claude」主色小字标明说话的人
/// 消息流里的实心主色小按钮：底部「发送」和表单「提交」两处骨架逐字相同。
/// 没用 kit 的 `btn_primary`——那是模态里的尺寸（px14/py5/12.5），消息流这两个
/// 小一号，硬合成一个就得给 kit 加尺寸参数。
///
/// cursor / hover / on_click 不收进来：表单提交在「填不全」时是禁用态，
/// 只画不接事件。
fn accent_btn(id: impl Into<ElementId>) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(4.))
        .rounded(px(6.))
        .bg(c(theme::ACCENT))
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::BOLD)
        .text_color(c(theme::ON_ACCENT))
}

fn assistant_block(body: AnyElement, copy: AnyElement) -> AnyElement {
    // 右上角那个「复制」是绝对定位的，正文右边预留出它的宽度，长行才不会压到它底下
    div()
        .relative()
        .w_full()
        .child(div().w_full().pr(px(32.)).child(body))
        .child(div().absolute().top(px(-1.)).right(px(0.)).child(copy))
        .into_any_element()
}

/// 离底距离：ScrollHandle 向下滚 offset.y 越负，max_offset.y 是可滚总量。
/// 内容装得下 / 还没排版时两者都是 0 → 0（算在底部）。
fn distance_to_bottom(offset_y: Pixels, max_y: Pixels) -> Pixels {
    (max_y + offset_y).max(px(0.))
}

/// 列表里相邻项之间的上边距：轮与轮之间 14（新一轮从用户消息开始），展开的步骤
/// 之间 3，同轮内 User→Fold→Reply 8；首项 0。
fn item_gap(index: usize, item: &StreamItem) -> f32 {
    match item {
        _ if index == 0 => 0.,
        StreamItem::User(_) => 14.,
        StreamItem::Step(_) => 3.,
        _ => 8.,
    }
}

/// 非 Markdown 的 assistant 文本（question 缺表单时的兜底）
fn assistant_text(text: SharedString) -> AnyElement {
    div()
        .w_full()
        .text_size(px(12.5))
        .text_color(c(theme::INK))
        .child(text)
        .into_any_element()
}

/// 用户一侧的一列：可选的小字说明（时间 / 「回答」）靠右压在气泡上方
fn user_column(caption: Option<(SharedString, u32)>, bubble: gpui::Div, copy: AnyElement) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(copy)
                .when_some(caption, |el, (text, color)| {
                    el.child(
                        div()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(c(color))
                            .child(text),
                    )
                }),
        )
        .child(bubble)
        .into_any_element()
}

/// 用户一侧的气泡（调用方负责靠右）：主色淡底 + 主色描边，右下角收小一点像
/// 「说出去的话」；亮底上 0.14 就够，再重会糊成一块
/// 待发送的气泡：和用户气泡同侧同款，只是底色换成琥珀、字用二级色——它还没进对话，
/// 但已经是「你说的话」。
fn pending_bubble(text: SharedString) -> gpui::Div {
    div()
        .max_w(relative(0.78))
        .px(px(12.))
        .py(px(7.))
        .rounded(px(12.))
        .rounded_br(px(5.))
        .bg(ca(theme::AMBER, 0.12))
        .border_1()
        .border_color(ca(theme::AMBER, 0.35))
        .text_size(px(12.5))
        .text_color(c(theme::DIM))
        .child(text)
}

fn user_bubble(text: SharedString) -> gpui::Div {
    div()
        .max_w(relative(0.78))
        .px(px(12.))
        .py(px(7.))
        .rounded(px(12.))
        .rounded_br(px(5.))
        .bg(ca(theme::ACCENT, 0.14))
        .border_1()
        .border_color(ca(theme::ACCENT, 0.35))
        .text_size(px(12.5))
        .text_color(c(theme::INK))
        .child(text)
}

/// 用户消息上方的时间小字：transcript 的 ISO 时间 → 本地时区 HH:mm。
/// 解析不了（旧 daemon 不带 ts / 格式怪）就不显示，不猜。
pub fn time_caption(ts: &str) -> Option<String> {
    time_caption_in(ts, &chrono::Local)
}

fn time_caption_in<Tz: chrono::TimeZone>(ts: &str, tz: &Tz) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let ts = ts.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return Some(dt.with_timezone(tz).format("%H:%M").to_string());
    }
    // 没有时区 / 秒的裸 ISO（"2026-09-02T10:05"）：按字面取 HH:mm，不换时区
    let b = ts.as_bytes();
    let looks_iso = b.len() >= 16
        && b[10] == b'T'
        && b[13] == b':'
        && b[11..13].iter().chain(&b[14..16]).all(u8::is_ascii_digit);
    looks_iso.then(|| ts[11..16].to_string())
}

/// 只有 assistant 的 text 走 Markdown；user / tool / thinking / system / question 保持纯文本
fn is_markdown(m: &ChatMessage) -> bool {
    m.role == "assistant" && m.kind == "text"
}

// ── Markdown 块渲染 ────────────────────────────────────────────────────────────
//
// 输入是 crate::markdown 的纯数据块，这里只负责映射到 gpui 元素。
// 需要 id 的元素（横向滚动容器、可点链接段落）用 (seq, 计数) 生成稳定且唯一的 id。

/// 一条消息内的 id 发号器。`seq` 只是让 id 在整页里唯一——目录浏览那边
/// （`doc_view`）没有消息号，传一个固定值即可，一页只画一个文件。
pub(super) struct MdIds {
    pub seq: u64,
    pub next: usize,
}

impl MdIds {
    fn next(&mut self, kind: &str) -> ElementId {
        self.next += 1;
        ElementId::from(format!("md-{kind}-{}-{}", self.seq, self.next))
    }
}

pub(super) fn md_blocks(blocks: &[Block], ids: &mut MdIds, ts: Option<&gpui::WindowTextSystem>) -> Vec<AnyElement> {
    blocks.iter().map(|b| md_block(b, ids, ts)).collect()
}

fn md_block(b: &Block, ids: &mut MdIds, ts: Option<&gpui::WindowTextSystem>) -> AnyElement {
    match b {
        Block::Heading { level, spans } => {
            let size = match level {
                1 => 16.,
                2 => 14.5,
                _ => 13.5,
            };
            div()
                .text_size(px(size))
                .font_weight(FontWeight::BOLD)
                .child(md_inline(spans, ids))
                .into_any_element()
        }
        Block::Paragraph(spans) => div()
            .text_size(px(12.5))
            .child(md_inline(spans, ids))
            .into_any_element(),
        Block::Code { lang, text } => md_mono_block(ids, "code", text, 11.5, lang),
        Block::Table { header, rows } => match ts {
            Some(ts) => md_table(header, rows, ids, ts),
            // 还没拿到窗口排版系统（理论上只有 render 之外）：退回等宽文本
            None => md_mono_block(ids, "table", &markdown::table_text(header, rows), 11., ""),
        },
        Block::List {
            ordered,
            start,
            items,
        } => {
            // 标记列定宽：无序 14px（嵌套列表由此缩进 14px），有序留够两位数 + 点
            let marker_w = if *ordered { 22. } else { 14. };
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .children(items.iter().enumerate().map(|(i, item)| {
                    let marker: SharedString = if *ordered {
                        format!("{}.", start + i as u64).into()
                    } else {
                        "•".into()
                    };
                    div()
                        .flex()
                        .items_start()
                        .child(
                            div()
                                .flex_none()
                                .w(px(marker_w))
                                .text_size(px(12.5))
                                .text_color(c(theme::DIM))
                                .child(marker),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .children(md_blocks(item, ids, ts)),
                        )
                }))
                .into_any_element()
        }
        Block::Quote(children) => div()
            .border_l(px(3.))
            .border_color(ca(theme::FAINT, 0.6))
            .pl(px(10.))
            .text_color(c(theme::DIM))
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(md_blocks(children, ids, ts))
            .into_any_element(),
        Block::Rule => div()
            .w_full()
            .h(px(1.))
            .my(px(4.))
            .bg(c(theme::EDGE))
            .into_any_element(),
    }
}

/// 行内片段 → 单个 StyledText（粗/斜/删除线/代码/链接为 highlight 区间）。
/// 含链接时包成 InteractiveText，点击区间打开 URL。
fn md_inline(spans: &[Span], ids: &mut MdIds) -> AnyElement {
    let mut text = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut families: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut link_ranges: Vec<Range<usize>> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    for s in spans {
        if s.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&s.text);
        let range = start..text.len();
        let mut hl = HighlightStyle::default();
        let mut styled = false;
        if s.bold {
            hl.font_weight = Some(FontWeight::BOLD);
            styled = true;
        }
        if s.italic {
            hl.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if s.strike {
            hl.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            });
            styled = true;
        }
        if s.code {
            hl.background_color = Some(c(theme::CODE_BG).into());
            hl.color = Some(c(theme::CODE_INK).into());
            families.push((range.clone(), "Menlo".into()));
            styled = true;
        }
        if let Some(url) = &s.link {
            hl.color = Some(c(theme::ACCENT).into());
            hl.underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(c(theme::ACCENT).into()),
                wavy: false,
            });
            link_ranges.push(range.clone());
            urls.push(url.clone());
            styled = true;
        }
        if styled {
            highlights.push((range, hl));
        }
    }
    let styled = StyledText::new(text)
        .with_highlights(highlights)
        .with_font_family_overrides(families);
    if urls.is_empty() {
        styled.into_any_element()
    } else {
        InteractiveText::new(ids.next("link"), styled)
            .on_click(link_ranges, move |ix, _window, cx| {
                if let Some(url) = urls.get(ix) {
                    cx.open_url(url);
                }
            })
            .into_any_element()
    }
}

/// 表格：真正的网格，列宽 = 该列最宽单元格**按实际排版测出来的像素**（+ 内边距）。
/// 以前是把表格拼成等宽文本靠空格对齐——中文落到备用字体时并不是 Menlo 的两倍宽，
/// 列就漂了（2026-09-07 用户反馈「表格不太整齐」）。单元格里的粗体 / 行内代码 / 链接
/// 照常渲染；表头加粗、下加一条线；整体比消息宽时横向滚动。
fn md_table(header: &[Vec<Span>], rows: &[Vec<Vec<Span>>], ids: &mut MdIds, ts: &gpui::WindowTextSystem) -> AnyElement {
    const SIZE: f32 = 11.5;
    const PAD: f32 = 8.;
    let cols = rows.iter().map(Vec::len).chain(std::iter::once(header.len())).max().unwrap_or(0);
    if cols == 0 {
        return div().into_any_element();
    }
    let measure = |spans: &[Span]| -> f32 {
        let text = markdown::spans_plain(spans).replace('\n', " ");
        if text.is_empty() {
            return 0.;
        }
        let run = gpui::TextRun {
            len: text.len(),
            font: gpui::font("Menlo"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        f32::from(ts.layout_line(&text, px(SIZE), &[run], None).width)
    };
    let empty: Vec<Span> = Vec::new();
    let mut widths = vec![0f32; cols];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (i, w) in widths.iter_mut().enumerate() {
            *w = w.max(measure(row.get(i).unwrap_or(&empty)));
        }
    }
    let line = |row: &[Vec<Span>], ids: &mut MdIds, head: bool| -> gpui::Div {
        let mut r = div().flex().items_start();
        for (i, w) in widths.iter().enumerate() {
            r = r.child(
                div()
                    .flex_none()
                    .w(px(w + PAD * 2.))
                    .px(px(PAD))
                    .py(px(4.))
                    .font_family("Menlo")
                    .text_size(px(SIZE))
                    .when(head, |el| el.font_weight(FontWeight::BOLD))
                    .child(md_inline(row.get(i).unwrap_or(&empty), ids)),
            );
        }
        r
    };
    let mut table = div().flex().flex_col().child(
        line(header, ids, true)
            .border_b_1()
            .border_color(c(theme::EDGE_LIGHT)),
    );
    for (i, row) in rows.iter().enumerate() {
        table = table.child(line(row, ids, false).when(i % 2 == 1, |el| el.bg(ca(theme::EDGE, 0.25))));
    }
    div()
        .id(ids.next("table"))
        .w_full()
        .overflow_x_scroll()
        .rounded(px(8.))
        .bg(c(theme::TERM_BG))
        .border_1()
        .border_color(c(theme::EDGE))
        .text_color(c(theme::TERM_FG))
        .child(table)
        .into_any_element()
}

/// 等宽块（代码 / 表格）：TERM_BG 圆角底、横向滚动、每个源行一行不折行；
/// `lang` 非空时右上角浮一个 FAINT 语言标签（不随内容横向滚动）。
fn md_mono_block(ids: &mut MdIds, kind: &str, text: &str, size: f32, lang: &str) -> AnyElement {
    let body = div()
        .id(ids.next(kind))
        .w_full()
        .overflow_x_scroll()
        .px(px(10.))
        .py(px(8.))
        .child(
            div()
                .font_family("Menlo")
                .text_size(px(size))
                .text_color(c(theme::TERM_FG))
                .whitespace_nowrap()
                .child(SharedString::from(text.to_string())),
        );
    div()
        .relative()
        .w_full()
        .rounded(px(8.))
        .bg(c(theme::TERM_BG))
        // 终端底与页面底太接近，描一圈边才看得出是个块
        .border_1()
        .border_color(c(theme::EDGE))
        .child(body)
        .when(!lang.is_empty(), |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(3.))
                    .right(px(8.))
                    .text_size(px(9.5))
                    .font_family("Menlo")
                    .text_color(c(theme::FAINT))
                    .child(SharedString::from(lang.to_string())),
            )
        })
        .into_any_element()
}

impl Render for MessagesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.text_sys = Some(window.text_system().clone());
        if self.wants_focus && self.supported != Some(false) {
            self.wants_focus = false;
            let fh = self.input.read(cx).focus_handle.clone();
            fh.focus(window, cx);
        }
        let mut root = div()
            .size_full()
            .bg(c(theme::BG))
            .flex()
            .flex_col()
            .on_key_down(cx.listener(Self::on_key_down))
            .child(self.render_stream(cx));
        // 底部 composer：终端仍是权威输入，但看着消息流就能直接回话
        if self.supported != Some(false) {
            root = root.child(self.render_composer(cx));
        }
        root
    }
}

impl MessagesView {
    /// 消息流本体：没消息时是一行居中的占位字，有消息时是可滚动的行列
    /// （外加右下角那个「回到底部」的浮动圆钮）。
    fn render_stream(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if self.msgs.is_empty() && self.queued.is_empty() {
            let msg = match self.supported {
                None => "加载消息流…",
                Some(false) => "该会话不支持消息流（已回落终端）",
                Some(true) => "暂无消息",
            };
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(theme::FAINT))
                .text_size(px(12.))
                .child(msg)
                .into_any_element();
        }
        let pending = self.asking_seq;
        // live = 会话活着且最后一轮还没有回复：过程行画成「进行中」
        let msgs = self.msgs.clone();
        let live = stream_fold::tail_is_live(&msgs, self.running);
        let turns = stream_fold::fold_turns(&msgs, live);
        let items = stream_fold::flatten(&turns, &self.fold_open);
        let mut rows: Vec<gpui::AnyElement> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let el = match item {
                    StreamItem::Fold {
                        fold_key,
                        steps,
                        live_tail,
                        live,
                    } => self.fold_row(*fold_key, steps, *live_tail, *live, cx),
                    StreamItem::Step(m) => self.step_row(m, pending, cx),
                    StreamItem::User(m)
                    | StreamItem::Reply(m)
                    | StreamItem::Question(m)
                    | StreamItem::Answer(m) => self.row(m, pending, cx),
                };
                div().w_full().mt(px(item_gap(i, item))).child(el).into_any_element()
            })
            .collect();
        // v1.22：待发送挂在流末尾——**Claude Code 自己排着的那些**（模型在跑时敲进去的字，
        // 它这一轮结束会自己送进去）。没有撤回：队列是 TUI 的，AAA 只把它画出来。
        for q in &self.queued {
            rows.push(
                div()
                    .w_full()
                    .mt(px(14.))
                    .flex()
                    .flex_col()
                    .items_end()
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(c(theme::AMBER))
                            .mb(px(3.))
                            .child("待发送 · 执行完自动发出"),
                    )
                    .child(pending_bubble(SharedString::from(q.text.clone())))
                    .into_any_element(),
            );
        }
        // v1.16：权限对话框挂在流末尾——以前它只弹在终端里，消息流一无所知
        if let Some(p) = self.permission.clone().filter(|_| self.alive) {
            rows.push(self.permission_card(&p, cx));
        }
        let list = div()
            .id("msgs-scroll")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(16.))
            .py(px(10.))
            .flex()
            .flex_col()
            .children(rows);
        // 右下角浮动 ↓：没在底部时出现，点一下滚到底。滚轮事件会触发重绘，
        // 所以这里按上一帧的 offset 判定就够了
        let show_jump = !self.near_bottom();
        div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .child(list)
            .when(show_jump, |el| {
                el.child(
                    div()
                        .id("msgs-jump-end")
                        .absolute()
                        .bottom(px(14.))
                        .right(px(18.))
                        .w(px(32.))
                        .h(px(32.))
                        .rounded_full()
                        .bg(c(theme::SURFACE_RAISED))
                        .border_1()
                        .border_color(c(theme::EDGE))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(15.))
                        .text_color(c(theme::ACCENT))
                        .cursor_pointer()
                        .hover(|s| s.bg(c(theme::SURFACE)))
                        .on_click(cx.listener(|v: &mut Self, _, _, cx| v.scroll_to_end(cx)))
                        .child("↓"),
                )
            })
            .into_any_element()
    }

    /// 底部输入条：📎 上传 + 输入框 + 发送
    fn render_composer(&self, cx: &mut Context<Self>) -> gpui::Div {
        let send_label = if self.sending { "…" } else { "发送" };
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(8.))
            .border_t_1()
            .border_color(c(theme::EDGE))
            .bg(c(theme::SURFACE))
            .child(
                div()
                    .id("msg-attach")
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(c(if self.uploading { theme::FAINT } else { theme::DIM }))
                    .cursor_pointer()
                    .hover(|s| s.bg(ca(theme::INK, 0.06)))
                    .on_click(cx.listener(|v: &mut Self, _, _, cx| v.attach(cx)))
                    .child(if self.uploading { "…" } else { "📎" }),
            )
            .child(div().flex_1().child(self.input.clone()))
            .child(
                accent_btn("msg-send")
                    .cursor_pointer()
                    .hover(|s| s.opacity(0.85))
                    .on_click(cx.listener(|v: &mut Self, _, _, cx| v.send(cx)))
                    .child(send_label),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(seq: u64, kind: &str) -> ChatMessage {
        ChatMessage {
            seq,
            ts: String::new(),
            role: String::new(),
            kind: kind.into(),
            text: String::new(),
            tool: None,
            question: None,
        }
    }


    #[test]
    fn answered_after_looks_at_next_form_event() {
        let msgs = vec![msg(2, "question"), msg(3, "tool_use"), msg(4, "answer")];
        assert!(answered_after(&msgs, 2));
        // 中间又来一题：旧的那张没答
        let msgs = vec![msg(2, "question"), msg(5, "question"), msg(6, "answer")];
        assert!(!answered_after(&msgs, 2));
        assert!(answered_after(&msgs, 5));
        assert!(!answered_after(&[msg(2, "question")], 2));
    }



    #[test]
    fn time_caption_is_hhmm_in_target_zone() {
        use chrono::{FixedOffset, Utc};
        let ts = "2026-09-02T09:59:59.900Z";
        assert_eq!(time_caption_in(ts, &Utc).as_deref(), Some("09:59"));
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(time_caption_in(ts, &cst).as_deref(), Some("17:59"));
        // 带偏移的输入照样归到目标时区
        assert_eq!(
            time_caption_in("2026-09-02T23:30:00+08:00", &Utc).as_deref(),
            Some("15:30")
        );
        // 裸 ISO 没时区：按字面
        assert_eq!(
            time_caption_in("2026-09-02T10:05", &Utc).as_deref(),
            Some("10:05")
        );
        assert_eq!(
            time_caption_in(" 2026-09-02T10:05:33 ", &Utc).as_deref(),
            Some("10:05")
        );
        // 空 / 垃圾 / 只有日期 → 不显示
        assert_eq!(time_caption_in("", &Utc), None);
        assert_eq!(time_caption_in("yesterday", &Utc), None);
        assert_eq!(time_caption_in("2026-09-02", &Utc), None);
        assert_eq!(time_caption_in("2026-09-02Tab:cd", &Utc), None);
        // 本地时区版本：至少是个 HH:mm 形状
        let local = time_caption(ts).unwrap();
        assert_eq!(local.len(), 5);
        assert_eq!(&local[2..3], ":");
    }

    #[test]
    fn distance_to_bottom_uses_negative_offset() {
        // 顶部：offset 0，可滚 500 → 离底 500
        assert_eq!(distance_to_bottom(px(0.), px(500.)), px(500.));
        // 滚到底：offset = -max
        assert_eq!(distance_to_bottom(px(-500.), px(500.)), px(0.));
        // 差 30px 在余量之内
        assert!(distance_to_bottom(px(-470.), px(500.)) <= px(BOTTOM_SLACK));
        assert!(distance_to_bottom(px(-400.), px(500.)) > px(BOTTOM_SLACK));
        // 内容装得下 / 还没排版：都是 0 → 在底部
        assert_eq!(distance_to_bottom(px(0.), px(0.)), px(0.));
        // 过冲（回弹中）不出负数
        assert_eq!(distance_to_bottom(px(-520.), px(500.)), px(0.));
    }

    #[test]
    fn item_gaps_follow_turn_structure() {
        let u = msg(1, "text");
        let t = msg(2, "tool_use");
        assert_eq!(item_gap(0, &StreamItem::User(&u)), 0.);
        assert_eq!(item_gap(3, &StreamItem::User(&u)), 14.);
        assert_eq!(item_gap(3, &StreamItem::Step(&t)), 3.);
        assert_eq!(item_gap(3, &StreamItem::Reply(&u)), 8.);
        assert_eq!(
            item_gap(
                3,
                &StreamItem::Fold {
                    fold_key: 1,
                    steps: vec![&t],
                    live_tail: None,
                    live: false
                }
            ),
            8.
        );
    }

}
