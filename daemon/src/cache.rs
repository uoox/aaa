//! `~/.cache/aaa-cwds.json` — cwd/naming cache shared with the aaa CLI.
//!
//! Keys: `claude:<path>` -> cwd string; `codex:<path>` / `pi:<path>` ->
//! `[sid, cwd]`; `cname2:<path>` -> `[mtime, name]`; `ainame:<path>` ->
//! `[size, name]`. On save, entries whose file (the part after the first `:`)
//! no longer exists are pruned — identical to AAA_PY `cc_save`.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub struct CwdCache {
    path: PathBuf,
    map: serde_json::Map<String, Value>,
    dirty: bool,
}

impl CwdCache {
    pub fn load(path: &Path) -> Self {
        let map = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| match v {
                Value::Object(m) => Some(m),
                _ => None,
            })
            .unwrap_or_default();
        CwdCache { path: path.to_path_buf(), map, dirty: false }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    pub fn put(&mut self, key: impl Into<String>, val: Value) {
        self.map.insert(key.into(), val);
        self.dirty = true;
    }

    /// Persist if dirty, pruning entries for files that no longer exist.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let pruned: serde_json::Map<String, Value> = self
            .map
            .iter()
            .filter(|(k, _)| {
                k.splitn(2, ':')
                    .nth(1)
                    .map(|p| Path::new(p).exists())
                    .unwrap_or(false)
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(dir) = self.path.parent() {
            if std::fs::create_dir_all(dir).is_err() {
                return;
            }
        }
        let tmp = self.path.with_extension("json.tmp");
        if serde_json::to_string(&Value::Object(pruned))
            .ok()
            .and_then(|body| std::fs::write(&tmp, body).ok())
            .is_some()
        {
            let _ = std::fs::rename(&tmp, &self.path);
        }
        self.dirty = false;
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
                r#"{{"claude:{f}":"/Users/l/project/x","codex:{f}":["sid-1","/p"],"cname2:{f}":[1700000000.5,"标题"],"ainame:{f}":[1234,"摘要标题"]}}"#,
                f = f1.display()
            ),
        )
        .unwrap();
        let cache = CwdCache::load(&cache_path);
        assert_eq!(cache.get(&format!("claude:{}", f1.display())).unwrap(), "/Users/l/project/x");
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
}
