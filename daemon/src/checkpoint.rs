//! v1.1 git checkpoints: start/end/auto snapshots of the project worktree,
//! diff against the start checkpoint, and worktree rollback.
//!
//! Hard constraint: NEVER touch the project's HEAD / index / worktree state
//! beyond what is explicitly requested. All snapshot operations run through a
//! temporary `GIT_INDEX_FILE`; refs live under `refs/aaa-ckpt/<sid>/…` so no
//! branch or reflog is polluted. `git init` only happens when the project has
//! no repo and `auto_init_git=true` (sanctioned by PROTOCOL.md).
//!
//! Rollback is deletion-class code (first-phase red line): every path it
//! removes is a relative path produced by `git ls-tree` on our own snapshot,
//! re-validated to be a plain child path, and joined under the project dir.

use std::path::{Path, PathBuf};
use std::time::Instant;

pub const PATCH_CAP_BYTES: usize = 64 * 1024;
const MAX_DIFF_FILES: usize = 500;

#[derive(Clone, Debug, Default)]
pub struct CkptState {
    pub start_ref: Option<String>,
    pub last_tree: Option<String>,
    pub count: u32,
    pub last_at: Option<Instant>,
}

#[derive(Clone, Debug, serde::Serialize, PartialEq)]
pub struct DiffFile {
    pub path: String,
    pub status: String, // added | modified | deleted
    pub additions: u64,
    pub deletions: u64,
    pub patch: String,
    pub truncated: bool,
}

