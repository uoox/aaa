//! 会话页右侧的详情面板（⌘I 切换，宽 300）+ 侧栏底部的套餐用量块。
//!
//! 面板五段：会话用量（模型 / 上下文条 / 费用 / 行数 / 时长）、产物（发布过的
//! Artifact，点开浏览器）、改动（start 检查点 vs 工作区，可展开 patch、可回滚）、
//!
//! 拉取节流：整个面板共用一个 [`Throttle`]——`messages_changed` 帧来得很密
//! （daemon 侧 ≥500ms 一帧），这里按「间隔内最多一次、间隔末尾补一次」收口：
//! 间隔外立刻拉；间隔内只挂一个定时器，到点再拉一次（不丢最后一次变化）；
//! 定时器已挂着时再来的帧直接忽略。**末尾那次补拉不能省**——变化正好落在间隔内
//! 时，朴素的「距上次够久才拉」会把它整个丢掉，面板僵在旧数据上。
//!
//! 2026-09-08：以前产物和 extras 各有一个节流器、由一个 `DetailKind` 选择走哪个，
//! 但两个调用点从来都是两样一起要，枚举永远只有一个取值。合成一个。

use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, TimeZone, Weekday};
use gpui::{Context, SharedString, div, prelude::*, px, relative};

use super::kit::*;
use super::{Page, RootView};
use crate::model::{
    Artifact, PlanUsage, Session, SessionDetailResponse, SessionUsage,
};
use crate::theme::{self, human_bytes};

/// 面板宽度
pub(super) const DETAIL_W: f32 = 300.0;

