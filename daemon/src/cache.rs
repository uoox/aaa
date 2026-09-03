//! `~/.cache/aaa-cwds.json` — cwd/naming cache shared with the aaa CLI.
//!
//! Keys: `claude:<path>` -> cwd string; `codex:<path>` / `pi:<path>` ->
//! `[sid, cwd]`; `cname2:<path>` -> `[mtime, name]`; `ainame:<path>` ->
//! `[size, name]`. On save, entries whose file (the part after the first `:`)
//! no longer exists are pruned — identical to AAA_PY `cc_save`.
//!
//! `save` is read-merge-write: it reloads the file and overlays only the keys
//! written through this instance. The aaa CLI (and other daemon handlers)
//! write the same file, and long-running work (haiku naming) happens between
//! our load and save — a plain overwrite would clobber whatever landed in the
//! meantime. Callers still serialize `save` itself (see `App::store_lock`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub struct CwdCache {
    path: PathBuf,
    map: serde_json::Map<String, Value>,
    /// keys written via `put` on this instance (the merge overlay on save)
    written: HashSet<String>,
}

fn read_map(path: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

impl CwdCache {
    pub fn load(path: &Path) -> Self {
        CwdCache { path: path.to_path_buf(), map: read_map(path), written: HashSet::new() }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    pub fn put(&mut self, key: impl Into<String>, val: Value) {
        let key = key.into();
        self.written.insert(key.clone());
        self.map.insert(key, val);
    }

    /// Whether `save` has anything to write. Callers use this to skip taking
    /// the store lock entirely on read-only passes.
    pub fn dirty(&self) -> bool {
        !self.written.is_empty()
    }

    /// Persist if dirty: reload the file, overlay this instance's writes,
    /// prune entries for files that no longer exist, write atomically.
    pub fn save(&mut self) {
        if self.written.is_empty() {
            return;
        }
        let mut merged = read_map(&self.path);
        for k in &self.written {
            if let Some(v) = self.map.get(k) {
                merged.insert(k.clone(), v.clone());
            }
        }
        let pruned: serde_json::Map<String, Value> = merged
            .into_iter()
            .filter(|(k, _)| {
                k.split_once(':')
                    .map(|(_, p)| Path::new(p).exists())
                    .unwrap_or(false)
            })
            .collect();
        if let Some(dir) = self.path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        if let Ok(body) = serde_json::to_string(&Value::Object(pruned)) {
            let _ = crate::paths::write_atomic(&self.path, body.as_bytes());
        }
        self.written.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_aaa_cli_cache_keys() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("sess.jsonl");
        std::fs::write(&f1, "").unwrap();
        let cache_path = dir.path().join("aaa-cwds.json");
        std::fs::write(
            &cache_path,
            format!(
                r#"{{"claude:{f}":"/Users/x/project/x","codex:{f}":["sid-1","/p"],"cname2:{f}":[1700000000.5,"标题"],"ainame:{f}":[1234,"摘要标题"]}}"#,
                f = f1.display()
            ),
        )
        .unwrap();
        let cache = CwdCache::load(&cache_path);
        assert_eq!(cache.get(&format!("claude:{}", f1.display())).unwrap(), "/Users/x/project/x");
        assert_eq!(
            cache.get(&format!("codex:{}", f1.display())).unwrap(),
            &json!(["sid-1", "/p"])
        );
        assert_eq!(
            cache.get(&format!("cname2:{}", f1.display())).unwrap()[1],
            "标题"
        );
    }

    #[test]
    fn save_prunes_dead_entries_and_is_readable_back() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live.jsonl");
        std::fs::write(&live, "").unwrap();
        let cache_path = dir.path().join("aaa-cwds.json");
        let mut cache = CwdCache::load(&cache_path);
        cache.put(format!("claude:{}", live.display()), json!("/cwd/a"));
        cache.put(format!("claude:{}/gone.jsonl", dir.path().display()), json!("/cwd/b"));
        cache.save();
        let re = CwdCache::load(&cache_path);
        assert!(re.get(&format!("claude:{}", live.display())).is_some());
        assert!(
            re.get(&format!("claude:{}/gone.jsonl", dir.path().display())).is_none(),
            "entries for deleted files are pruned on save"
        );
        // plain JSON object on disk — parseable by python json.load
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        assert!(raw.is_object());
    }

    #[test]
    fn save_merges_with_concurrent_writers() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("a.jsonl");
        let f2 = dir.path().join("b.jsonl");
        std::fs::write(&f1, "").unwrap();
        std::fs::write(&f2, "").unwrap();
        let cache_path = dir.path().join("aaa-cwds.json");
        // two instances loaded from the same (empty) file, as two concurrent
        // handlers would be
        let mut c1 = CwdCache::load(&cache_path);
        let mut c2 = CwdCache::load(&cache_path);
        c1.put(format!("claude:{}", f1.display()), json!("/cwd/1"));
        c2.put(format!("claude:{}", f2.display()), json!("/cwd/2"));
        c1.save();
        c2.save(); // must not clobber c1's entry
        let re = CwdCache::load(&cache_path);
        assert!(re.get(&format!("claude:{}", f1.display())).is_some(), "first writer survives");
        assert!(re.get(&format!("claude:{}", f2.display())).is_some(), "second writer present");
        // saving with no writes is a no-op (no lock contention, no file touch)
        let before = std::fs::metadata(&cache_path).unwrap().modified().unwrap();
        let mut c3 = CwdCache::load(&cache_path);
        c3.save();
        assert_eq!(std::fs::metadata(&cache_path).unwrap().modified().unwrap(), before);
    }
}
