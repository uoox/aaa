//! Session log: one record per session that ever existed on this daemon —
//! alive, exited, or deleted — so the clients can show「历史」even after the
//! session record itself is gone. Stored in `<state_dir>/history.json`.
//!
//! Kept in sync from the pool on the 1s tick (title / state / summary
//! change → record updated); deletion is stamped explicitly by the delete
//! handlers. Capped at [`KEEP`] newest by `created_at`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

pub const KEEP: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub project_path: String,
    pub project_name: String,
    pub agent: String,
    pub title: String,
    pub created_at: String,
    /// 进程退出的时刻（exited）；还活着为 null
    pub ended_at: Option<String>,
    pub exit_code: Option<i64>,
    /// 记录被删除（DELETE /sessions/:id、删项目、daemon 重启清理）的时刻
    pub deleted_at: Option<String>,
    /// 最后一版进度清单（summary.rs），删除后也留着
    #[serde(default)]
    pub summary: String,
    /// 最后一次已知状态：running | waiting | exited
    #[serde(default)]
    pub last_state: String,
}

pub struct History {
    file: PathBuf,
    map: HashMap<String, Entry>,
    dirty: bool,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl History {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("history.json");
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<Entry>>(&s).ok())
            .map(|v| v.into_iter().map(|e| (e.id.clone(), e)).collect())
            .unwrap_or_default();
        History { file, map, dirty: false }
    }

    /// Newest first by `created_at`.
    pub fn list(&self, limit: usize) -> Vec<Entry> {
        let mut v: Vec<Entry> = self.map.values().cloned().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.cmp(&a.id)));
        v.truncate(limit);
        v
    }

    /// Insert or refresh a record from the live session's current shape.
    /// Returns whether anything changed.
    pub fn upsert(&mut self, fresh: Entry) -> bool {
        match self.map.get_mut(&fresh.id) {
            Some(cur) => {
                // deletion stamp is ours, never overwritten by a live sync
                let deleted_at = cur.deleted_at.clone();
                let mut next = fresh;
                next.deleted_at = deleted_at;
                if *cur == next {
                    false
                } else {
                    *cur = next;
                    self.dirty = true;
                    true
                }
            }
            None => {
                self.map.insert(fresh.id.clone(), fresh);
                self.dirty = true;
                true
            }
        }
    }

    pub fn mark_deleted(&mut self, id: &str) {
        if let Some(e) = self.map.get_mut(id) {
            if e.deleted_at.is_none() {
                e.deleted_at = Some(now_iso());
                self.dirty = true;
            }
        }
    }

    /// Every record under a project path (删项目).
    pub fn mark_project_deleted(&mut self, project_path: &str) {
        let ids: Vec<String> = self
            .map
            .values()
            .filter(|e| e.project_path == project_path && e.deleted_at.is_none())
            .map(|e| e.id.clone())
            .collect();
        for id in ids {
            self.mark_deleted(&id);
        }
    }

    /// Write out if anything changed since the last save; trims to KEEP.
    pub fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        if self.map.len() > KEEP {
            let keep: Vec<Entry> = self.list(KEEP);
            self.map = keep.into_iter().map(|e| (e.id.clone(), e)).collect();
        }
        if let Some(dir) = self.file.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let v = self.list(usize::MAX);
        if let Ok(body) = serde_json::to_string(&v) {
            let _ = crate::paths::write_atomic(&self.file, body.as_bytes());
        }
        self.dirty = false;
    }
}

/// Snapshot a live session into a record.
pub fn entry_from(sess: &crate::pool::Session) -> Entry {
    let m = sess.meta.lock().unwrap();
    let iso = |t: &chrono::DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Secs, true);
    let exited = m.state == crate::pool::State::Exited;
    Entry {
        id: sess.id.clone(),
        project_path: m.project_path.clone(),
        project_name: m.project_name.clone(),
        agent: m.agent.clone(),
        title: m.title.clone(),
        created_at: iso(&m.created_at),
        // 退出时刻 = 最后一次「有意义变化」（退出本身会 touch）
        ended_at: if exited { Some(iso(&m.updated_at.unwrap_or(m.last_output_at))) } else { None },
        exit_code: m.exit_code,
        deleted_at: None,
        summary: m.summary.clone(),
        last_state: match m.state {
            crate::pool::State::Running => "running",
            crate::pool::State::Waiting => "waiting",
            crate::pool::State::Exited => "exited",
        }
        .into(),
    }
}

