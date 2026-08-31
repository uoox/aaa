//! `.aaa-agents` registry — line format `<dir>\t<agent>[\t<conversation id>]`,
//! atomic whole-file rewrite. Bidirectionally compatible with the aaa CLI.
//!
//! The third column records the agent conversation id for that directory, so a
//! project can be resumed after the project root moves: agent stores key their
//! sessions by cwd, and a migrated path finds nothing there — the registry id
//! is the fallback that still names the old conversation.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
struct Entry {
    agent: String,
    id: Option<String>,
}

pub struct Registry {
    path: PathBuf,
    map: HashMap<String, Entry>,
}

static REG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 进程内注册表写锁。load→修改→flush 不是原子的：并发 handler 各自 load
/// 再 flush 会互相覆盖丢行——名册行丢了 = 项目从所有客户端消失（审查 P1）。
/// 每段「load + 修改」都要先拿这把锁；纯读可以不拿。
pub fn lock() -> std::sync::MutexGuard<'static, ()> {
    REG_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

impl Registry {
    /// Registry file lives at `<project_root>/.aaa-agents`.
    pub fn registry_path(project_root: &Path) -> PathBuf {
        project_root.join(".aaa-agents")
    }

    pub fn load(project_root: &Path) -> Self {
        let path = Self::registry_path(project_root);
        let mut map = HashMap::new();
        if let Ok(body) = std::fs::read_to_string(&path) {
            for line in body.lines() {
                let mut it = line.splitn(3, '\t');
                let (Some(d), Some(a)) = (it.next(), it.next()) else { continue };
                if d.is_empty() || a.is_empty() {
                    continue;
                }
                let id = it.next().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                map.insert(d.to_string(), Entry { agent: a.to_string(), id });
            }
        }
        Registry { path, map }
    }

    pub fn get(&self, dir: &str) -> Option<&str> {
        self.map.get(dir).map(|e| e.agent.as_str())
    }

    pub fn get_id(&self, dir: &str) -> Option<&str> {
        self.map.get(dir).and_then(|e| e.id.as_deref())
    }

    /// All `(dir, agent, id)` rows, for migration sweeps.
    pub fn entries(&self) -> Vec<(String, String, Option<String>)> {
        self.map
            .iter()
            .map(|(d, e)| (d.clone(), e.agent.clone(), e.id.clone()))
            .collect()
    }

    pub fn set(&mut self, dir: &str, agent: &str) -> io::Result<()> {
        let e = self.map.entry(dir.to_string()).or_default();
        if e.agent != agent {
            // 换 agent 的同时旧对话 id 就没有意义了
            e.id = None;
        }
        e.agent = agent.to_string();
        self.flush()
    }

    /// Record the conversation id for a directory (agent entry must make sense
    /// to the caller; a missing row is created with the given agent).
    pub fn set_id(&mut self, dir: &str, agent: &str, id: &str) -> io::Result<()> {
        let e = self.map.entry(dir.to_string()).or_default();
        if e.agent.is_empty() {
            e.agent = agent.to_string();
        }
        if e.agent == agent && e.id.as_deref() == Some(id) {
            return Ok(()); // no churn, no write
        }
        e.agent = agent.to_string();
        e.id = Some(id.to_string());
        self.flush()
    }

    /// 批量落 id，一次 flush——迁移前采集逐条 set_id 是 O(n²) 次整文件写
    pub fn set_ids(&mut self, triples: &[(String, String, String)]) -> io::Result<()> {
        let mut dirty = false;
        for (dir, agent, id) in triples {
            let e = self.map.entry(dir.clone()).or_default();
            if e.agent == *agent && e.id.as_deref() == Some(id) {
                continue;
            }
            e.agent = agent.clone();
            e.id = Some(id.clone());
            dirty = true;
        }
        if dirty { self.flush() } else { Ok(()) }
    }

    /// 兜底 id 被证实已失效（agent 存储里找不到）时清掉，别反复撞同一堵墙
    pub fn clear_id(&mut self, dir: &str) -> io::Result<()> {
        match self.map.get_mut(dir) {
            Some(e) if e.id.is_some() => {
                e.id = None;
                self.flush()
            }
            _ => Ok(()),
        }
    }

