//! Session log: one record per session that ever existed on this daemon —
//! alive, exited, or deleted — so the clients can show「历史」even after the
//! session record itself is gone. Stored in `<state_dir>/history.json`.
//!
//! Kept in sync from the pool on the 1s tick (title / state / summary
//! change → record updated); deletion is stamped explicitly by the delete
//! handlers. Capped at [`KEEP`] newest by `created_at`.

use std::collections::HashMap;
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

// ── 看板 ────────────────────────────────────────────────────────────────────
//
// 2026-09-07 用户拍板（第二版）：看板 = **所有会话的进度**，没有时间维度。一会话一张
// 卡：状态 + 标题 + 项目 + 进度（清单 done/total）+ 没勾的项；按 待回复 > 运行 >
// 后台 > 激活 > 暂停 排，同状态里最近更新在前。第一版的日流水 / haiku 日摘要 / 热力条
// 全部移除——进度本身就带着时间，「哪天做了什么」没人看。
// 聚合只在 daemon 做一次，两端只画。

/// 进度清单的一项
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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

/// 会话此刻的状态字（与侧栏同一套五态，PROTOCOL「GUI 列表口径」）
pub const STATUS_ORDER: [&str; 5] = ["asking", "running", "background", "active", "paused"];

/// 池子里一条会话此刻是五态里的哪一个。**只此一处**（v1.22）：看板、项目列表、
/// 两端的侧栏说的都是这一句——此前 daemon 一份、mac `RowStatus::of` 一份、
/// Android `projectStateOf` 一份，同一台机器同一时刻能给出不同的答案。
/// 退出的进程一律 `paused`（哪怕还在池子里点得开）。
pub fn status_of(m: &crate::pool::Meta) -> &'static str {
    match m.state {
        crate::pool::State::Exited => "paused",
        _ if m.asking => "asking",
        crate::pool::State::Running => "running",
        crate::pool::State::Waiting if m.background > 0 => "background",
        crate::pool::State::Waiting => "active",
    }
}

pub fn status_rank(status: &str) -> usize {
    STATUS_ORDER.iter().position(|s| *s == status).unwrap_or(STATUS_ORDER.len())
}

/// 池子里一条会话，聚项目行时要看的那几样（v1.22）
#[derive(Clone, Debug)]
pub struct SessSnap {
    /// 已规范化的项目路径（`stores::realpath`）
    pub path: String,
    pub id: String,
    /// 会话的 agent（claude / codex…）：没登记的目录靠它给项目行填 agent
    pub agent: String,
    pub title: String,
    /// 排序键：状态翻转 / 改名的时刻（老元数据没有 → created_at）
    pub updated_at: String,
    /// 五态之一（[`status_of`]）。`paused` = 这个进程已经退出了
    pub status: &'static str,
}

/// 一个项目在池子里的会话聚成的一条
#[derive(Default, Clone, Debug)]
pub struct ProjAgg {
    /// 代表这个项目的**活**会话：几个同时活着取最近更新的。None = 项目此刻没有活会话
    pub live: Option<SessSnap>,
    /// 最新一条会话（**含已退出**）：项目行的排序时间，也是「点一下 resume 谁」
    pub latest: Option<SessSnap>,
}

/// 会话 → 项目行的聚合（v1.22，`GET /projects` 用）。**这条口径只此一处**：
/// 此前 mac 取 `updated_at` 最大的活会话、Android 先按 agent 过滤再按
/// 「待回复 < 执行中 < 其它」排，同一个项目在两台设备上会显示不同的标题和状态。
/// 传进来的应当已经滤掉终端（shell 不代表项目）。
pub fn project_aggregate(snaps: impl Iterator<Item = SessSnap>) -> std::collections::HashMap<String, ProjAgg> {
    let mut out: std::collections::HashMap<String, ProjAgg> = std::collections::HashMap::new();
    for s in snaps {
        let e = out.entry(s.path.clone()).or_default();
        if e.latest.as_ref().is_none_or(|l| s.updated_at > l.updated_at) {
            e.latest = Some(s.clone());
        }
        // 退出的会话只贡献排序时间：它不代表项目（PROTOCOL「exited 会话不代表项目」）
        if s.status != "paused" && e.live.as_ref().is_none_or(|l| s.updated_at > l.updated_at) {
            e.live = Some(s);
        }
    }
    out
}