/// 1s tick: mirror every pooled session into the log, save when changed.
pub fn sync(app: &crate::api::SharedApp) {
    let mut h = app.history.lock().unwrap();
    for sess in app.pool.all() {
        h.upsert(entry_from(&sess));
    }
    h.save_if_dirty();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, created: &str) -> Entry {
        Entry {
            id: id.into(),
            project_path: "/p/a".into(),
            project_name: "a".into(),
            agent: "claude".into(),
            title: "t".into(),
            created_at: created.into(),
            ended_at: None,
            exit_code: None,
            deleted_at: None,
            summary: String::new(),
            last_state: "waiting".into(),
        }
    }

    #[test]
    fn upsert_delete_persist_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::load(dir.path());
        assert!(h.upsert(e("s1", "2026-09-06T10:00:00Z")));
        assert!(!h.upsert(e("s1", "2026-09-06T10:00:00Z")), "unchanged: no write");
        assert!(h.upsert(e("s2", "2026-09-06T11:00:00Z")));
        h.mark_deleted("s1");
        // a later live sync must not clear the deletion stamp
        let mut again = e("s1", "2026-09-06T10:00:00Z");
        again.title = "renamed".into();
        h.upsert(again);
        h.save_if_dirty();
        let back = History::load(dir.path());
        let list = back.list(10);
        assert_eq!(list.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["s2", "s1"], "newest first");
        assert!(list[1].deleted_at.is_some());
        assert_eq!(list[1].title, "renamed");
        assert!(back.list(1).len() == 1);
    }

    #[test]
    fn project_delete_marks_all_and_cap_holds() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = History::load(dir.path());
        for i in 0..(KEEP + 20) {
            h.upsert(e(&format!("s{i}"), &format!("2026-09-06T10:{:02}:{:02}Z", i / 60, i % 60)));
        }
        h.mark_project_deleted("/p/a");
        h.save_if_dirty();
        let back = History::load(dir.path());
        let list = back.list(usize::MAX);
        assert_eq!(list.len(), KEEP);
        assert!(list.iter().all(|x| x.deleted_at.is_some()));
        assert_eq!(list[0].id, format!("s{}", KEEP + 19), "the oldest were trimmed");
    }
}

// ── 日历：按天的 haiku 摘要 ─────────────────────────────────────────────────
//
// 每天一段「这一天做了什么」，输入是当天开始的会话（标题 + 进度清单），haiku 写成
// 要点。输入变了（新会话、清单更新）就重写，没变不碰；一次最多写两天，控制成本。
// 存 `<state_dir>/history_days.json`。

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DayDigest {
    /// 本地日期 YYYY-MM-DD
    pub date: String,
    pub text: String,
    pub sessions: usize,
    #[serde(default)]
    pub input_hash: u64,
    #[serde(default)]
    pub generated_at: String,
}

/// ISO 时间 → daemon 所在时区的日期
pub fn local_day(iso: &str) -> Option<String> {
    let t = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some(t.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
}

/// 按天分组（按会话开始时间），只收项目会话（终端不算）
pub fn group_by_day(entries: &[Entry]) -> BTreeMap<String, Vec<&Entry>> {
    let mut m: BTreeMap<String, Vec<&Entry>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.agent != "shell") {
        if let Some(d) = local_day(&e.created_at) {
            m.entry(d).or_default().push(e);
        }
    }
    m
}

pub fn day_hash(entries: &[&Entry]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in entries {
        e.id.hash(&mut h);
        e.title.hash(&mut h);
        e.summary.hash(&mut h);
    }
    h.finish()
}

const DAY_PROMPT: &str = "下面是某一天里 Claude Code 各个会话的标题和进度清单（[x] 已做 / [ ] 未做）。\
请用要点写出这一天做了什么：按项目归并，每条一件事、不超过 30 个字，最多 10 条；做完的直接陈述，没做完的末尾标「（未完）」。\
使用对话的主要语言。只输出要点，每行一条，以「- 」开头，不要标题、不要解释。\n\n";

