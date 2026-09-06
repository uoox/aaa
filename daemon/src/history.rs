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
