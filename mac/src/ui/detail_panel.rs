//! 会话页右侧的详情面板（⌘I 切换，宽 300）+ 侧栏底部的套餐用量块。
//!
//! 面板五段：会话用量（模型 / 上下文条 / 费用 / 行数 / 时长）、产物（发布过的
//! Artifact，点开浏览器）、改动（start 检查点 vs 工作区，可展开 patch、可回滚）、
//! 收件箱（项目任务清单，Claude 空下来时 daemon 自动喂）、通知（按项目静音）。
//!
//! 拉取节流：产物 / 改动各有一个 [`Throttle`]——`messages_changed` 帧来得很密
//! （daemon 侧 ≥500ms 一帧），这里按「间隔内最多一次、间隔末尾补一次」收口：
//! 间隔外立刻拉；间隔内只挂一个定时器，到点再拉一次（不丢最后一次变化）；
//! 定时器已挂着时再来的帧直接忽略。

use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, TimeZone, Weekday};
use gpui::{Context, SharedString, div, prelude::*, px, relative};

use super::kit::*;
use super::{Page, RootView};
use crate::model::{parse_checklist, 
    Artifact, InboxItem, PlanUsage, Session, SessionUsage, is_muted, toggle_muted,
};
use crate::theme;

/// 面板宽度
pub(super) const DETAIL_W: f32 = 300.0;
/// 产物的最小重拉间隔
const ARTIFACTS_MIN: Duration = Duration::from_secs(2);

// ── 纯函数 ──────────────────────────────────────────────────────────────────

/// 节流器的判定结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    /// 立刻拉
    Now,
    /// 间隔内：这么久之后补拉一次
    Defer(Duration),
    /// 已经挂了补拉定时器，什么都不做
    Skip,
}

/// 「间隔内最多一次、末尾补一次」的节流器（纯状态机，时间由调用方传入便于测试）
#[derive(Debug, Clone)]
pub(super) struct Throttle {
    min: Duration,
    last: Option<Instant>,
    scheduled: bool,
}

impl Throttle {
    pub(super) fn new(min: Duration) -> Self {
        Throttle {
            min,
            last: None,
            scheduled: false,
        }
    }

    /// 有一次拉取需求
    pub(super) fn request(&mut self, now: Instant) -> Decision {
        if self.scheduled {
            return Decision::Skip;
        }
        match self.last {
            Some(last) if now.duration_since(last) < self.min => {
                self.scheduled = true;
                Decision::Defer(self.min - now.duration_since(last))
            }
            _ => {
                self.last = Some(now);
                Decision::Now
            }
        }
    }

    /// 补拉定时器到点：记下这次时间，随后调用方真正去拉
    pub(super) fn fire(&mut self, now: Instant) {
        self.scheduled = false;
        self.last = Some(now);
    }
}

/// 百分比的警戒级别：≥90 红、≥70 琥珀、其余正常
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Level {
    Ok,
    Warn,
    Crit,
}

pub(super) fn pct_level(pct: f64) -> Level {
    if pct >= 90.0 {
        Level::Crit
    } else if pct >= 70.0 {
        Level::Warn
    } else {
        Level::Ok
    }
}

/// 级别 → 颜色；正常级别用调用方给的底色（侧栏是 dim、上下文条是主色）
pub(super) fn level_color(level: Level, ok: u32) -> u32 {
    match level {
        Level::Ok => ok,
        Level::Warn => theme::amber(),
        Level::Crit => theme::red(),
    }
}

fn weekday_zh(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "周一",
        Weekday::Tue => "周二",
        Weekday::Wed => "周三",
        Weekday::Thu => "周四",
        Weekday::Fri => "周五",
        Weekday::Sat => "周六",
        Weekday::Sun => "周日",
    }
}

/// 重置时刻的短写：今天 → `14:30`；一周内 → `周四`；更远 → `9/12`
pub(super) fn fmt_reset<Tz: TimeZone>(t: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let (d, today) = (t.date_naive(), now.date_naive());
    if d == today {
        return t.format("%H:%M").to_string();
    }
    let days = (d - today).num_days();
    if (1..7).contains(&days) {
        weekday_zh(d.weekday()).to_string()
    } else {
        format!("{}/{}", d.month(), d.day())
    }
}