    pub fn unset(&mut self, dir: &str) -> io::Result<()> {
        if self.map.remove(dir).is_none() {
            return Ok(());
        }
        self.flush()
    }

    /// After the project root moved from `old_root` to `new_root`: rewrite every
    /// key under the old prefix to the new one. Call with a Registry loaded from
    /// the NEW root (the file travelled with the move).
    pub fn rewrite_prefix(&mut self, old_root: &Path, new_root: &Path) -> io::Result<()> {
        let old_s = old_root.to_string_lossy().into_owned();
        let mut next = HashMap::new();
        for (d, e) in self.map.drain() {
            let nd = match Path::new(&d).strip_prefix(old_root) {
                Ok(rel) => new_root.join(rel).to_string_lossy().into_owned(),
                Err(_) if d == old_s => new_root.to_string_lossy().into_owned(),
                Err(_) => d,
            };
            next.insert(nd, e);
        }
        self.map = next;
        self.flush()
    }

    /// Whether an entry still refers to something real, and so should survive a
    /// rewrite. Entries accumulate whenever a project is removed by any route
    /// other than the delete API — Finder, `rm -rf`, an agent tidying up — and
    /// nothing ever collected them (the aaa CLI drops such rows only on its own
    /// `d` key), so a long-lived registry ends up mostly stale rows naming
    /// directories that no longer exist. Self-healing on write mirrors what the
    /// CLI's cwd cache already does when it saves.
    ///
    /// Only entries *inside* the project root are judged: the root is known
    /// mounted here (flush checks), so a missing child is genuinely gone. Paths
    /// elsewhere are kept untouched — their volume may simply not be mounted.
    fn is_live(&self, dir: &str) -> bool {
        let Some(root) = self.path.parent() else { return true };
        let p = Path::new(dir);
        if !p.starts_with(root) {
            return true;
        }
        p.is_dir()
    }

