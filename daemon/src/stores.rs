//! Line-by-line port of AAA_PY (embedded in ~/.local/bin/aaa):
//! session-store iterators, find / detect / collect / purge.
//!
//! Store layout (unchanged from the script comments):
//!   claude:   ~/.claude/projects/*/<sid>.jsonl          (cwd field in lines)
//!   codex:    ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl (first line session_meta with cwd/id)
//!   pi:       ~/.pi/agent/sessions/--<cwd>--/<ts>_<uuid>.jsonl (first line type=session with cwd/id)
//!   reasonix: ~/.reasonix/projects/<cwd with / -> ->/sessions/<sid>.jsonl (+ .meta)
//!   agy:      ~/.gemini/antigravity-cli/  (history.jsonl / cache / conversations)
//!   grok:     ~/.grok/sessions/<url-enc-cwd>/<sid>/    (legacy; purge only)
//!
//! SAFETY: every path this module deletes is composed from the injected
//! `Paths` roots (home-derived). Nothing here touches the real HOME unless
//! `Paths` says so.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cache::CwdCache;
use crate::paths::Paths;

#[derive(Clone, Debug)]
pub struct SessRec {
    pub path: PathBuf,
    pub sid: String,
    pub cwd: String,
    pub size: u64,
    pub mtime: f64,
}

pub fn realpath(p: &str) -> String {
    std::fs::canonicalize(p)
        .map(|x| x.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

pub fn fstat(p: &Path) -> (u64, f64) {
    match std::fs::metadata(p) {
        Ok(md) => (md.len(), mtime_f(&md)),
        Err(_) => (0, 0.0),
    }
}

pub fn mtime_f(md: &std::fs::Metadata) -> f64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9)
        .unwrap_or(0.0)
}

/// Parse up to `limit + 1` jsonl lines from the head of a file, skipping
/// blank/broken lines (AAA_PY `jsonl_head`).
pub fn jsonl_head(p: &Path, limit: usize) -> Vec<Value> {
    use std::io::BufRead;
    let Ok(f) = std::fs::File::open(p) else { return vec![] };
    let reader = std::io::BufReader::new(f);
    let mut out = Vec::new();
    for (i, line) in reader.split(b'\n').enumerate() {
        if i > limit {
            break;
        }
        let Ok(bytes) = line else { break };
        let s = String::from_utf8_lossy(&bytes);
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            out.push(v);
        }
    }
    out
}

/// Parse complete jsonl lines within the last `nbytes` of a file
/// (AAA_PY `jsonl_tail`).
pub fn jsonl_tail(p: &Path, nbytes: u64) -> Vec<Value> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return vec![] };
    let Ok(size) = f.seek(SeekFrom::End(0)) else { return vec![] };
    let start = size.saturating_sub(nbytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return vec![];
    }
    let mut data = Vec::new();
    if f.read_to_end(&mut data).is_err() {
        return vec![];
    }
    let mut lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
    if size > nbytes && !lines.is_empty() {
        lines.remove(0); // drop the leading partial line
    }
    let mut out = Vec::new();
    for ln in lines {
        let s = String::from_utf8_lossy(ln);
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            out.push(v);
        }
    }
    out
}

// ---- glob helpers (python glob semantics: `*` skips dotfiles) ----

fn visible_entries(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut v: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| !n.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();
    v.sort();
    v
}

fn visible_dirs(dir: &Path) -> Vec<PathBuf> {
    visible_entries(dir).into_iter().filter(|p| p.is_dir()).collect()
}

fn visible_files(dir: &Path) -> Vec<PathBuf> {
    visible_entries(dir).into_iter().filter(|p| p.is_file()).collect()
}

