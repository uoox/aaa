//! `.aaa-agents` registry — line format `<dir>\t<agent>`, atomic whole-file
//! rewrite. Bidirectionally compatible with the aaa CLI.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

pub struct Registry {
    path: PathBuf,
    map: HashMap<String, String>,
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
                let mut it = line.splitn(2, '\t');
                let (Some(d), Some(a)) = (it.next(), it.next()) else { continue };
                if !d.is_empty() && !a.is_empty() {
                    map.insert(d.to_string(), a.to_string());
                }
            }
        }
        Registry { path, map }
    }

    pub fn get(&self, dir: &str) -> Option<&str> {
        self.map.get(dir).map(|s| s.as_str())
    }

    pub fn set(&mut self, dir: &str, agent: &str) -> io::Result<()> {
        self.map.insert(dir.to_string(), agent.to_string());
        self.flush()
    }

    pub fn unset(&mut self, dir: &str) -> io::Result<()> {
        if self.map.remove(dir).is_none() {
            return Ok(());
        }
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
        entries.sort();
        for (d, a) in entries {
            body.push_str(d);
            body.push('\t');
            body.push_str(a);
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
        assert_eq!(reg.get("badline"), None);
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
    fn refuses_to_create_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-mounted");
        let mut reg = Registry::load(&missing);
        assert!(reg.set("/x", "claude").is_err());
        assert!(!missing.exists(), "must not mkdir the project root");
    }
}
