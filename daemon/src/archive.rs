//! 归档的项目（2026-09-07 用户要求：不想每次都删除 + 确认）：一个路径集合，存在
//! `<state_dir>/archived.json`，三端一起变。归档 = 从列表和看板里藏起来（一个开关能
//! 翻出来），目录、对话、日志一样不动；取消归档就回来。归档时还活着的会话先结束
//! （不确认——归档本来就是「先放一边」）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub struct Archive {
    file: PathBuf,
    set: BTreeSet<String>,
}

impl Archive {
    pub fn load(state_dir: &Path) -> Self {
        let file = state_dir.join("archived.json");
        let set = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        Archive { file, set }
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

    pub fn is_archived(&self, path: &str) -> bool {
        self.set.contains(path)
    }

    pub fn all(&self) -> std::collections::HashSet<String> {
        self.set.iter().cloned().collect()
    }

    /// Returns whether anything changed.
    pub fn set_archived(&mut self, path: &str, archived: bool) -> bool {
        let changed = if archived { self.set.insert(path.to_string()) } else { self.set.remove(path) };
        if changed {
            self.save();
        }
        changed
    }

    /// 删掉的项目不该还留在归档集合里
    pub fn forget(&mut self, path: &str) {
        if self.set.remove(path) {
            self.save();
        }
    }

    /// 迁根：路径前缀跟着换
    pub fn reroot(&mut self, old: &Path, new: &Path) -> usize {
        let mut n = 0;
        let next: BTreeSet<String> = self
            .set
            .iter()
            .map(|p| match crate::migrate::reroot(p, old, new) {
                Some(np) => {
                    n += 1;
                    np
                }
                None => p.clone(),
            })
            .collect();
        if n > 0 {
            self.set = next;
            self.save();
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_roundtrip_and_reroot() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = Archive::load(dir.path());
        assert!(a.set_archived("/p/a", true));
        assert!(!a.set_archived("/p/a", true));
        assert!(Archive::load(dir.path()).is_archived("/p/a"), "persisted");
        assert_eq!(a.reroot(Path::new("/p"), Path::new("/q")), 1);
        assert!(Archive::load(dir.path()).is_archived("/q/a"));
        a.forget("/q/a");
        assert!(!Archive::load(dir.path()).is_archived("/q/a"));
    }
}
