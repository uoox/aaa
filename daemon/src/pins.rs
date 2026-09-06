//! Pinned projects: a set of project paths the user wants at the top of every
//! client's list. Stored daemon-side in `<state_dir>/pins.json` so the three
//! clients agree; `projects_changed` is broadcast on every change.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Pins {
    file: PathBuf,
    set: BTreeSet<String>,
}

impl Pins {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("pins.json");
        let set = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Pins { file, set }
    }

    fn save(&self) {
        if let Some(dir) = self.file.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let v: Vec<&String> = self.set.iter().collect();
        if let Ok(body) = serde_json::to_string_pretty(&v) {
            let _ = crate::paths::write_atomic(&self.file, body.as_bytes());
        }
    }

    pub fn is_pinned(&self, path: &str) -> bool {
        self.set.contains(path)
    }

    /// Returns whether anything changed.
    pub fn set_pinned(&mut self, path: &str, pinned: bool) -> bool {
        let changed = if pinned { self.set.insert(path.to_string()) } else { self.set.remove(path) };
        if changed {
            self.save();
        }
        changed
    }

    /// A deleted project must not stay pinned.
    pub fn forget(&mut self, path: &str) {
        if self.set.remove(path) {
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_unpin_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut p = Pins::load(dir.path());
        assert!(!p.is_pinned("/p/a"));
        assert!(p.set_pinned("/p/a", true));
        assert!(!p.set_pinned("/p/a", true), "already pinned: no change");
        assert!(Pins::load(dir.path()).is_pinned("/p/a"), "persisted");
        assert!(p.set_pinned("/p/a", false));
        assert!(!Pins::load(dir.path()).is_pinned("/p/a"));
        p.set_pinned("/p/b", true);
        p.forget("/p/b");
        assert!(!p.is_pinned("/p/b"));
    }
}