fn git(dir: &Path, index: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd.env("GIT_AUTHOR_NAME", "aaa-daemon")
        .env("GIT_AUTHOR_EMAIL", "aaa-daemon@localhost")
        .env("GIT_COMMITTER_NAME", "aaa-daemon")
        .env("GIT_COMMITTER_EMAIL", "aaa-daemon@localhost")
        .env("GIT_OPTIONAL_LOCKS", "0");
    if let Some(ix) = index {
        cmd.env("GIT_INDEX_FILE", ix);
    }
    let out = cmd
        .output()
        .map_err(|e| format!("git spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

pub fn has_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Ensure the project is a git repo; init only when allowed.
pub fn ensure_repo(dir: &Path, auto_init: bool, max_mb: u64) -> Result<(), String> {
    if has_repo(dir) {
        return Ok(());
    }
    if !auto_init {
        return Err("no .git and auto_init_git disabled".into());
    }
    // Guard against ballooning `.git/objects` when a session opens in a large
    // non-code directory (media dumps, extracted archives). The probe exits
    // early the moment the budget is blown, so big directories cost little.
    if max_mb > 0 && exceeds_size_budget(dir, max_mb.saturating_mul(1024 * 1024)) {
        return Err(format!("project exceeds auto_init_max_mb ({max_mb}MB); skipping git init"));
    }
    git(dir, None, &["init", "-q"]).map(|_| ())
}

/// True as soon as the cumulative size of regular files under `dir` exceeds
/// `budget` bytes. Skips nested `.git` and does not follow symlinks; returns
/// early to keep the cost proportional to the budget, not the tree.
fn exceeds_size_budget(dir: &Path, budget: u64) -> bool {
    fn walk(dir: &Path, budget: u64, acc: &mut u64) -> bool {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return false,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if entry.file_name() == ".git" {
                    continue;
                }
                if walk(&entry.path(), budget, acc) {
                    return true;
                }
            } else if meta.is_file() {
                *acc = acc.saturating_add(meta.len());
                if *acc > budget {
                    return true;
                }
            }
        }
        false
    }
    let mut acc = 0u64;
    walk(dir, budget, &mut acc)
}

struct TmpIndex(PathBuf);

impl TmpIndex {
    fn new() -> Self {
        use rand::Rng;
        let n: u64 = rand::rng().random();
        TmpIndex(std::env::temp_dir().join(format!("aaa-ckpt-index-{n:016x}")))
    }
}

impl Drop for TmpIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Snapshot the current worktree (tracked + untracked, .gitignore respected)
/// into a tree object, via a throwaway index. Never touches `.git/index`.
pub fn snapshot_tree(dir: &Path) -> Result<String, String> {
    let ix = TmpIndex::new();
    git(dir, Some(&ix.0), &["add", "-A"])?;
    git(dir, Some(&ix.0), &["write-tree"])
}

/// Create a checkpoint ref. For `auto`, skip when nothing changed since the
/// previous checkpoint. Returns the ref name when one was created.
pub fn make_checkpoint(
    dir: &Path,
    session_id: &str,
    state: &mut CkptState,
    label: &str,
) -> Result<Option<String>, String> {
    let tree = snapshot_tree(dir)?;
    if label == "auto" && state.last_tree.as_deref() == Some(tree.as_str()) {
        state.last_at = Some(Instant::now());
        return Ok(None);
    }
    let msg = format!(
        "aaa-ckpt {label} {session_id} {}",
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    let commit = git(dir, None, &["commit-tree", &tree, "-m", &msg])?;
    let refname = format!("refs/aaa-ckpt/{session_id}/{}-{label}", state.count);
    git(dir, None, &["update-ref", &refname, &commit])?;
    state.count += 1;
    state.last_tree = Some(tree);
    state.last_at = Some(Instant::now());
    if label == "start" {
        state.start_ref = Some(refname.clone());
    }
    Ok(Some(refname))
}

fn ref_tree(dir: &Path, refname: &str) -> Result<String, String> {
    // refs come from our own metadata, but it lives in an editable state file:
    // only ever resolve refs inside our namespace, and never option-like args.
    if !refname.starts_with("refs/aaa-ckpt/") || refname.contains("..") {
        return Err(format!("refusing non-checkpoint ref: {refname}"));
    }
    git(dir, None, &["rev-parse", &format!("{refname}^{{tree}}")])
}

fn ls_tree_files(dir: &Path, tree: &str) -> Result<Vec<String>, String> {
    let out = git(dir, None, &["ls-tree", "-r", "-z", "--name-only", tree])?;
    Ok(out
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

/// start-checkpoint tree vs current worktree (untracked included).
pub fn diff(dir: &Path, start_ref: &str) -> Result<Vec<DiffFile>, String> {
    let base = ref_tree(dir, start_ref)?;
    let work = snapshot_tree(dir)?;
    // status (A/M/D), NUL-separated: "M\0path\0M\0path2\0…"
    let ns = git(dir, None, &["diff-tree", "-r", "-z", "--no-renames", "--name-status", &base, &work])?;
    let mut statuses: Vec<(String, String)> = Vec::new();
    let mut it = ns.split('\0').filter(|s| !s.is_empty());
    while let (Some(st), Some(path)) = (it.next(), it.next()) {
        statuses.push((st.to_string(), path.to_string()));
        if statuses.len() >= MAX_DIFF_FILES {
            break;
        }
    }
    // numstat: "adds\tdels\tpath\0…"
    let num = git(dir, None, &["diff-tree", "-r", "-z", "--no-renames", "--numstat", &base, &work])?;
    let mut counts: std::collections::HashMap<String, (u64, u64)> = Default::default();
    for entry in num.split('\0').filter(|s| !s.is_empty()) {
        let mut parts = entry.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        counts.insert(
            p.to_string(),
            (a.parse().unwrap_or(0), d.parse().unwrap_or(0)),
        );
    }
    let mut files = Vec::new();
    for (st, path) in statuses {
        let status = match st.as_str() {
            "A" => "added",
            "D" => "deleted",
            _ => "modified",
        };
        let (additions, deletions) = counts.get(&path).copied().unwrap_or((0, 0));
        let patch_full = git(
            dir,
            None,
            &["diff-tree", "-r", "-p", "--no-renames", &base, &work, "--", &path],
        )
        .unwrap_or_default();
        let (patch, truncated) = if patch_full.len() > PATCH_CAP_BYTES {
            let mut cut = PATCH_CAP_BYTES;
            while cut > 0 && !patch_full.is_char_boundary(cut) {
                cut -= 1;
            }
            (patch_full[..cut].to_string(), true)
        } else {
            (patch_full, false)
        };
        files.push(DiffFile {
            path,
            status: status.to_string(),
            additions,
            deletions,
            patch,
            truncated,
        });
    }
    Ok(files)
}

/// Validate a git-relative path before deleting: plain relative child path,
/// no parent refs, never inside .git.
fn safe_rel_path(rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
        return false;
    }
    let mut comps = rel.split('/');
    if comps.clone().any(|c| c == ".." || c.is_empty()) {
        return false;
    }
    comps.next() != Some(".git")
}

/// Restore the worktree to the start checkpoint:
/// - every file in the start tree is written back (temp index + checkout-index)
/// - files created after start (tracked or untracked, non-ignored) are removed
/// - `.git` and ignored files are never touched.
pub fn rollback(dir: &Path, start_ref: &str) -> Result<(u64, u64), String> {
    let base = ref_tree(dir, start_ref)?;
    // snapshot BEFORE restoring: current file set (ignores respected)
    let work = snapshot_tree(dir)?;
    let current: std::collections::BTreeSet<String> =
        ls_tree_files(dir, &work)?.into_iter().collect();
    let start: std::collections::BTreeSet<String> =
        ls_tree_files(dir, &base)?.into_iter().collect();

    // restore start-tree contents through a throwaway index
    let ix = TmpIndex::new();
    git(dir, Some(&ix.0), &["read-tree", &base])?;
    git(dir, Some(&ix.0), &["checkout-index", "-a", "-f"])?;

    // delete files that did not exist at start
    let dir_canon = std::fs::canonicalize(dir).map_err(|e| format!("canonicalize: {e}"))?;
    let mut deleted = 0u64;
    let mut prune_dirs: std::collections::BTreeSet<PathBuf> = Default::default();
    for rel in current.difference(&start) {
        if !safe_rel_path(rel) {
            continue; // defence in depth; git never emits such paths
        }
        let target = dir_canon.join(rel);
        // belt and braces: the joined path must stay under the project dir
        if !target.starts_with(&dir_canon) {
            continue;
        }
        if std::fs::remove_file(&target).is_ok() {
            deleted += 1;
            if let Some(parent) = target.parent() {
                if parent != dir_canon {
                    prune_dirs.insert(parent.to_path_buf());
                }
            }
        }
    }
    // prune now-empty directories (deepest first), never the project root
    for d in prune_dirs.iter().rev() {
        let mut cur = d.clone();
        while cur != dir_canon && cur.starts_with(&dir_canon) {
            // remove_dir fails on non-empty dirs — that is the safety net
            if std::fs::remove_dir(&cur).is_err() {
                break;
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break,
            }
        }
    }
    Ok((start.len() as u64, deleted))
}

/// Auto-checkpoint due check (pure, unit-testable).
pub fn auto_due(last_at: Option<Instant>, interval_minutes: u64, now: Instant) -> bool {
    match last_at {
        None => true,
        Some(t) => now.duration_since(t).as_secs() >= interval_minutes * 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_rel_paths() {
        assert!(safe_rel_path("a.txt"));
        assert!(safe_rel_path("sub/dir/file.rs"));
        assert!(!safe_rel_path("../escape"));
        assert!(!safe_rel_path("a/../../b"));
        assert!(!safe_rel_path("/abs"));
        assert!(!safe_rel_path(".git/config"));
        assert!(!safe_rel_path(""));
        assert!(!safe_rel_path("a//b"));
        assert!(safe_rel_path(".gitignore"));
    }

    #[test]
    fn ref_namespace_guard() {
        let dir = std::env::temp_dir();
        assert!(ref_tree(&dir, "HEAD").is_err());
        assert!(ref_tree(&dir, "refs/heads/main").is_err());
        assert!(ref_tree(&dir, "--output=/tmp/x").is_err());
        assert!(ref_tree(&dir, "refs/aaa-ckpt/../heads/main").is_err());
        // in-namespace refs pass the guard (resolution itself may still fail)
    }

    #[test]
    fn auto_due_logic() {
        let now = Instant::now();
        assert!(auto_due(None, 10, now));
        assert!(!auto_due(Some(now), 10, now));
        // can't fabricate an Instant 10min in the past portably; the false
        // case above plus the None case cover the branch structure.
    }
}