fn val_str(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

// ---- session iterators ----

/// 注册表第三列兜底 id 的有效性检查：`Some(存在与否)`=能便宜验证（claude /
/// reasonix 的会话文件名就是 id，扫一层目录即可）；`None`=该 agent 没有便宜
/// 的验证手段（codex 要拆 rollout 文件、pi 根本不用 id），调用方按 best-effort
/// 继续用。目的：agent 那边把会话 GC 掉之后，别拿着坏 id 反复 resume 失败。
pub fn id_exists(paths: &Paths, agent: &str, id: &str) -> Option<bool> {
    if id.is_empty() || id.contains('/') || id.contains("..") {
        return Some(false);
    }
    let name = format!("{id}.jsonl");
    let scan = |root: std::path::PathBuf, nested: Option<&str>| -> bool {
        let Ok(rd) = std::fs::read_dir(&root) else { return false };
        for e in rd.flatten() {
            let dir = match nested {
                Some(sub) => e.path().join(sub),
                None => e.path(),
            };
            if dir.join(&name).is_file() {
                return true;
            }
        }
        false
    };
    match agent {
        "claude" => Some(scan(paths.claude_root(), None)),
        _ => None,
    }
}

pub fn claude_sessions(paths: &Paths, cache: &mut CwdCache) -> Vec<SessRec> {
    let mut out = Vec::new();
    for d in visible_dirs(&paths.claude_root()) {
        for fnm in visible_files(&d) {
            if fnm.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let key = format!("claude:{}", fnm.display());
            let mut cwd = cache.get(&key).map(val_str).unwrap_or_default();
            if cache.get(&key).is_none() {
                for o in jsonl_head(&fnm, 120) {
                    let c = val_str(o.get("cwd").unwrap_or(&Value::Null));
                    if !c.is_empty() {
                        cwd = c;
                        break;
                    }
                }
                if !cwd.is_empty() {
                    cache.put(key, Value::String(cwd.clone()));
                }
            }
            if cwd.is_empty() {
                continue;
            }
            let (size, mtime) = fstat(&fnm);
            let sid = fnm
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.trim_end_matches(".jsonl").to_string())
                .unwrap_or_default();
            out.push(SessRec { path: fnm, sid, cwd, size, mtime });
        }
    }
    out
}

// ---- find ----

fn newest<I: IntoIterator<Item = (f64, String)>>(pairs: I) -> String {
    let mut best: Option<(f64, String)> = None;
    for (mt, sid) in pairs {
        if sid.is_empty() {
            continue;
        }
        if best.as_ref().map(|b| mt > b.0).unwrap_or(true) {
            best = Some((mt, sid));
        }
    }
    best.map(|b| b.1).unwrap_or_default()
}