/// 产物时间：今天 → `HH:mm`；否则 `M月D日`。解析不了不显示
pub(super) fn fmt_artifact_time<Tz: TimeZone>(ts: &str, now: &DateTime<Tz>, tz: &Tz) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    let t = DateTime::parse_from_rfc3339(ts.trim()).ok()?.with_timezone(tz);
    Some(if t.date_naive() == now.date_naive() {
        t.format("%H:%M").to_string()
    } else {
        format!("{}月{}日", t.month(), t.day())
    })
}

/// 时长人话：`45 秒` / `3 分 12 秒` / `1 小时 5 分` / `2 天 3 小时`
pub(super) fn humanize_ms(ms: u64) -> String {
    let s = ms / 1000;
    let (d, h, m, sec) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60, s % 60);
    if d > 0 {
        format!("{d} 天 {h} 小时")
    } else if h > 0 {
        format!("{h} 小时 {m} 分")
    } else if m > 0 {
        format!("{m} 分 {sec} 秒")
    } else {
        format!("{sec} 秒")
    }
}

/// 费用：两位小数；不足一分但非零显示 `<$0.01`
pub(super) fn fmt_cost(usd: f64) -> String {
    if usd > 0.0 && usd < 0.005 {
        "<$0.01".to_string()
    } else {
        format!("${usd:.2}")
    }
}

/// 产物按时间倒序（ISO 串字典序即时间序；同刻按 url 稳住）
pub(super) fn sort_artifacts_newest_first(v: &mut [Artifact]) {
    v.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| a.url.cmp(&b.url)));
}

/// 套餐块第一行的各段：`(标签, 百分比)`，顺序 5h、7d、各模型窗口；没数的窗口不出现
pub(super) fn plan_parts(plan: &PlanUsage) -> Vec<(String, f64)> {
    let mut v = Vec::new();
    if let Some(p) = plan.five_hour.as_ref().and_then(|w| w.used_percentage) {
        v.push(("5h".to_string(), p));
    }
    if let Some(p) = plan.seven_day.as_ref().and_then(|w| w.used_percentage) {
        v.push(("7d".to_string(), p));
    }
    for m in plan.model_scoped.iter().flatten() {
        if let Some(p) = m.utilization {
            let name = if m.display_name.is_empty() { "模型" } else { m.display_name.as_str() };
            v.push((name.to_string(), p));
        }
    }
    v
}

/// 套餐块第二行：`5h 重置 14:30` 这样的段，只列解析得出重置时刻的窗口
pub(super) fn plan_resets<Tz: TimeZone>(plan: &PlanUsage, now: &DateTime<Tz>, tz: &Tz) -> Vec<String>
where
    Tz::Offset: std::fmt::Display,
{
    let mut v = Vec::new();
    let mut push = |label: &str, at: &Option<crate::model::ResetsAt>| {
        if let Some(t) = at.as_ref().and_then(|r| r.to_utc()) {
            v.push(format!("{label} 重置 {}", fmt_reset(&t.with_timezone(tz), now)));
        }
    };
    if let Some(w) = &plan.five_hour {
        push("5h", &w.resets_at);
    }
    if let Some(w) = &plan.seven_day {
        push("7d", &w.resets_at);
    }
    for m in plan.model_scoped.iter().flatten() {
        let name = if m.display_name.is_empty() { "模型" } else { m.display_name.as_str() };
        push(name, &m.resets_at);
    }
    v
}

// ── 每会话的面板状态 ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailKind {
    Artifacts,
}

pub(super) struct SessionDetail {
    pub artifacts: Vec<Artifact>,
    pub artifacts_fetch: Throttle,
}

impl Default for SessionDetail {
    fn default() -> Self {
        SessionDetail {
            artifacts: Vec::new(),
            artifacts_fetch: Throttle::new(ARTIFACTS_MIN),
        }
    }
}

impl RootView {
    // ── 状态 / 拉取 ────────────────────────────────────────────────────