pub fn day_prompt(date: &str, entries: &[&Entry]) -> String {
    let mut out = format!("{DAY_PROMPT}日期：{date}\n");
    for e in entries {
        out.push_str(&format!("\n## {} · {}\n", e.project_name, if e.title.is_empty() { &e.project_name } else { &e.title }));
        if !e.summary.is_empty() {
            out.push_str(&e.summary);
            out.push('\n');
        }
    }
    out
}

/// 模型输出 → 规整的要点（只认以 - * • 开头的行；最多 12 行），None = 没有可用内容
pub fn clean_day(out: &str) -> Option<String> {
    let lines: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with(['-', '*', '•']))
        .map(|l| l.trim_start_matches(['-', '*', '•']).trim())
        .filter(|l| !l.is_empty())
        .take(12)
        .map(|l| format!("- {l}"))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub struct Days {
    file: PathBuf,
    map: BTreeMap<String, DayDigest>,
}

impl Days {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("history_days.json");
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<DayDigest>>(&s).ok())
            .map(|v| v.into_iter().map(|d| (d.date.clone(), d)).collect())
            .unwrap_or_default();
        Days { file, map }
    }

    fn save(&self) {
        if let Some(dir) = self.file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let v: Vec<&DayDigest> = self.map.values().collect();
        if let Ok(body) = serde_json::to_string(&v) {
            let _ = crate::paths::write_atomic(&self.file, body.as_bytes());
        }
    }

    pub fn get(&self, date: &str) -> Option<&DayDigest> {
        self.map.get(date)
    }

    pub fn put(&mut self, d: DayDigest) {
        self.map.insert(d.date.clone(), d);
        self.save();
    }

    /// 需要（重）写的日期：没有摘要、或输入变了。最新的日期在前
    pub fn stale<'a>(&self, groups: &'a BTreeMap<String, Vec<&'a Entry>>) -> Vec<&'a str> {
        let mut v: Vec<&str> = groups
            .iter()
            .filter(|(date, es)| self.map.get(*date).map(|d| d.input_hash != day_hash(es)).unwrap_or(true))
            .map(|(d, _)| d.as_str())
            .collect();
        v.reverse();
        v
    }
}

/// 日历数据：每天的会话数 + 摘要（没写出来的 text 为空）
pub fn calendar(entries: &[Entry], days: &Days) -> Vec<DayDigest> {
    group_by_day(entries)
        .iter()
        .rev()
        .map(|(date, es)| DayDigest {
            date: date.clone(),
            text: days.get(date).map(|d| d.text.clone()).unwrap_or_default(),
            sessions: es.len(),
            input_hash: 0,
            generated_at: days.get(date).map(|d| d.generated_at.clone()).unwrap_or_default(),
        })
        .collect()
}

/// 5 分钟一次：给输入变了的日子重写摘要（一次最多 `max` 天）。阻塞（跑 haiku）
pub fn refresh_days(app: &crate::api::SharedApp, max: usize) {
    if !app.cfg.namer {
        return;
    }
    let entries = app.history.lock().unwrap().list(KEEP);
    let groups = group_by_day(&entries);
    let stale: Vec<String> = app.days.lock().unwrap().stale(&groups).into_iter().map(str::to_owned).collect();
    let Some(exe) = crate::agents::which("claude", &app.paths.home) else { return };
    for date in stale.into_iter().take(max) {
        let Some(es) = groups.get(&date) else { continue };
        let hash = day_hash(es);
        let prompt = day_prompt(&date, es);
        let Some(text) = crate::namer::run_haiku(&exe, &prompt).as_deref().and_then(clean_day) else { continue };
        app.days.lock().unwrap().put(DayDigest {
            date: date.clone(),
            text,
            sessions: es.len(),
            input_hash: hash,
            generated_at: now_iso(),
        });
    }
}

#[cfg(test)]
mod day_tests {
    use super::*;

    fn e(id: &str, created: &str, title: &str, summary: &str) -> Entry {
        Entry {
            id: id.into(),
            project_path: "/p/a".into(),
            project_name: "a".into(),
            agent: "claude".into(),
            title: title.into(),
            created_at: created.into(),
            ended_at: None,
            exit_code: None,
            deleted_at: None,
            summary: summary.into(),
            last_state: "waiting".into(),
        }
    }

