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