    /// ⌘I / 状态栏「详情」：开关面板并落盘；打开时把当前会话的数据补齐
    pub(super) fn toggle_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_visible = !self.detail_visible;
        self.ui_state().save();
        if self.detail_visible
            && let Page::Session(id) = self.page.clone()
        {
            self.refresh_detail(&id, cx);
        }
        cx.notify();
    }

    /// 面板正显示着这个会话
    fn detail_showing(&self, id: &str) -> bool {
        self.detail_visible && self.page == Page::Session(id.to_string())
    }

    /// 进入会话页 / 打开面板：产物、收件箱都（按节流）拉一遍
    pub(super) fn refresh_detail(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.detail_showing(id) || self.session(id).is_none_or(Session::is_terminal) {
            return;
        }
        self.request_detail_fetch(id, DetailKind::Artifacts, cx);
        if let Some(path) = self.session(id).map(|s| s.project_path.clone())
            && !path.is_empty()
            && !self.inbox.contains_key(&path)
        {
            self.fetch_inbox(path, cx);
        }
    }

    /// `messages_changed`：面板正看着它才重拉（不做无谓轮询），节流见模块注释
    pub(super) fn on_detail_messages_changed(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.detail_showing(id) {
            return;
        }
        self.request_detail_fetch(id, DetailKind::Artifacts, cx);
    }

    /// 会话没了：面板状态一起丢
    pub(super) fn forget_detail(&mut self, id: &str) {
        self.detail.remove(id);
    }

    fn request_detail_fetch(&mut self, id: &str, kind: DetailKind, cx: &mut Context<Self>) {
        let now = Instant::now();
        let d = self.detail.entry(id.to_string()).or_default();
        let t = match kind {
            DetailKind::Artifacts => &mut d.artifacts_fetch,
        };
        match t.request(now) {
            Decision::Now => self.fetch_detail_now(id, kind, cx),
            Decision::Defer(delay) => {
                let id = id.to_string();
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |r, cx| {
                        // 会话已被删就算了
                        if let Some(d) = r.detail.get_mut(&id) {
                            match kind {
                                DetailKind::Artifacts => d.artifacts_fetch.fire(Instant::now()),
                            }
                            r.fetch_detail_now(&id, kind, cx);
                        }
                    });
                })
                .detach();
            }
            Decision::Skip => {}
        }
    }

    fn fetch_detail_now(&mut self, id: &str, kind: DetailKind, cx: &mut Context<Self>) {
        let sid = id.to_string();
        match kind {
            DetailKind::Artifacts => {
                let fut = self.net.artifacts(id);
                self.spawn_fetch(
                    fut,
                    move |r, resp: crate::model::ArtifactsResponse, cx| {
                        let mut list = resp.artifacts;
                        sort_artifacts_newest_first(&mut list);
                        r.detail.entry(sid).or_default().artifacts = list;
                        cx.notify();
                    },
                    false,
                    cx,
                );
            }
        }
    }

    /// 套餐用量：连上时拉一次，之后靠 `usage` 帧
    pub(super) fn fetch_usage(&mut self, cx: &mut Context<Self>) {
        self.spawn_fetch(
            self.net.usage(),
            |r, u: crate::model::UsageResponse, cx| {
                r.plan = u.plan;
                cx.notify();
            },
            false,
            cx,
        );
    }

    pub(super) fn fetch_inbox(&mut self, path: String, cx: &mut Context<Self>) {
        let fut = self.net.inbox(&path);
        self.spawn_fetch(
            fut,
            move |r, items: Vec<InboxItem>, cx| {
                r.inbox.insert(path, items);
                cx.notify();
            },
            false,
            cx,
        );
    }

    /// `inbox_changed` 帧：正看着这个项目就重拉，否则丢掉缓存等下次打开
    pub(super) fn on_inbox_changed(&mut self, path: &str, cx: &mut Context<Self>) {
        let showing = match &self.page {
            Page::Session(id) => self
                .session(id)
                .is_some_and(|s| s.project_path.trim_end_matches('/') == path.trim_end_matches('/')),
            _ => false,
        };
        if showing && self.detail_visible {
            self.fetch_inbox(path.to_string(), cx);
        } else {
            self.inbox.remove(path);
        }
    }

    /// 当前会话所属项目的路径（面板的收件箱 / 静音都按它）
    fn current_project_path(&self) -> Option<String> {
        let Page::Session(id) = &self.page else { return None };
        self.session(id)
            .map(|s| s.project_path.clone())
            .filter(|p| !p.is_empty())
    }

    /// 收件箱输入框回车：POST 后清空；列表靠 inbox_changed 帧或这里的乐观重拉对齐
    pub(super) fn inbox_add(&mut self, cx: &mut Context<Self>) {
        let text = self.inbox_input.read(cx).text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(path) = self.current_project_path() else { return };
        self.inbox_input.update(cx, |i, cx| i.set_text("", cx));
        let fut = self.net.inbox_add(&path, &text);
        self.spawn_fetch(
            fut,
            move |r, _: serde_json::Value, cx| r.fetch_inbox(path, cx),
            true,
            cx,
        );
    }

    fn inbox_delete(&mut self, path: String, id: String, cx: &mut Context<Self>) {
        // 乐观删除，失败再拉回来
        if let Some(items) = self.inbox.get_mut(&path) {
            items.retain(|i| i.id != id);
        }
        let fut = self.net.inbox_delete(&id);
        let path2 = path.clone();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let _ = this.update(cx, |r, cx| {
                if let Err(e) = res {
                    r.set_error(format!("删除收件箱条目失败: {e}"), cx);
                }
                r.fetch_inbox(path2, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_mute_current(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.current_project_path() {
            toggle_muted(&mut self.muted_projects, &path);
            self.ui_state().save();
            cx.notify();
        }
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    /// 段标题：与设置页同款的 Menlo 小字
    fn sect_label(text: &'static str) -> gpui::Div {
        div()
            .font_family("Menlo")
            .text_size(px(10.))
            .text_color(c(theme::faint()))
            .pb(px(6.))
            .child(text)
    }

    fn empty_hint(text: &'static str) -> gpui::Div {
        div()
            .text_size(px(11.5))
            .text_color(c(theme::faint()))
            .child(text)
    }

    fn section(label: &'static str, body: impl IntoElement) -> gpui::Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .px(px(14.))
            .py(px(12.))
            .border_b_1()
            .border_color(ca(theme::edge(), 0.7))
            .child(Self::sect_label(label))
            .child(body)
    }

    /// 面板本体；不在会话页 / 面板收起 / 终端会话 → None
    pub(super) fn render_detail_panel(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        if !self.detail_visible {
            return None;
        }
        let Page::Session(id) = &self.page else { return None };
        let s = self.session(id)?;
        if s.is_terminal() {
            return None;
        }
        let d = self.detail.get(id);
        let now = chrono::Local::now();

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .h(px(30.))
            .px(px(14.))
            .border_b_1()
            .border_color(c(theme::edge()))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(c(theme::ink()))
                    .child("详情"),
            )
            .child(
                div()
                    .font_family("Menlo")
                    .text_size(px(10.))
                    .text_color(c(theme::faint()))
                    .pr(px(8.))
                    .child("⌘I"),
            )
            .child(
                div()
                    .id("detail-close")
                    .px(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(c(theme::faint()))
                    .hover(|st| st.text_color(c(theme::ink())).bg(c(theme::edge_light())))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_detail(cx)))
                    .child("✕"),
            );

        let body = div()
            .id("detail-scroll")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .child(Self::section("会话", Self::render_usage_section(s.usage.as_ref())))
            .child(Self::section("进度", Self::render_checklist_section(s)))
            .child(Self::section("产物", self.render_artifacts_section(d, &now, cx)))
            .child(Self::section("收件箱", self.render_inbox_section(s, cx)))
            .child(Self::section("通知", self.render_notify_section(s, cx)));

        Some(
            div()
                .w(px(DETAIL_W))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .bg(c(theme::surface()))
                .border_l_1()
                .border_color(c(theme::edge()))
                .child(header)
                .child(body),
        )
    }

    /// 整个对话的进度清单（daemon 在每次 Stop 后让 haiku 重写）：☑ 已做、☐ 未做
    fn render_checklist_section(s: &Session) -> gpui::Div {
        let items = parse_checklist(&s.summary);
        if items.is_empty() {
            return Self::empty_hint("每轮回复结束后这里会更新一份「做了什么 / 还没做什么」");
        }
        let (done, total) = (items.iter().filter(|i| i.done).count(), items.len());
        let mut col = div().flex().flex_col().gap(px(5.));
        col = col.child(
            div()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(c(theme::faint()))
                .pb(px(2.))
                .child(SharedString::from(format!("{done} / {total} 完成"))),
        );
        for it in items {
            col = col.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(7.))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.))
                            .text_color(c(if it.done { theme::green() } else { theme::faint() }))
                            .child(if it.done { "☑" } else { "☐" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(12.))
                            .text_color(c(if it.done { theme::dim() } else { theme::ink() }))
                            .child(SharedString::from(it.text)),
                    ),
            );
        }
        col
    }

    fn render_usage_section(usage: Option<&SessionUsage>) -> gpui::Div {
        let Some(u) = usage else {
            return Self::empty_hint("还没有用量数据");
        };
        let mono = |text: String, color: u32| {
            div()
                .font_family("Menlo")
                .text_size(px(11.))
                .text_color(c(color))
                .child(SharedString::from(text))
        };
        let mut col = div().flex().flex_col().gap(px(8.));

        // 模型 + effort 芯片
        let mut model_row = div().flex().items_center().gap(px(6.)).min_w(px(0.));
        let model = u
            .model
            .clone()
            .or_else(|| u.model_id.clone())
            .unwrap_or_else(|| "未知模型".into());
        model_row = model_row.child(
            div()
                .truncate()
                .text_size(px(12.5))
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(c(theme::ink()))
                .child(SharedString::from(model)),
        );
        if let Some(effort) = u.effort.as_deref().filter(|e| !e.is_empty()) {
            model_row = model_row.child(
                div()
                    .flex_none()
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(ca(theme::accent(), 0.14))
                    .font_family("Menlo")
                    .text_size(px(9.5))
                    .text_color(c(theme::accent()))
                    .child(SharedString::from(effort.to_string())),
            );
        }
        col = col.child(model_row);

        // 上下文条
        if let Some(pct) = u.context_pct {
            let pct = pct.clamp(0.0, 100.0);
            let color = level_color(pct_level(pct), theme::accent());
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(c(theme::dim()))
                                    .child("上下文"),
                            )
                            .child(mono(format!("{}%", pct.round() as i64), color)),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(c(theme::edge()))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative((pct / 100.0) as f32))
                                    .rounded(px(2.))
                                    .bg(c(color)),
                            ),
                    ),
            );
        }

        // 提示缓存：最近一次调用的输入构成——命中率 = 缓存读 ÷ (缓存读 + 新写 + 新读)。
        // 有「缓存读」就说明这段对话的缓存还活着；断了一会儿再聊，读会变成 0、写变大
        if let Some(hit) = u.cache_hit_pct {
            let k = |n: Option<u64>| match n {
                Some(n) if n >= 1000 => format!("{:.1}k", n as f64 / 1000.0),
                Some(n) => n.to_string(),
                None => "—".into(),
            };
            let alive = u.cache_read_tokens.unwrap_or(0) > 0;
            col = col.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(11.)).text_color(c(theme::dim())).child("缓存"))
                    .child(mono(
                        format!(
                            "{} · 命中 {}% · 读 {} 写 {} 新 {}",
                            if alive { "在" } else { "无" },
                            hit.round() as i64,
                            k(u.cache_read_tokens),
                            k(u.cache_creation_tokens),
                            k(u.fresh_input_tokens)
                        ),
                        if alive { theme::green() } else { theme::faint() },
                    )),
            );
        }
        // 费用 · 行数 · 时长
        let mut stats = div().flex().flex_wrap().items_center().gap(px(10.));
        if let Some(cost) = u.cost_usd {
            stats = stats.child(mono(fmt_cost(cost), theme::ink()));
        }
        if u.lines_added.is_some() || u.lines_removed.is_some() {
            stats = stats.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(mono(format!("+{}", u.lines_added.unwrap_or(0)), theme::green()))
                    .child(mono(format!("−{}", u.lines_removed.unwrap_or(0)), theme::red()))
                    .child(div().text_size(px(11.)).text_color(c(theme::dim())).child("行")),
            );
        }
        if let Some(ms) = u.duration_ms {
            stats = stats.child(mono(humanize_ms(ms), theme::dim()));
        }
        col.child(stats)
    }

    fn render_artifacts_section(
        &self,
        d: Option<&SessionDetail>,
        now: &DateTime<chrono::Local>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let list: &[Artifact] = d.map(|d| d.artifacts.as_slice()).unwrap_or(&[]);
        if list.is_empty() {
            return Self::empty_hint("这个会话还没有发布产物");
        }
        let mut col = div().flex().flex_col().gap(px(2.));
        for (ix, a) in list.iter().enumerate() {
            let url = a.url.clone();
            let time = fmt_artifact_time(&a.ts, now, &chrono::Local).unwrap_or_default();
            let title = if a.title.is_empty() { a.url.clone() } else { a.title.clone() };
            col = col.child(
                div()
                    .id(("artifact", ix))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(8.))
                    .py(px(6.))
                    .mx(px(-8.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|st| st.bg(c(theme::surface_raised())))
                    .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(12.))
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(c(theme::ink()))
                                    .child(SharedString::from(title)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .font_family("Menlo")
                                    .text_size(px(10.))
                                    .text_color(c(theme::faint()))
                                    .child(SharedString::from(time)),
                            ),
                    )
                    .when(!a.description.is_empty(), |el| {
                        el.child(
                            div()
                                .line_clamp(2)
                                .text_size(px(11.))
                                .text_color(c(theme::dim()))
                                .child(SharedString::from(a.description.clone())),
                        )
                    }),
            );
        }
        col
    }

    fn render_inbox_section(&self, s: &Session, cx: &mut Context<Self>) -> gpui::Div {
        let path = s.project_path.clone();
        let items: &[InboxItem] = self.inbox.get(&path).map(|v| v.as_slice()).unwrap_or(&[]);
        let mut col = div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(self.inbox_input.clone());
        if items.is_empty() {
            col = col.child(Self::empty_hint("收件箱是空的"));
        }
        for (ix, it) in items.iter().enumerate() {
            let (p, id) = (path.clone(), it.id.clone());
            col = col.child(
                div()
                    .id(("inbox", ix))
                    .group("inbox-row")
                    .flex()
                    .items_start()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(4.))
                    .mx(px(-8.))
                    .rounded(px(5.))
                    .hover(|st| st.bg(c(theme::surface_raised())))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(12.))
                            .text_color(c(theme::ink()))
                            .child(SharedString::from(it.text.clone())),
                    )
                    .child(
                        div()
                            .id(("inbox-del", ix))
                            .flex_none()
                            .px(px(3.))
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(c(theme::faint()))
                            .cursor_pointer()
                            .hover(|st| st.text_color(c(theme::red())).bg(c(theme::edge_light())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.inbox_delete(p.clone(), id.clone(), cx);
                            }))
                            .child("✕"),
                    ),
            );
        }
        col
    }

    fn render_notify_section(&self, s: &Session, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let on = is_muted(&self.muted_projects, &s.project_path);
        let knob = div()
            .flex_none()
            .w(px(30.))
            .h(px(16.))
            .rounded_full()
            .p(px(2.))
            .flex()
            .items_center()
            .when(on, |el| el.justify_end())
            .bg(c(if on { theme::accent() } else { theme::edge_light() }))
            .child(
                div()
                    .w(px(12.))
                    .h(px(12.))
                    .rounded_full()
                    .bg(c(if on { theme::on_accent() } else { theme::ink() })),
            );
        div()
            .id("mute-toggle")
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle_mute_current(cx)))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(c(theme::ink()))
                    .child("静音此项目的通知"),
            )
            .child(knob)
    }

    /// 侧栏最底部的套餐用量块；plan 为 null / 没有任何窗口有数就整块不画
    pub(super) fn render_plan_usage(&self) -> Option<gpui::Div> {
        let plan = self.plan.as_ref()?;
        let parts = plan_parts(plan);
        if parts.is_empty() {
            return None;
        }
        let now = chrono::Local::now();
        let resets = plan_resets(plan, &now, &chrono::Local);

        let mut line1 = div()
            .flex()
            .flex_wrap()
            .items_center()
            .font_family("Menlo")
            .text_size(px(11.));
        for (ix, (label, pct)) in parts.iter().enumerate() {
            if ix > 0 {
                line1 = line1.child(div().px(px(4.)).text_color(c(theme::faint())).child("·"));
            }
            let color = level_color(pct_level(*pct), theme::dim());
            line1 = line1.child(
                div()
                    .text_color(c(color))
                    .child(SharedString::from(format!("{label} {}%", pct.round() as i64))),
            );
        }
        let mut block = div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.))
            .px(px(16.))
            .py(px(7.))
            .border_t_1()
            .border_color(c(theme::edge()))
            .child(line1);
        if !resets.is_empty() {
            block = block.child(
                div()
                    .font_family("Menlo")
                    .text_size(px(10.))
                    .text_color(c(theme::faint()))
                    .truncate()
                    .child(SharedString::from(resets.join(" · "))),
            );
        }
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelWindow, PlanWindow, ResetsAt};
    use chrono::{FixedOffset, Utc};

    #[test]
    fn throttle_now_defer_skip_fire() {
        let t0 = Instant::now();
        let mut t = Throttle::new(Duration::from_secs(2));
        // 第一次立刻拉
        assert_eq!(t.request(t0), Decision::Now);
        // 500ms 后又来：间隔内，1.5s 后补拉
        assert_eq!(
            t.request(t0 + Duration::from_millis(500)),
            Decision::Defer(Duration::from_millis(1500))
        );
        // 定时器挂着时再来的一律忽略
        assert_eq!(t.request(t0 + Duration::from_millis(900)), Decision::Skip);
        assert_eq!(t.request(t0 + Duration::from_millis(1900)), Decision::Skip);
        // 到点补拉：视作一次拉取
        t.fire(t0 + Duration::from_secs(2));
        // 补拉后 1s 又来 → 再挂一个 1s 的定时器
        assert_eq!(
            t.request(t0 + Duration::from_secs(3)),
            Decision::Defer(Duration::from_secs(1))
        );
        t.fire(t0 + Duration::from_secs(4));
        // 间隔外 → 立刻
        assert_eq!(t.request(t0 + Duration::from_secs(7)), Decision::Now);
        // 恰好等于间隔也算间隔外
        assert_eq!(t.request(t0 + Duration::from_secs(9)), Decision::Now);
    }

    #[test]
    fn percent_levels() {
        assert_eq!(pct_level(0.0), Level::Ok);
        assert_eq!(pct_level(69.9), Level::Ok);
        assert_eq!(pct_level(70.0), Level::Warn);
        assert_eq!(pct_level(89.9), Level::Warn);
        assert_eq!(pct_level(90.0), Level::Crit);
        assert_eq!(pct_level(150.0), Level::Crit);
        // 正常级别用调用方的底色，警戒级别用主题色
        assert_eq!(level_color(Level::Ok, 0x123456), 0x123456);
        assert_eq!(level_color(Level::Warn, 0x123456), theme::amber());
        assert_eq!(level_color(Level::Crit, 0x123456), theme::red());
    }

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn reset_time_formatting() {
        let now = at("2026-09-03T10:00:00+08:00"); // 周四
        // 今天 → 时刻
        assert_eq!(fmt_reset(&at("2026-09-03T14:30:00+08:00"), &now), "14:30");
        // 明天到六天后 → 周几
        assert_eq!(fmt_reset(&at("2026-09-04T06:00:00+08:00"), &now), "周五");
        assert_eq!(fmt_reset(&at("2026-09-06T06:00:00+08:00"), &now), "周日");
        assert_eq!(fmt_reset(&at("2026-09-09T06:00:00+08:00"), &now), "周三");
        // 七天及以上 → 月/日
        assert_eq!(fmt_reset(&at("2026-09-10T06:00:00+08:00"), &now), "9/10");
        assert_eq!(fmt_reset(&at("2026-10-01T06:00:00+08:00"), &now), "10/1");
        // 过去（已经重置了）→ 月/日
        assert_eq!(fmt_reset(&at("2026-09-01T06:00:00+08:00"), &now), "9/1");
        // 今天但时区不同的输入：比较前应先换到同一时区（调用方负责），这里只验同 tz
        assert_eq!(fmt_reset(&at("2026-09-03T00:05:00+08:00"), &now), "00:05");
    }

    #[test]
    fn artifact_time_formatting() {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let now = at("2026-09-03T10:00:00+08:00");
        // UTC 输入换到本地时区后判「今天」
        assert_eq!(
            fmt_artifact_time("2026-09-03T01:05:00Z", &now, &tz).as_deref(),
            Some("09:05")
        );
        // UTC 的 9/2 18:00 = 本地 9/3 02:00，仍是今天
        assert_eq!(
            fmt_artifact_time("2026-09-02T18:00:00Z", &now, &tz).as_deref(),
            Some("02:00")
        );
        assert_eq!(
            fmt_artifact_time("2026-09-01T18:00:00Z", &now, &tz).as_deref(),
            Some("9月2日")
        );
        assert_eq!(fmt_artifact_time("", &now, &tz), None);
        assert_eq!(fmt_artifact_time("yesterday", &now, &tz), None);
    }

    #[test]
    fn duration_and_cost() {
        assert_eq!(humanize_ms(0), "0 秒");
        assert_eq!(humanize_ms(45_000), "45 秒");
        assert_eq!(humanize_ms(192_000), "3 分 12 秒");
        assert_eq!(humanize_ms(3_900_000), "1 小时 5 分");
        assert_eq!(humanize_ms(183_600_000), "2 天 3 小时");
        assert_eq!(fmt_cost(1.25), "$1.25");
        assert_eq!(fmt_cost(0.0), "$0.00");
        assert_eq!(fmt_cost(0.004), "<$0.01");
        assert_eq!(fmt_cost(0.005), "$0.01");
        assert_eq!(fmt_cost(12.0), "$12.00");
    }

    #[test]
    fn artifacts_sorted_newest_first() {
        let mk = |ts: &str, url: &str| Artifact {
            ts: ts.into(),
            url: url.into(),
            ..Default::default()
        };
        let mut v = vec![
            mk("2026-09-03T10:00:00Z", "b"),
            mk("2026-09-03T12:00:00Z", "c"),
            mk("2026-09-01T10:00:00Z", "a"),
            mk("2026-09-03T10:00:00Z", "a"),
        ];
        sort_artifacts_newest_first(&mut v);
        let order: Vec<(&str, &str)> = v.iter().map(|a| (a.ts.as_str(), a.url.as_str())).collect();
        assert_eq!(
            order,
            [
                ("2026-09-03T12:00:00Z", "c"),
                ("2026-09-03T10:00:00Z", "a"),
                ("2026-09-03T10:00:00Z", "b"),
                ("2026-09-01T10:00:00Z", "a"),
            ]
        );
    }

    #[test]
    fn plan_lines() {
        let plan = PlanUsage {
            five_hour: Some(PlanWindow {
                used_percentage: Some(32.4),
                resets_at: Some(ResetsAt::Text("2026-09-03T06:30:00Z".into())),
            }),
            seven_day: Some(PlanWindow {
                used_percentage: Some(61.0),
                resets_at: Some(ResetsAt::Epoch(
                    at("2026-09-06T00:00:00+08:00").with_timezone(&Utc).timestamp() as f64,
                )),
            }),
            model_scoped: Some(vec![
                ModelWindow {
                    display_name: "Fable".into(),
                    utilization: Some(40.0),
                    resets_at: None,
                },
                // 没数的窗口不出现
                ModelWindow {
                    display_name: "Opus".into(),
                    utilization: None,
                    resets_at: None,
                },
            ]),
        };
        let parts = plan_parts(&plan);
        assert_eq!(
            parts,
            vec![("5h".to_string(), 32.4), ("7d".to_string(), 61.0), ("Fable".to_string(), 40.0)]
        );
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let now = at("2026-09-03T10:00:00+08:00");
        // 5h 的 06:30Z = 本地 14:30 今天；7d 的 9/6 = 周日；Fable 没有重置时刻
        assert_eq!(plan_resets(&plan, &now, &tz), vec!["5h 重置 14:30", "7d 重置 周日"]);
        // 空套餐 → 没有段（侧栏整块隐藏）
        assert!(plan_parts(&PlanUsage::default()).is_empty());
        assert!(plan_resets(&PlanUsage::default(), &now, &tz).is_empty());
        // 只有一个窗口有数
        let only7 = PlanUsage {
            seven_day: Some(PlanWindow {
                used_percentage: Some(95.0),
                resets_at: None,
            }),
            ..Default::default()
        };
        assert_eq!(plan_parts(&only7), vec![("7d".to_string(), 95.0)]);
    }
}