    #[test]
    fn grouping_hash_and_staleness() {
        let dir = tempfile::tempdir().unwrap();
        let mut days = Days::load(dir.path());
        let a = e("a", "2026-09-06T02:00:00Z", "修登录", "- [x] 修登录");
        let mut shell = e("t", "2026-09-06T03:00:00Z", "zsh", "");
        shell.agent = "shell".into();
        let entries = vec![a.clone(), shell];
        let groups = group_by_day(&entries);
        assert_eq!(groups.len(), 1, "终端不进日历");
        let date = groups.keys().next().unwrap().clone();
        assert_eq!(days.stale(&groups), vec![date.as_str()], "没摘要 = 要写");
        let hash = day_hash(&groups[&date]);
        days.put(DayDigest { date: date.clone(), text: "- 修好登录".into(), sessions: 1, input_hash: hash, generated_at: "t".into() });
        assert!(days.stale(&groups).is_empty(), "输入没变不重写");
        // 清单更新 → 输入变了 → 重写
        let mut a2 = a.clone();
        a2.summary = "- [x] 修登录\n- [ ] 补测试".into();
        let entries2 = vec![a2];
        assert_eq!(days.stale(&group_by_day(&entries2)).len(), 1);
        // 日历数据带会话数与摘要；落盘可读回
        let cal = calendar(&entries, &Days::load(dir.path()));
        assert_eq!(cal[0].sessions, 1);
        assert_eq!(cal[0].text, "- 修好登录");
        assert_eq!(clean_day("要点：\n- 修好登录\n* 补测试（未完）\n\n"), Some("- 修好登录\n- 补测试（未完）".into()));
        assert!(clean_day("  \n").is_none());
        assert!(day_prompt(&date, &groups[&date]).contains("## a · 修登录"));
    }
}

// ── 仪表盘 ────────────────────────────────────────────────────────────────
//
// 「一眼看出最近做了什么，还有什么没做」（2026-09-07 用户拍板：两栏，左待办右流水）。
// 左栏 = 所有未删除会话里没勾的清单项，聚合成一张待办表；右栏 = 按天倒序的流水，
// 每天一段 haiku 日摘要 + 当天的会话。数字块（今天 / 近 7 天）在顶上。
// 聚合只在 daemon 做一次，两端只负责画。

/// 进度清单的一项
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecklistItem {
    pub done: bool,
    pub text: String,
}

/// `- [x] …` / `- [ ] …` 行 → 项；其它行忽略（与两端客户端同口径）
pub fn parse_checklist(md: &str) -> Vec<ChecklistItem> {
    md.lines()
        .filter_map(|raw| {
            let l = raw.trim().trim_start_matches(['-', '*']).trim_start();
            let (done, rest) = if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
                (true, r)
            } else if let Some(r) = l.strip_prefix("[ ]") {
                (false, r)
            } else {
                return None;
            };
            let text = rest.trim();
            (!text.is_empty()).then(|| ChecklistItem { done, text: text.to_string() })
        })
        .collect()
}

/// 左栏一条待办：某个会话清单里没勾的一项
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenItem {
    pub session_id: String,
    pub project_name: String,
    pub project_path: String,
    /// 会话标题（点进去看的是这个会话）
    pub title: String,
    /// 没勾的那一项本身
    pub text: String,
    pub created_at: String,
    /// 会话还在池子里（能点开，已退出的也算）
    pub alive: bool,
    /// 会话进程还没退出（running / waiting）——「在跑」的徽标按它画
    #[serde(default)]
    pub running: bool,
}

/// 右栏一天里的一个会话
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DayEntry {
    pub id: String,
    pub title: String,
    pub project_name: String,
    /// 还在池子里（能点开）
    pub alive: bool,
    /// 进程还没退出
    #[serde(default)]
    pub running: bool,
    pub deleted: bool,
    pub done: usize,
    pub open: usize,
}

