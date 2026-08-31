//! Task inbox: per-project queued instructions, auto-fed into the PTY the
//! first time a session goes waiting. Stored daemon-side in
//! `<state_dir>/inbox.json` (atomic rewrite), so entries survive restarts and
//! can be queued even while the SSD is unmounted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub text: String,
    pub created_at: String,
}

pub struct Inbox {
    file: PathBuf,
    map: HashMap<String, Vec<Entry>>, // project path -> entries (FIFO)
}

fn new_id() -> String {
    use rand::Rng;
    let n: u32 = rand::rng().random();
    format!("in_{n:08x}")
}

impl Inbox {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("inbox.json");
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Inbox { file, map }
    }

    fn save(&self) {
        if let Some(dir) = self.file.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        if let Ok(body) = serde_json::to_string_pretty(&self.map) {
            let _ = crate::paths::write_atomic(&self.file, body.as_bytes());
        }
    }

    pub fn list(&self, path: &str) -> Vec<Entry> {
        self.map.get(path).cloned().unwrap_or_default()
    }

    pub fn add(&mut self, path: &str, text: &str) -> Entry {
        let entry = Entry {
            id: new_id(),
            text: text.to_string(),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        };
        self.map.entry(path.to_string()).or_default().push(entry.clone());
        self.save();
        entry
    }

    /// Remove one entry by id anywhere; returns the project path it was under.
    pub fn remove(&mut self, id: &str) -> Option<String> {
        let mut hit: Option<String> = None;
        for (path, entries) in self.map.iter_mut() {
            if let Some(pos) = entries.iter().position(|e| e.id == id) {
                entries.remove(pos);
                hit = Some(path.clone());
                break;
            }
        }
        if let Some(path) = &hit {
            if self.map.get(path).map(|v| v.is_empty()).unwrap_or(false) {
                self.map.remove(path);
            }
            self.save();
        }
        hit
    }

    /// Drain all entries for a project (used by auto-feed).
    pub fn take_all(&mut self, path: &str) -> Vec<Entry> {
        let entries = self.map.remove(path).unwrap_or_default();
        if !entries.is_empty() {
            self.save();
        }
        entries
    }
}

/// Compose the auto-feed message: `任务清单：\n1. …\n2. …`.
pub fn compose_feed(entries: &[Entry]) -> String {
    let mut out = String::from("任务清单：\n");
    for (i, e) in entries.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, e.text.trim()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let mut inbox = Inbox::load(dir.path());
        assert!(inbox.list("/p/a").is_empty());
        let e1 = inbox.add("/p/a", "修复登录 bug");
        let e2 = inbox.add("/p/a", "加个深色模式");
        let e3 = inbox.add("/p/b", "另一个项目的任务");
        assert_eq!(inbox.list("/p/a").len(), 2);
        assert_eq!(inbox.list("/p/b").len(), 1);
        // reload from disk
        let mut inbox2 = Inbox::load(dir.path());
        assert_eq!(inbox2.list("/p/a").len(), 2);
        assert_eq!(inbox2.list("/p/a")[0].id, e1.id);
        // remove by id (searches across projects)
        assert_eq!(inbox2.remove(&e3.id).as_deref(), Some("/p/b"));
        assert!(inbox2.remove("in_nonexistent").is_none());
        assert!(inbox2.list("/p/b").is_empty());
        // drain
        let drained = inbox2.take_all("/p/a");
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[1].id, e2.id);
        assert!(inbox2.take_all("/p/a").is_empty());
        let inbox3 = Inbox::load(dir.path());
        assert!(inbox3.list("/p/a").is_empty(), "drain persisted");
    }

    #[test]
    fn feed_composition() {
        let entries = vec![
            Entry { id: "1".into(), text: " 任务甲 ".into(), created_at: "t".into() },
            Entry { id: "2".into(), text: "任务乙".into(), created_at: "t".into() },
        ];
        assert_eq!(compose_feed(&entries), "任务清单：\n1. 任务甲\n2. 任务乙\n");
    }
}