/// 详情栏里的一行：`(行首记号, 标题, 小字, 行尾时间, 点开看的正文)`。
/// 最后一项 `(key, body)`：`key` 是展开状态的键（会话内唯一），`body` 是正文；
/// None = 这一行没有可看的正文，也就不可点。
type DetailRow = (Option<(&'static str, u32)>, String, String, String, Option<(String, String)>);
/// 面板的最小重拉间隔
const DETAIL_MIN: Duration = Duration::from_secs(2);

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
        Level::Warn => theme::AMBER,
        Level::Crit => theme::RED,
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

pub(super) struct SessionDetail {
    pub artifacts: Vec<Artifact>,
    /// `GET /sessions/:id/detail` 的结果；老 daemon 404 时留空表
    pub extras: SessionDetailResponse,
    /// 两样一起拉，一个节流器管着
    pub fetch: Throttle,
}

impl Default for SessionDetail {
    fn default() -> Self {
        SessionDetail {
            artifacts: Vec::new(),
            extras: SessionDetailResponse::default(),
            fetch: Throttle::new(DETAIL_MIN),
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

    /// 进入会话页 / 打开面板 / `messages_changed` 帧：面板正看着这个会话才拉
    /// （不做无谓轮询），节流见模块注释。终端没有详情。
    pub(super) fn refresh_detail(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.detail_showing(id) || self.session(id).is_none_or(Session::is_terminal) {
            return;
        }
        let now = Instant::now();
        match self.detail.entry(id.to_string()).or_default().fetch.request(now) {
            Decision::Now => self.fetch_detail_now(id, cx),
            Decision::Defer(delay) => {
                let id = id.to_string();
                // 不走 spawn_fetch：这里等的是一个定时器，不是一个请求
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |r, cx| {
                        // 会话已被删就算了
                        if let Some(d) = r.detail.get_mut(&id) {
                            d.fetch.fire(Instant::now());
                            r.fetch_detail_now(&id, cx);
                        }
                    });
                })
                .detach();
            }
            Decision::Skip => {}
        }
    }

    /// 会话没了：面板状态一起丢
    pub(super) fn forget_detail(&mut self, id: &str) {
        self.detail.remove(id);
    }

    /// 产物和 extras 两个接口一起打（并发，两个 spawn）
    fn fetch_detail_now(&mut self, id: &str, cx: &mut Context<Self>) {
        let sid = id.to_string();
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
        let sid = id.to_string();
        let fut = self.net.session_detail(id);
        self.spawn_fetch(
            fut,
            move |r, resp: crate::model::SessionDetailResponse, cx| {
                r.detail.entry(sid).or_default().extras = resp;
                cx.notify();
            },
            false,
            cx,
        );
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

    /// 当前会话所属项目的路径。**去尾斜杠**再用：daemon 发的路径是 realpath 过的，
    /// 而会话行上的可能带尾斜杠，裸字符串比会把 `/p/a` 和 `/p/a/` 当成两个项目
    /// （与未读黄点同一个口径）。
    pub(super) fn current_project_path(&self) -> Option<String> {
        let Page::Session(id) = &self.page else { return None };
        self.session(id)
            .map(|s| s.project_path.trim_end_matches('/').to_string())
            .filter(|p| !p.is_empty())
    }

    // ── 渲染 ────────────────────────────────────────────────────────────

    /// 段标题：与设置页同款的 Menlo 小字
    fn sect_label(text: &'static str) -> gpui::Div {
        meta()
            .pb(px(6.))
            .child(text)
    }

    fn empty_hint(text: &'static str) -> gpui::Div {
        div()
            .text_size(px(11.5))
            .text_color(c(theme::FAINT))
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
            .border_color(ca(theme::EDGE, 0.7))
            .child(Self::sect_label(label))
            .child(body)
    }

    /// 带计数的段（`子代理 3`）：v1.17 的四段都有条数，数字比「有没有内容」更快说明问题
    fn section_n(label: &'static str, n: usize, body: impl IntoElement) -> gpui::Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .px(px(14.))
            .py(px(12.))
            .border_b_1()
            .border_color(ca(theme::EDGE, 0.7))
            .child(
                meta()
                    .pb(px(6.))
                    .child(SharedString::from(if n == 0 { label.to_string() } else { format!("{label} {n}") })),
            )
            .child(body)
    }

    /// v1.17 的四段（子代理 / 后台任务 / 已上传 / 技能）长得都是「一行标题 + 一行小字」，
    /// 排版只写一次。`lead` 是行首那一小块（子代理的状态记号），没有就传 None；
    /// `preview` 有值的行点一下原地展开正文（v1.30，仅预览，没有任何干预的口子）。
    fn detail_rows(
        &self,
        rows: Vec<DetailRow>,
        empty: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if rows.is_empty() {
            return Self::empty_hint(empty);
        }
        let mut col = div().flex().flex_col().gap(px(4.));
        let mut ix = 0usize;
        for (lead, title, sub, trailing, preview) in rows {
            let mut head = div().flex().items_center().gap(px(6.));
            if let Some((mark, color)) = lead {
                head = head.child(
                    div()
                        .flex_none()
                        .w(px(10.))
                        .font_family("Menlo")
                        .text_size(px(10.))
                        .text_color(c(color))
                        .child(mark),
                );
            }
            head = head
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(c(theme::INK))
                        .child(SharedString::from(title)),
                )
                .child(
                    meta().flex_none()
                        .child(SharedString::from(trailing)),
                );
            // 有正文的行（子代理 / 后台任务）点一下原地展开，**只读**：AAA 不提供
            // 插手子代理和后台任务的口子（2026-09-10 用户拍板：仅预览）
            let open = preview.as_ref().is_some_and(|(k, _)| self.detail_open.contains(k));
            let key = preview.as_ref().map(|(k, _)| k.clone());
            let body = preview.filter(|_| open).map(|(_, b)| b);
            ix += 1;
            let mut row = div()
                .id(SharedString::from(format!("dt-row:{ix}")))
                .flex()
                .flex_col()
                .gap(px(1.))
                .py(px(3.));
            if let Some(k) = key {
                row = row
                    .cursor_pointer()
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.detail_open.remove(&k) {
                            this.detail_open.insert(k.clone());
                        }
                        cx.notify();
                    }));
            }
            col = col.child(
                row
                    .child(head)
                    .when(!sub.is_empty(), |el| {
                        el.child(
                            div()
                                .line_clamp(2)
                                .text_size(px(11.))
                                .text_color(c(theme::DIM))
                                .child(SharedString::from(sub)),
                        )
                    })
                    .when_some(body, |el, b| {
                        el.child(
                            div()
                                .mt(px(4.))
                                .p(px(8.))
                                .rounded(px(6.))
                                .bg(c(theme::INSET))
                                .font_family("Menlo")
                                .text_size(px(10.5))
                                .text_color(c(theme::DIM))
                                .child(SharedString::from(b)),
                        )
                    }),
            );
        }
        col
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
        let empty_extras = SessionDetailResponse::default();
        let ex = d.map(|d| &d.extras).unwrap_or(&empty_extras);
        let now = chrono::Local::now();

        let header = div()
            .flex_none()
            .flex()
            .items_center()
            .h(px(30.))
            .px(px(14.))
            .border_b_1()
            .border_color(c(theme::EDGE))
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(c(theme::INK))
                    .child("详情"),
            )
            .child(
                meta()
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
                    .text_color(c(theme::FAINT))
                    .hover(|st| st.text_color(c(theme::INK)).bg(c(theme::EDGE_LIGHT)))
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
            // v1.17：消息流里翻不出来的四样（子代理 / 后台任务 / 已上传 / 技能）
            .child(Self::section_n("子代理", ex.subagents.len(), self.render_subagents(ex, &now, cx)))
            .child(Self::section_n("后台任务", ex.background_tasks.len(), self.render_background(ex, &now, cx)))
            .child(Self::section_n("已上传", ex.uploads.len(), self.render_uploads(ex, &now, cx)))
            .child(Self::section_n("产物", d.map(|d| d.artifacts.len()).unwrap_or(0), self.render_artifacts_section(d, &now, cx)))
            .child(Self::section_n("已使用技能", ex.skills.len(), self.render_skills(ex, &now, cx)))
            ;

        Some(
            div()
                .w(px(DETAIL_W))
                .flex_none()
                .h_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .bg(c(theme::SURFACE))
                .border_l_1()
                .border_color(c(theme::EDGE))
                .child(header)
                .child(body),
        )
    }

    /// 整个对话的进度清单（daemon 在每次 Stop 后让 haiku 重写）：☑ 已做、☐ 未做。
    /// v1.22 起直接用会话上的 `checklist`——那串 markdown 由 daemon 解析一次，看板卡片
    /// 的 `items` 同源；此前客户端自己解析 `summary`，与看板可能数出不一样的条数。
    fn render_checklist_section(s: &Session) -> gpui::Div {
        let items = s.checklist.clone();
        if items.is_empty() {
            return Self::empty_hint("每轮回复结束后这里会更新一份「做了什么 / 还没做什么」");
        }
        let (done, total) = (items.iter().filter(|i| i.done).count(), items.len());
        let mut col = div().flex().flex_col().gap(px(5.));
        col = col.child(
            div()
                .font_family("Menlo")
                .text_size(px(10.5))
                .text_color(c(theme::FAINT))
                .pb(px(2.))
                .child(SharedString::from(format!("{done}/{total} 完成"))),
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
                            .text_color(c(if it.done { theme::GREEN } else { theme::FAINT }))
                            .child(if it.done { "☑" } else { "☐" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_size(px(12.))
                            .text_color(c(if it.done { theme::DIM } else { theme::INK }))
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
                .text_color(c(theme::INK))
                .child(SharedString::from(model)),
        );
        if let Some(effort) = u.effort.as_deref().filter(|e| !e.is_empty()) {
            model_row = model_row.child(
                div()
                    .flex_none()
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .bg(ca(theme::ACCENT, 0.14))
                    .font_family("Menlo")
                    .text_size(px(9.5))
                    .text_color(c(theme::ACCENT))
                    .child(SharedString::from(effort.to_string())),
            );
        }
        col = col.child(model_row);

        // 上下文条
        if let Some(pct) = u.context_pct {
            let pct = pct.clamp(0.0, 100.0);
            let color = level_color(pct_level(pct), theme::ACCENT);
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
                                    .text_color(c(theme::DIM))
                                    .child("上下文"),
                            )
                            .child(mono(format!("{}%", pct.round() as i64), color)),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(c(theme::EDGE))
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
                    .child(div().text_size(px(11.)).text_color(c(theme::DIM)).child("缓存"))
                    .child(mono(
                        format!(
                            "{} · 命中 {:.1}% · 读 {} 写 {} 新 {}",
                            if alive { "在" } else { "无" },
                            hit,
                            k(u.cache_read_tokens),
                            k(u.cache_creation_tokens),
                            k(u.fresh_input_tokens)
                        ),
                        if alive { theme::GREEN } else { theme::FAINT },
                    )),
            );
        }
        // 费用 · 行数 · 时长
        let mut stats = div().flex().flex_wrap().items_center().gap(px(10.));
        if let Some(cost) = u.cost_usd {
            stats = stats.child(mono(fmt_cost(cost), theme::INK));
        }
        if u.lines_added.is_some() || u.lines_removed.is_some() {
            stats = stats.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(mono(format!("+{}", u.lines_added.unwrap_or(0)), theme::GREEN))
                    .child(mono(format!("−{}", u.lines_removed.unwrap_or(0)), theme::RED))
                    .child(div().text_size(px(11.)).text_color(c(theme::DIM)).child("行")),
            );
        }
        if let Some(ms) = u.duration_ms {
            stats = stats.child(mono(humanize_ms(ms), theme::DIM));
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
                    .hover(|st| st.bg(c(theme::SURFACE_RAISED)))
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
                                    .text_color(c(theme::INK))
                                    .child(SharedString::from(title)),
                            )
                            .child(
                                meta().flex_none()
                                    .child(SharedString::from(time)),
                            ),
                    )
                    .when(!a.description.is_empty(), |el| {
                        el.child(
                            div()
                                .line_clamp(2)
                                .text_size(px(11.))
                                .text_color(c(theme::DIM))
                                .child(SharedString::from(a.description.clone())),
                        )
                    }),
            );
        }
        col
    }

    /// 子代理：状态记号（跑着 ⋯ / 成了 ✓ / 挂了 ✗）+ 类型 + 它去干什么
    fn render_subagents(&self, ex: &SessionDetailResponse, now: &DateTime<chrono::Local>, cx: &mut Context<Self>) -> gpui::Div {
        let rows = ex
            .subagents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let lead = match a.status.as_str() {
                    "running" => ("⋯", theme::ACCENT),
                    "err" => ("✗", theme::RED),
                    _ => ("✓", theme::GREEN),
                };
                let title = if a.kind.is_empty() { a.tool.clone() } else { a.kind.clone() };
                // 点开看：派给它的任务书 + 它交回来的报告（还在跑就只有任务书）
                let mut body = a.prompt.clone();
                if !a.result.is_empty() {
                    if !body.is_empty() {
                        body.push_str("\n\n── 它交回来的 ──\n");
                    }
                    body.push_str(&a.result);
                }
                let preview = (!body.is_empty()).then(|| (format!("sub:{i}"), body));
                (Some(lead), title, a.summary.clone(), fmt_artifact_time(&a.ts, now, &chrono::Local).unwrap_or_default(), preview)
            })
            .collect();
        self.detail_rows(rows, "这个会话还没开过子代理", cx)
    }

    /// 后台任务：还没等到 `<task-notification>` 的那些（会话行上「后台」两个字的来源）
    fn render_background(&self, ex: &SessionDetailResponse, now: &DateTime<chrono::Local>, cx: &mut Context<Self>) -> gpui::Div {
        let rows = ex
            .background_tasks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let preview = (!t.detail.is_empty()).then(|| (format!("bg:{i}"), t.detail.clone()));
                (
                    Some(("⋯", theme::ACCENT)),
                    t.tool.clone(),
                    t.summary.clone(),
                    fmt_artifact_time(&t.ts, now, &chrono::Local).unwrap_or_default(),
                    preview,
                )
            })
            .collect();
        self.detail_rows(rows, "没有挂着的后台任务", cx)
    }

    /// 已上传：项目 `_inbox/` 里的文件（📎 和手机的系统分享都落这儿）
    fn render_uploads(&self, ex: &SessionDetailResponse, now: &DateTime<chrono::Local>, cx: &mut Context<Self>) -> gpui::Div {
        let rows = ex
            .uploads
            .iter()
            .map(|u| {
                (
                    None,
                    u.name.clone(),
                    human_bytes(u.size),
                    fmt_artifact_time(&u.ts, now, &chrono::Local).unwrap_or_default(),
                    None,
                )
            })
            .collect();
        self.detail_rows(rows, "还没有传过文件进这个项目", cx)
    }

    /// 已使用技能：Skill 工具调用，按名字合并计数
    fn render_skills(&self, ex: &SessionDetailResponse, now: &DateTime<chrono::Local>, cx: &mut Context<Self>) -> gpui::Div {
        let rows = ex
            .skills
            .iter()
            .map(|u| {
                (
                    None,
                    u.name.clone(),
                    if u.count > 1 { format!("{} 次", u.count) } else { String::new() },
                    fmt_artifact_time(&u.last_ts, now, &chrono::Local).unwrap_or_default(),
                    None,
                )
            })
            .collect();
        self.detail_rows(rows, "这个会话还没用过技能", cx)
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
                line1 = line1.child(div().px(px(4.)).text_color(c(theme::FAINT)).child("·"));
            }
            let color = level_color(pct_level(*pct), theme::DIM);
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
            .border_color(c(theme::EDGE))
            .child(line1);
        if !resets.is_empty() {
            block = block.child(
                meta()
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
        assert_eq!(level_color(Level::Warn, 0x123456), theme::AMBER);
        assert_eq!(level_color(Level::Crit, 0x123456), theme::RED);
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