/// Find the most recent session id for (agent, cwd). `target` should already
/// be realpath'd by the caller (as the zsh caller does with `pwd -P`).
pub fn find(paths: &Paths, cache: &mut CwdCache, agent: &str, target: &str) -> String {
    match agent {
        "claude" => newest(
            claude_sessions(paths, cache)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        _ => String::new(),
    }
}

// ---- detect ----

/// Agent of the most recent session for `target`, across all stores. Part of
/// the ported AAA_PY surface (used by the store_debug example + parity tests).
pub fn detect(paths: &Paths, cache: &mut CwdCache, target: &str) -> String {
    let target = realpath(target);
    let hit = claude_sessions(paths, cache)
        .into_iter()
        .any(|r| !r.cwd.is_empty() && realpath(&r.cwd) == target);
    if hit { "claude".to_string() } else { String::new() }
}

// ---- collect ----

#[derive(Clone, Debug)]
pub struct ProjectRow {
    pub path: String,
    pub name: String,
    pub mtime: f64,
    pub dir_size: u64,
    pub ctx_size: Option<u64>,
    /// agent of the most recent session found in any store (None = none found)
    pub det_agent: Option<String>,
    /// session file path usable by the namer (claude jsonl / reasonix .meta)
    pub det_path: String,
}

pub fn collect(paths: &Paths, cache: &mut CwdCache, root: &Path) -> Vec<ProjectRow> {
    use std::collections::HashMap;
    let mut ctx: HashMap<String, (u64, f64)> = HashMap::new();
    let mut det: HashMap<String, (f64, String, String)> = HashMap::new();
    // Session records share few distinct cwds; memoize canonicalization so a
    // scan does one `canonicalize` per cwd instead of two per record.
    let mut memo: HashMap<String, String> = HashMap::new();

    fn realpath_memo(memo: &mut HashMap<String, String>, p: &str) -> String {
        if let Some(rp) = memo.get(p) {
            return rp.clone();
        }
        let rp = realpath(p);
        memo.insert(p.to_string(), rp.clone());
        rp
    }

    fn mark(
        det: &mut HashMap<String, (f64, String, String)>,
        memo: &mut HashMap<String, String>,
        cwd: &str,
        mtime: f64,
        agent: &str,
        path: &str,
    ) {
        if cwd.is_empty() {
            return;
        }
        let rp = realpath_memo(memo, cwd);
        let cur = det.get(&rp);
        if cur.map(|c| mtime > c.0).unwrap_or(true) {
            det.insert(rp, (mtime, agent.to_string(), path.to_string()));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        ctx: &mut HashMap<String, (u64, f64)>,
        det: &mut HashMap<String, (f64, String, String)>,
        memo: &mut HashMap<String, String>,
        cwd: &str,
        size: u64,
        mtime: f64,
        agent: &str,
        path: &str,
    ) {
        if cwd.is_empty() {
            return;
        }
        let rp = realpath_memo(memo, cwd);
        let cur = ctx.get(&rp);
        if cur.map(|c| mtime > c.1).unwrap_or(true) {
            ctx.insert(rp, (size, mtime));
        }
        mark(det, memo, cwd, mtime, agent, path);
    }

    for r in claude_sessions(paths, cache) {
        push(&mut ctx, &mut det, &mut memo, &r.cwd, r.size, r.mtime, "claude", &r.path.to_string_lossy());
    }

    fn dir_size_one_level(d: &Path) -> u64 {
        let mut total = 0u64;
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                if let Ok(md) = std::fs::symlink_metadata(e.path()) {
                    total += md.len();
                }
            }
        }
        total
    }

    // project rows = non-hidden subdirectories, mtime desc
    let mut rows: Vec<(f64, String, String)> = Vec::new();
    if root.is_dir() {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                let Some(name) = p.file_name().and_then(|n| n.to_str()).map(String::from) else {
                    continue;
                };
                if name.starts_with('.') || !p.is_dir() {
                    continue;
                }
                let mt = std::fs::metadata(&p).map(|m| mtime_f(&m)).unwrap_or(0.0);
                rows.push((mt, p.to_string_lossy().into_owned(), name));
            }
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    rows.into_iter()
        .map(|(mt, p, name)| {
            let rp = realpath_memo(&mut memo, &p);
            let tup = ctx.get(&rp).or_else(|| ctx.get(&p));
            let dt = det.get(&rp).or_else(|| det.get(&p));
            ProjectRow {
                dir_size: dir_size_one_level(Path::new(&p)),
                ctx_size: tup.map(|t| t.0),
                det_agent: dt.map(|d| d.1.clone()),
                det_path: dt.map(|d| d.2.clone()).unwrap_or_default(),
                path: p,
                name,
                mtime: mt,
            }
        })
        .collect()
}

// ---- purge ----

/// Purge the Claude store for `target` (an absolute, realpath'd cwd).
/// Returns `(label, count)` pairs for stores that had hits (only Claude now).
pub fn purge(paths: &Paths, cache: &mut CwdCache, target: &str) -> Vec<(String, u64)> {
    let mut report = Vec::new();
    if !target.starts_with('/') {
        return report; // safety: only absolute cwds, same contract as the CLI
    }
    let mut n = 0u64;
    for r in claude_sessions(paths, cache) {
        if r.cwd == target && std::fs::remove_file(&r.path).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        report.push(("Claude".to_string(), n));
    }
    report
}

#[cfg(test)]
mod id_exists_tests {
    use super::*;

    #[test]
    fn claude_id_found_by_filename_scan() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let proj = paths.claude_root().join("-Volumes-SSD-project-x");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("live-id.jsonl"), "{}\n").unwrap();
        assert_eq!(id_exists(&paths, "claude", "live-id"), Some(true));
        assert_eq!(id_exists(&paths, "claude", "gone-id"), Some(false), "被 GC 的 id 要报 false");
        // 非 claude（终端）没有可验证的存储
        assert_eq!(id_exists(&paths, "shell", "whatever"), None);
        // 别让奇怪的 id 变成路径穿越
        assert_eq!(id_exists(&paths, "claude", "../../etc/passwd"), Some(false));
        assert_eq!(id_exists(&paths, "claude", ""), Some(false));
    }
}