/// 右栏的一天
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DayCard {
    pub date: String,
    /// haiku 写的「这一天做了什么」；还没写出来为空
    pub text: String,
    pub sessions: usize,
    pub done: usize,
    pub open: usize,
    pub entries: Vec<DayEntry>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stats {
    pub sessions: usize,
    pub done: usize,
    pub open: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dashboard {
    /// daemon 本机时区的今天（YYYY-MM-DD）。客户端画「（今天）」用它，别用自己的
    /// `LocalDate.now()`——手机和 Mac 不在一个时区时会标错天。
    pub date: String,
    pub today: Stats,
    pub week: Stats,
    /// 此刻进程还没退出的会话数（running / waiting）
    pub active: usize,
    pub open: Vec<OpenItem>,
    pub days: Vec<DayCard>,
    /// 近 8 周的每日会话数（热力条），最旧在前，长度 = 56
    pub spark: Vec<usize>,
}

/// 待办上限：够看一屏一屏，不至于把响应撑爆
pub const OPEN_CAP: usize = 200;
/// 流水天数上限
pub const DAYS_CAP: usize = 30;
const SPARK_DAYS: usize = 56;

fn stats_of(entries: &[&Entry]) -> Stats {
    let mut s = Stats { sessions: entries.len(), done: 0, open: 0 };
    for e in entries {
        for it in parse_checklist(&e.summary) {
            if it.done {
                s.done += 1;
            } else {
                s.open += 1;
            }
        }
    }
    s
}

/// `today` = daemon 本机时区的今天；`alive` = 池子里还在的会话 id（含已退出的，能点开）；
/// `running` = 其中进程还没退出的（「在跑」与 `active` 按它算——池子里绝大多数是已退出的，
/// 2026-09-07 首版把两者混成一个，看板上写出「此刻 135 在跑」）
pub fn dashboard(
    entries: &[Entry],
    days: &Days,
    alive: &std::collections::HashSet<String>,
    running: &std::collections::HashSet<String>,
    today: &str,
) -> Dashboard {
    let groups = group_by_day(entries);
    let week: Vec<String> = last_days(today, 7);
    let today_rows: Vec<&Entry> = groups.get(today).cloned().unwrap_or_default();
    let week_rows: Vec<&Entry> = week.iter().filter_map(|d| groups.get(d)).flatten().copied().collect();

    // 左栏：未删除会话里没勾的项，新会话在前
    let mut open = Vec::new();
    for e in entries.iter().filter(|e| e.deleted_at.is_none() && e.agent != "shell") {
        for it in parse_checklist(&e.summary).into_iter().filter(|i| !i.done) {
            open.push(OpenItem {
                session_id: e.id.clone(),
                project_name: e.project_name.clone(),
                project_path: e.project_path.clone(),
                title: if e.title.is_empty() { e.project_name.clone() } else { e.title.clone() },
                text: it.text,
                created_at: e.created_at.clone(),
                alive: alive.contains(&e.id),
                running: running.contains(&e.id),
            });
            if open.len() >= OPEN_CAP {
                break;
            }
        }
        if open.len() >= OPEN_CAP {
            break;
        }
    }

    // 右栏：按天倒序
    let days_out: Vec<DayCard> = groups
        .iter()
        .rev()
        .take(DAYS_CAP)
        .map(|(date, es)| {
            let s = stats_of(es);
            DayCard {
                date: date.clone(),
                text: days.get(date).map(|d| d.text.clone()).unwrap_or_default(),
                sessions: s.sessions,
                done: s.done,
                open: s.open,
                entries: es
                    .iter()
                    .map(|e| {
                        let items = parse_checklist(&e.summary);
                        DayEntry {
                            id: e.id.clone(),
                            title: if e.title.is_empty() { e.project_name.clone() } else { e.title.clone() },
                            project_name: e.project_name.clone(),
                            alive: alive.contains(&e.id),
                            running: running.contains(&e.id),
                            deleted: e.deleted_at.is_some(),
                            done: items.iter().filter(|i| i.done).count(),
                            open: items.iter().filter(|i| !i.done).count(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();

    let spark = last_days(today, SPARK_DAYS)
        .into_iter()
        .rev()
        .map(|d| groups.get(&d).map(|v| v.len()).unwrap_or(0))
        .collect();

    Dashboard {
        date: today.to_string(),
        today: stats_of(&today_rows),
        week: stats_of(&week_rows),
        active: entries.iter().filter(|e| running.contains(&e.id)).count(),
        open,
        days: days_out,
        spark,
    }
}

/// 从 `today` 往回数 n 天的日期（最新在前）；`today` 解析不了就返回空
fn last_days(today: &str, n: usize) -> Vec<String> {
    let Ok(d0) = chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") else { return Vec::new() };
    (0..n).filter_map(|i| d0.checked_sub_signed(chrono::Duration::days(i as i64))).map(|d| d.format("%Y-%m-%d").to_string()).collect()
}

/// 本机时区的今天
pub fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod dashboard_tests {
    use super::*;

    fn e(id: &str, created: &str, title: &str, summary: &str) -> Entry {
        Entry {
            id: id.into(),
            project_path: "/p/a".into(),
            project_name: "a".into(),
            agent: "claude".into(),
            title: title.into(),
            created_at: created.into(),
            ended_at: None,
            exit_code: None,
            deleted_at: None,
            summary: summary.into(),
            last_state: "waiting".into(),
        }
    }

    #[test]
    fn checklist_parsing() {
        let items = parse_checklist("- [x] 修登录\n* [ ] 补测试\n随便一句\n- [ ]   \n-[X] 发版");
        assert_eq!(items.len(), 3);
        assert!(items[0].done && items[0].text == "修登录");
        assert!(!items[1].done && items[1].text == "补测试");
        assert!(items[2].done, "-[X] 也认");
    }

    #[test]
    fn dashboard_counts_open_days_and_spark() {
        let dir = tempfile::tempdir().unwrap();
        let mut days = Days::load(dir.path());
        // 用本地时区构造，避免测试机时区把日期挪走
        let iso = |day: i64, h: u32| {
            let t = chrono::Local::now() - chrono::Duration::days(day);
            t.date_naive()
                .and_hms_opt(h, 0, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .unwrap()
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        };
        let today = today_local();
        let a = e("a", &iso(0, 9), "修登录", "- [x] 修登录\n- [ ] 补测试");
        let b = e("b", &iso(2, 9), "发版", "- [x] 打包\n- [x] 上传");
        let mut gone = e("c", &iso(0, 10), "废弃", "- [ ] 不该出现");
        gone.deleted_at = Some("t".into());
        let mut shell = e("t1", &iso(0, 11), "zsh", "- [ ] 终端不算");
        shell.agent = "shell".into();
        let entries = vec![a, gone, shell, b];
        days.put(DayDigest { date: today.clone(), text: "- 修好登录".into(), sessions: 2, input_hash: 0, generated_at: "t".into() });
        // a 在池子里且还在跑；b 只是没被清理掉的已退出记录
        let alive: std::collections::HashSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        let running: std::collections::HashSet<String> = ["a".to_string()].into_iter().collect();
        let d = dashboard(&entries, &days, &alive, &running, &today);

        assert_eq!(d.date, today, "客户端按 daemon 的今天画「（今天）」，不按自己的时区");
        assert_eq!(d.today.sessions, 2, "今天：a + 已删除的 c（终端不进日历）");
        assert_eq!((d.today.done, d.today.open), (1, 2));
        assert_eq!(d.week.sessions, 3, "近 7 天含前天的 b");
        assert_eq!(d.active, 1, "已退出但还在池子里的 b 不算「在跑」");
        assert_eq!(d.open.len(), 1, "已删除与终端的未完项不进待办");
        assert_eq!(d.open[0].text, "补测试");
        assert!(d.open[0].alive && d.open[0].running);
        assert_eq!(d.days[0].date, today, "最新的一天在前");
        assert_eq!(d.days[0].text, "- 修好登录");
        assert_eq!((d.days[0].done, d.days[0].open), (1, 2));
        assert_eq!(d.days[0].entries.len(), 2);
        assert!(d.days[0].entries.iter().any(|x| x.deleted && x.id == "c"));
        assert_eq!(d.spark.len(), 56);
        assert_eq!(d.spark[55], 2, "热力条最后一格是今天");
        assert_eq!(d.spark[53], 1, "前天一条");
    }
}