    fn flush(&self) -> io::Result<()> {
        // Atomic whole-file rewrite (tmp + rename), like registry_flush in aaa.
        // Never create the parent (= project root): SSD guard.
        let Some(parent) = self.path.parent() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "registry has no parent dir"));
        };
        if !parent.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "project root not mounted"));
        }
        let mut body = String::new();
        let mut entries: Vec<_> = self.map.iter().filter(|(d, _)| self.is_live(d)).collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (d, e) in entries {
            body.push_str(d);
            body.push('\t');
            body.push_str(&e.agent);
            if let Some(id) = &e.id {
                body.push('\t');
                body.push_str(id);
            }
            body.push('\n');
        }
        let tmp = self.path.with_file_name(format!(".aaa-agents.tmp.{}", std::process::id()));
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_aaa_cli_format() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join(".aaa-agents"),
            "/Volumes/SSD/project/foo\tclaude\n/Volumes/SSD/project/bar\tcodex\nbadline\n",
        )
        .unwrap();
        let reg = Registry::load(root);
        assert_eq!(reg.get("/Volumes/SSD/project/foo"), Some("claude"));
        assert_eq!(reg.get("/Volumes/SSD/project/bar"), Some("codex"));
        assert_eq!(reg.get_id("/Volumes/SSD/project/foo"), None);
        assert_eq!(reg.get("badline"), None);
    }

    #[test]
    fn conversation_id_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("proj");
        std::fs::create_dir(&p).unwrap();
        let ps = p.to_string_lossy().into_owned();
        let mut reg = Registry::load(root);
        reg.set_id(&ps, "claude", "sess-uuid-1").unwrap();
        let body = std::fs::read_to_string(root.join(".aaa-agents")).unwrap();
        assert_eq!(body, format!("{ps}\tclaude\tsess-uuid-1\n"));
        let reg2 = Registry::load(root);
        assert_eq!(reg2.get(&ps), Some("claude"));
        assert_eq!(reg2.get_id(&ps), Some("sess-uuid-1"));
    }

    #[test]
    fn changing_agent_drops_stale_id() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("proj");
        std::fs::create_dir(&p).unwrap();
        let ps = p.to_string_lossy().into_owned();
        let mut reg = Registry::load(root);
        reg.set_id(&ps, "claude", "id-1").unwrap();
        reg.set(&ps, "codex").unwrap();
        assert_eq!(reg.get_id(&ps), None, "claude 的 id 对 codex 没有意义");
        // 同 agent 重复 set 保留 id
        reg.set_id(&ps, "codex", "id-2").unwrap();
        reg.set(&ps, "codex").unwrap();
        assert_eq!(reg.get_id(&ps), Some("id-2"));
    }

    #[test]
    fn clear_id_keeps_the_agent_row() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let p = root.join("proj");
        std::fs::create_dir(&p).unwrap();
        let ps = p.to_string_lossy().into_owned();
        let mut reg = Registry::load(root);
        reg.set_id(&ps, "claude", "dead-id").unwrap();
        reg.clear_id(&ps).unwrap();
        assert_eq!(reg.get(&ps), Some("claude"), "行还在");
        assert_eq!(reg.get_id(&ps), None, "坏 id 清掉");
        // 幂等：没 id 时不写盘也不报错
        reg.clear_id(&ps).unwrap();
    }

    #[test]
    fn writes_format_readable_by_aaa_cli() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut reg = Registry::load(root);
        reg.set("/x/one", "pi").unwrap();
        reg.set("/x/two", "reasonix").unwrap();
        reg.unset("/x/one").unwrap();
        let body = std::fs::read_to_string(root.join(".aaa-agents")).unwrap();
        assert_eq!(body, "/x/two\treasonix\n");
        // and round-trips through our own loader
        let reg2 = Registry::load(root);
        assert_eq!(reg2.get("/x/two"), Some("reasonix"));
        assert_eq!(reg2.get("/x/one"), None);
    }

    #[test]
    fn rewrite_drops_entries_whose_directory_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let live = root.join("live");
        std::fs::create_dir_all(&live).unwrap();
        let live_s = live.to_string_lossy().into_owned();
        let gone_s = root.join("gone").to_string_lossy().into_owned();
        // An external path on a possibly-unmounted volume must be preserved.
        let external = "/Volumes/Other/project/x";
        std::fs::write(
            root.join(".aaa-agents"),
            format!("{live_s}\tclaude\n{gone_s}\treasonix\n{external}\tcodex\n"),
        )
        .unwrap();

        // Loading preserves everything; only a write prunes.
        let mut reg = Registry::load(root);
        assert_eq!(reg.get(&gone_s), Some("reasonix"));
        reg.set(&live_s, "codex").unwrap();

        let body = std::fs::read_to_string(root.join(".aaa-agents")).unwrap();
        assert!(body.contains(&format!("{live_s}\tcodex")), "live entry updated");
        assert!(!body.contains(&gone_s), "orphaned entry pruned on write");
        assert!(body.contains(external), "entry outside the root is kept");
    }

    #[test]
    fn rewrite_prefix_moves_keys_and_keeps_ids() {
        let old = tempfile::tempdir().unwrap();
        let new = tempfile::tempdir().unwrap();
        let proj_old = old.path().join("p1").to_string_lossy().into_owned();
        std::fs::create_dir_all(new.path().join("p1")).unwrap();
        std::fs::write(
            new.path().join(".aaa-agents"),
            format!("{proj_old}\tclaude\tid-9\n/Volumes/Other/x\tcodex\n"),
        )
        .unwrap();
        let mut reg = Registry::load(new.path());
        reg.rewrite_prefix(old.path(), new.path()).unwrap();
        let np = new.path().join("p1").to_string_lossy().into_owned();
        assert_eq!(reg.get(&np), Some("claude"));
        assert_eq!(reg.get_id(&np), Some("id-9"));
        assert_eq!(reg.get("/Volumes/Other/x"), Some("codex"), "外部路径不动");
        let body = std::fs::read_to_string(new.path().join(".aaa-agents")).unwrap();
        assert!(body.contains(&format!("{np}\tclaude\tid-9")));
    }

    #[test]
    fn refuses_to_create_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-mounted");
        let mut reg = Registry::load(&missing);
        assert!(reg.set("/x", "claude").is_err());
        assert!(!missing.exists(), "must not mkdir the project root");
    }
}