/// 池子里一条会话此刻的样子（api 层从 pool 取）
#[derive(Clone, Debug)]
pub struct LiveStatus {
    /// asking | running | background | active | paused（已退出但还在池子里 = paused）
    pub status: &'static str,
    /// updated_at（排序键）
    pub updated_at: String,
}

/// 一张卡
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCard {
    pub id: String,
    pub title: String,
    pub project_name: String,
    pub project_path: String,
    /// asking | running | background | active | paused
    pub status: String,
    /// 还在池子里（能点开，已退出的回放也算）
    pub alive: bool,
    pub deleted: bool,
    pub done: usize,
    pub open: usize,
    pub items: Vec<ChecklistItem>,
    /// 排序键：状态翻转 / 改名 / 退出的时刻
    pub updated_at: String,
}

/// 看板顶上唯一还留着的数字（2026-09-08 用户拍板：五态计数条跟着列表的状态字一起去掉——
/// 「还欠多少件事」是个真的量，「有几个 active」不是）。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Counts {
    /// 未删除会话里没勾的清单项总数
    pub open_items: usize,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Dashboard {
    pub counts: Counts,
    pub sessions: Vec<SessionCard>,
}

/// `live`：池子里的会话 id → 此刻状态；不在池子里的一律 paused（不能点开）。
/// 终端不进看板；已删除的进（`deleted:true`，客户端默认藏起来），但不计数。
pub fn dashboard(
    entries: &[Entry],
    live: &std::collections::HashMap<String, LiveStatus>,
) -> Dashboard {
    let mut cards: Vec<SessionCard> = entries
        .iter()
        .filter(|e| e.agent != "shell")
        .map(|e| {
            let items = parse_checklist(&e.summary);
            let (status, updated_at, alive) = match live.get(&e.id) {
                Some(l) => (l.status.to_string(), l.updated_at.clone(), true),
                None => ("paused".to_string(), e.ended_at.clone().unwrap_or_else(|| e.created_at.clone()), false),
            };
            SessionCard {
                id: e.id.clone(),
                title: if e.title.is_empty() { e.project_name.clone() } else { e.title.clone() },
                project_name: e.project_name.clone(),
                project_path: e.project_path.clone(),
                status,
                alive,
                deleted: e.deleted_at.is_some(),
                done: items.iter().filter(|i| i.done).count(),
                open: items.iter().filter(|i| !i.done).count(),
                items,
                updated_at,
            }
        })
        .collect();
    // 状态 → 已删除的沉到组尾 → 最近更新在前
    cards.sort_by(|a, b| {
        status_rank(&a.status)
            .cmp(&status_rank(&b.status))
            .then_with(|| a.deleted.cmp(&b.deleted))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut counts = Counts::default();
    for c in cards.iter().filter(|c| !c.deleted) {
        counts.open_items += c.open;
    }
    Dashboard { counts, sessions: cards }
}

#[cfg(test)]
mod dashboard_tests {
    use super::*;
    use std::collections::HashMap;

    fn snap(path: &str, id: &str, title: &str, updated: &str, status: &'static str) -> SessSnap {
        SessSnap { path: path.into(), id: id.into(), agent: "claude".into(), title: title.into(), updated_at: updated.into(), status }
    }

    /// 项目行的代表会话：活着的优先、几个活着取最近更新的；全退出了只留排序时间。
    /// 这条口径 v1.22 从两端搬进来——两端此前的推法本来还不一样。
    #[test]
    fn project_aggregate_picks_the_live_session() {
        let m = project_aggregate(
            [
                snap("/p/a", "s_old", "旧", "2026-09-08T10:00:00Z", "paused"),
                snap("/p/a", "s_live", "在跑", "2026-09-08T11:00:00Z", "running"),
                snap("/p/a", "s_new", "更新但退了", "2026-09-08T12:00:00Z", "paused"),
                snap("/p/b", "s_b", "都退了", "2026-09-08T09:00:00Z", "paused"),
            ]
            .into_iter(),
        );
        let a = &m["/p/a"];
        assert_eq!(a.live.as_ref().unwrap().id, "s_live", "退出的不代表项目，哪怕它更新");
        assert_eq!(a.latest.as_ref().unwrap().id, "s_new", "排序时间含已退出的");
        let b = &m["/p/b"];
        assert!(b.live.is_none(), "一个活的都没有 = paused");
        assert_eq!(b.latest.as_ref().unwrap().id, "s_b", "点它 resume");
    }

    #[test]
    fn project_aggregate_prefers_the_newest_live_one() {
        let m = project_aggregate(
            [
                snap("/p/a", "s1", "先跑的", "2026-09-08T10:00:00Z", "asking"),
                snap("/p/a", "s2", "后跑的", "2026-09-08T10:30:00Z", "active"),
            ]
            .into_iter(),
        );
        assert_eq!(m["/p/a"].live.as_ref().unwrap().id, "s2");
    }

    /// 三端共享向量 fixtures/dashboard.json：daemon 的输出必须逐字段等于 `sessions`
    #[test]
    fn shared_fixture_matches_daemon_output() {
        let fx: serde_json::Value = serde_json::from_str(include_str!("../../fixtures/dashboard.json")).unwrap();
        let entries: Vec<Entry> = serde_json::from_value(fx["entries"].clone()).unwrap();
        let live: HashMap<String, LiveStatus> = fx["live"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, v)| {
                let status = match v["status"].as_str().unwrap() {
                    "asking" => "asking", "running" => "running", "background" => "background", "active" => "active", _ => "paused",
                };
                (id.clone(), LiveStatus { status, updated_at: v["updated_at"].as_str().unwrap().to_string() })
            })
            .collect();
        let d = dashboard(&entries, &live);
        assert_eq!(serde_json::to_value(&d.sessions).unwrap(), fx["sessions"], "卡片（含顺序）与共享向量不一致");
        assert_eq!(serde_json::to_value(&d.counts).unwrap(), fx["expect"]["counts"]);
    }

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
    fn cards_sort_by_status_then_update_and_counts_skip_deleted() {
        let ls = |status: &'static str, t: &str| LiveStatus { status, updated_at: t.into() };
        let mut live = HashMap::new();
        live.insert("act".to_string(), ls("active", "2026-09-07T09:00:00Z"));
        live.insert("bg".to_string(), ls("background", "2026-09-07T01:00:00Z"));
        live.insert("run".to_string(), ls("running", "2026-09-07T02:00:00Z"));
        live.insert("ask".to_string(), ls("asking", "2026-09-07T00:00:00Z"));
        live.insert("gone_pooled".to_string(), ls("paused", "2026-09-07T05:00:00Z"));
        let mut gone = e("gone", "2026-09-06T00:00:00Z", "老会话", "- [ ] 没做完");
        gone.ended_at = Some("2026-09-06T03:00:00Z".into());
        let mut del = e("del", "2026-09-07T10:00:00Z", "删了", "- [ ] 不该计数");
        del.deleted_at = Some("t".into());
        let mut shell = e("t1", "2026-09-07T11:00:00Z", "zsh", "- [ ] 终端不进看板");
        shell.agent = "shell".into();
        let entries = vec![
            e("act", "2026-09-07T00:00:00Z", "激活的", "- [x] a\n- [ ] b"),
            e("bg", "2026-09-07T00:00:00Z", "后台的", "- [ ] c"),
            e("run", "2026-09-07T00:00:00Z", "运行的", ""),
            e("ask", "2026-09-07T00:00:00Z", "在问的", "- [x] d"),
            e("gone_pooled", "2026-09-07T00:00:00Z", "退出还在池子", "- [ ] e"),
            gone,
            del,
            shell,
        ];
        let d = dashboard(&entries, &live);
        let order: Vec<(&str, &str)> = d.sessions.iter().map(|c| (c.id.as_str(), c.status.as_str())).collect();
        assert_eq!(
            order,
            vec![
                ("ask", "asking"),
                ("run", "running"),
                ("bg", "background"),
                ("act", "active"),
                // paused 里按 updated_at：退出还在池子的 05:00 > 老会话的 ended_at 03:00；
                // 已删除的哪怕更新（10:00）也沉到组尾
                ("gone_pooled", "paused"),
                ("gone", "paused"),
                ("del", "paused"),
            ]
        );
        assert!(d.sessions.iter().all(|c| c.id != "t1"), "终端不进看板");
        let gp = d.sessions.iter().find(|c| c.id == "gone_pooled").unwrap();
        assert!(gp.alive, "还在池子里就能点开");
        assert!(!d.sessions.iter().find(|c| c.id == "gone").unwrap().alive);
        assert!(d.sessions.iter().find(|c| c.id == "del").unwrap().deleted);
        assert_eq!(
            d.counts,
            Counts { open_items: 4 },
            "已删除的不计数（open_items：b c e + 老会话的一项）"
        );
        let act = d.sessions.iter().find(|c| c.id == "act").unwrap();
        assert_eq!((act.done, act.open), (1, 1));
        assert_eq!(act.items[1].text, "b");
    }
}
