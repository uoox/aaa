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
        "reasonix" => Some(scan(paths.rnx_root(), Some("sessions"))),
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

pub fn codex_sessions(paths: &Paths, cache: &mut CwdCache) -> Vec<SessRec> {
    let mut out = Vec::new();
    for y in visible_dirs(&paths.codex_root()) {
        for m in visible_dirs(&y) {
            for d in visible_dirs(&m) {
                for fnm in visible_files(&d) {
                    let Some(name) = fnm.file_name().and_then(|n| n.to_str()) else { continue };
                    if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
                        continue;
                    }
                    let key = format!("codex:{}", fnm.display());
                    let (mut sid, mut cwd) = (String::new(), String::new());
                    match cache.get(&key) {
                        Some(Value::Array(a)) if a.len() == 2 => {
                            sid = val_str(&a[0]);
                            cwd = val_str(&a[1]);
                        }
                        Some(_) => {}
                        None => {
                            for o in jsonl_head(&fnm, 5) {
                                if o.get("type").and_then(|t| t.as_str()) == Some("session_meta") {
                                    let p = o.get("payload").cloned().unwrap_or(Value::Null);
                                    sid = val_str(p.get("id").unwrap_or(&Value::Null));
                                    cwd = val_str(p.get("cwd").unwrap_or(&Value::Null));
                                    break;
                                }
                            }
                            if !sid.is_empty() && !cwd.is_empty() {
                                cache.put(key, serde_json::json!([sid, cwd]));
                            }
                        }
                    }
                    if sid.is_empty() || cwd.is_empty() {
                        continue;
                    }
                    let (size, mtime) = fstat(&fnm);
                    out.push(SessRec { path: fnm, sid, cwd, size, mtime });
                }
            }
        }
    }
    out
}

pub fn pi_sessions(paths: &Paths, cache: &mut CwdCache) -> Vec<SessRec> {
    let mut out = Vec::new();
    for d in visible_dirs(&paths.pi_root()) {
        for fnm in visible_files(&d) {
            if fnm.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let key = format!("pi:{}", fnm.display());
            let (mut sid, mut cwd) = (String::new(), String::new());
            match cache.get(&key) {
                Some(Value::Array(a)) if a.len() == 2 => {
                    sid = val_str(&a[0]);
                    cwd = val_str(&a[1]);
                }
                Some(_) => {}
                None => {
                    for o in jsonl_head(&fnm, 5) {
                        if o.get("type").and_then(|t| t.as_str()) == Some("session") {
                            sid = val_str(o.get("id").unwrap_or(&Value::Null));
                            cwd = val_str(o.get("cwd").unwrap_or(&Value::Null));
                            break;
                        }
                    }
                    if !sid.is_empty() && !cwd.is_empty() {
                        cache.put(key, serde_json::json!([sid, cwd]));
                    }
                }
            }
            if sid.is_empty() || cwd.is_empty() {
                continue;
            }
            let (size, mtime) = fstat(&fnm);
            out.push(SessRec { path: fnm, sid, cwd, size, mtime });
        }
    }
    out
}

/// grok: one dir per url-encoded cwd; each subdir is a session.
pub fn grok_sessions(paths: &Paths) -> Vec<SessRec> {
    let mut out = Vec::new();
    let root = paths.grok_root();
    let Ok(rd) = std::fs::read_dir(&root) else { return out };
    for entry in rd.flatten() {
        let d = entry.path();
        if !d.is_dir() {
            continue;
        }
        let Some(enc) = d.file_name().and_then(|n| n.to_str()).map(String::from) else { continue };
        let cwd = percent_encoding::percent_decode_str(&enc)
            .decode_utf8_lossy()
            .into_owned();
        if !cwd.starts_with('/') {
            continue;
        }
        let Ok(rd2) = std::fs::read_dir(&d) else { continue };
        for se in rd2.flatten() {
            let sdir = se.path();
            if !sdir.is_dir() {
                continue; // skip loose files like prompt_history.jsonl
            }
            let sid = sdir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let (mut size, mut mtime) = (0u64, 0f64);
            if let Ok(rd3) = std::fs::read_dir(&sdir) {
                for fe in rd3.flatten() {
                    if let Ok(md) = fe.metadata() {
                        size += md.len();
                        let mt = mtime_f(&md);
                        if mt > mtime {
                            mtime = mt;
                        }
                    }
                }
            }
            out.push(SessRec { path: sdir, sid, cwd: cwd.clone(), size, mtime });
        }
    }
    out
}

// ---- reasonix (forward lookup only, cwd -> dir name) ----

pub fn rnx_dir(paths: &Paths, cwd: &str) -> PathBuf {
    paths.rnx_root().join(cwd.replace('/', "-"))
}

pub struct RnxStat {
    pub sid: String,
    pub meta: String,
    pub size: u64,
    pub mtime: f64,
}

pub fn rnx_stat(paths: &Paths, cwd: &str) -> RnxStat {
    let mut cands = vec![cwd.to_string()];
    let rp = realpath(cwd);
    if rp != cwd {
        cands.push(rp);
    }
    let mut st = RnxStat { sid: String::new(), meta: String::new(), size: 0, mtime: 0.0 };
    for c in &cands {
        let sess_dir = rnx_dir(paths, c).join("sessions");
        for fnm in visible_files(&sess_dir) {
            let Some(name) = fnm.file_name().and_then(|n| n.to_str()) else { continue };
            if !name.ends_with(".jsonl") || name.ends_with(".events.jsonl") {
                continue;
            }
            let (sz, mt) = fstat(&fnm);
            st.size += sz;
            if mt > st.mtime {
                st.mtime = mt;
                st.sid = name.trim_end_matches(".jsonl").to_string();
                let meta = format!("{}.meta", fnm.display());
                st.meta = if Path::new(&meta).exists() { meta } else { String::new() };
            }
        }
    }
    st
}

// ---- agy ----

fn load_json_obj(p: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn agy_find(paths: &Paths, target: &str) -> String {
    let cache = paths.agy_root().join("cache").join("last_conversations.json");
    let d = load_json_obj(&cache);
    if d.is_empty() {
        return String::new();
    }
    let mut cid = d.get(target).map(val_str).unwrap_or_default();
    if cid.is_empty() {
        // case-insensitive fallback (macOS filesystems)
        let low = target.to_lowercase();
        for (k, v) in &d {
            if k.to_lowercase() == low {
                cid = val_str(v);
                break;
            }
        }
    }
    if cid.is_empty() {
        return String::new();
    }
    let conv = paths.agy_root().join("conversations");
    for ext in [".db", ".pb"] {
        if conv.join(format!("{cid}{ext}")).exists() {
            return cid;
        }
    }
    String::new()
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
        "reasonix" => rnx_stat(paths, target).sid,
        "claude" => newest(
            claude_sessions(paths, cache)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        "grok" => newest(
            grok_sessions(paths)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        "codex" => newest(
            codex_sessions(paths, cache)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        "pi" => newest(
            pi_sessions(paths, cache)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        "agy" => agy_find(paths, target),
        _ => String::new(),
    }
}

// ---- detect ----

/// Agent of the most recent session for `target`, across all stores. Part of
/// the ported AAA_PY surface (used by the store_debug example + parity tests).
pub fn detect(paths: &Paths, cache: &mut CwdCache, target: &str) -> String {
    let target = realpath(target);
    let mut best: (f64, String) = (0.0, String::new());
    let upd = |cwd: &str, mt: f64, agent: &str, best: &mut (f64, String)| {
        if cwd.is_empty() {
            return;
        }
        let rp = realpath(cwd);
        if rp == target && mt > best.0 {
            *best = (mt, agent.to_string());
        }
    };
    for r in claude_sessions(paths, cache) {
        upd(&r.cwd, r.mtime, "claude", &mut best);
    }
    for r in grok_sessions(paths) {
        upd(&r.cwd, r.mtime, "grok", &mut best);
    }
    for r in codex_sessions(paths, cache) {
        upd(&r.cwd, r.mtime, "codex", &mut best);
    }
    for r in pi_sessions(paths, cache) {
        upd(&r.cwd, r.mtime, "pi", &mut best);
    }
    let st = rnx_stat(paths, &target);
    if !st.sid.is_empty() {
        upd(&target, st.mtime, "reasonix", &mut best);
    }
    let cid = agy_find(paths, &target);
    if !cid.is_empty() {
        for ext in [".db", ".pb"] {
            let p = paths.agy_root().join("conversations").join(format!("{cid}{ext}"));
            if let Ok(md) = std::fs::metadata(&p) {
                upd(&target, mtime_f(&md), "agy", &mut best);
            }
        }
    }
    best.1
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
    for r in codex_sessions(paths, cache) {
        push(&mut ctx, &mut det, &mut memo, &r.cwd, r.size, r.mtime, "codex", &r.path.to_string_lossy());
    }
    for r in pi_sessions(paths, cache) {
        push(&mut ctx, &mut det, &mut memo, &r.cwd, r.size, r.mtime, "pi", &r.path.to_string_lossy());
    }
    // grok: aggregate per cwd first
    let mut agg: HashMap<String, (u64, f64)> = HashMap::new();
    for r in grok_sessions(paths) {
        if r.cwd.is_empty() {
            continue;
        }
        let e = agg.entry(r.cwd.clone()).or_insert((0, 0.0));
        e.0 += r.size;
        if r.mtime > e.1 {
            e.1 = r.mtime;
        }
    }
    for (cwd, (s, m)) in agg {
        push(&mut ctx, &mut det, &mut memo, &cwd, s, m, "grok", "");
    }
    // agy: agent detection only (no size info)
    let lc = load_json_obj(&paths.agy_root().join("cache").join("last_conversations.json"));
    for (ws, cid) in &lc {
        let Some(cid) = cid.as_str() else { continue };
        for ext in [".db", ".pb"] {
            let p = paths.agy_root().join("conversations").join(format!("{cid}{ext}"));
            if let Ok(md) = std::fs::metadata(&p) {
                mark(&mut det, &mut memo, ws, mtime_f(&md), "agy", "");
                break;
            }
        }
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

    // reasonix can only be looked up forward, per known project dir
    for (_, p, _) in &rows {
        let st = rnx_stat(paths, p);
        if !st.sid.is_empty() {
            push(&mut ctx, &mut det, &mut memo, p, st.size, st.mtime, "reasonix", &st.meta);
        }
    }

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

fn agy_purge(paths: &Paths, target: &str) -> u64 {
    let root = paths.agy_root();
    if !root.is_dir() {
        return 0;
    }
    let hist = root.join("history.jsonl");
    let conv_dir = root.join("conversations");
    let lc_path = root.join("cache").join("last_conversations.json");
    let pj_path = root.join("cache").join("projects.json");

    fn dump_json(p: &Path, d: &serde_json::Map<String, Value>) {
        let tmp = p.with_extension("tmp");
        if serde_json::to_string(&Value::Object(d.clone()))
            .ok()
            .and_then(|s| std::fs::write(&tmp, s).ok())
            .is_some()
        {
            let _ = std::fs::rename(&tmp, p);
        }
    }

    let mut ids_target: std::collections::HashSet<String> = Default::default();
    let mut ids_other: std::collections::HashSet<String> = Default::default();
    let mut kept: Vec<String> = Vec::new();
    let mut rewrite_hist = hist.is_file();
    if rewrite_hist {
        match std::fs::read(&hist) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                // preserve line structure incl. trailing newline handling:
                // python iterates lines (keeping \n); we rebuild with \n.
                for ln in text.split_inclusive('\n') {
                    let s = ln.trim();
                    if s.is_empty() {
                        kept.push(ln.to_string());
                        continue;
                    }
                    let Ok(o) = serde_json::from_str::<Value>(s) else {
                        kept.push(ln.to_string());
                        continue;
                    };
                    let cid = o.get("conversationId").and_then(|v| v.as_str()).unwrap_or("");
                    let ws = o.get("workspace").and_then(|v| v.as_str()).unwrap_or("");
                    if ws == target {
                        if !cid.is_empty() {
                            ids_target.insert(cid.to_string());
                        }
                    } else {
                        if !cid.is_empty() {
                            ids_other.insert(cid.to_string());
                        }
                        kept.push(ln.to_string());
                    }
                }
            }
            Err(_) => rewrite_hist = false,
        }
    }

    let mut lc = load_json_obj(&lc_path);
    let mut pj = load_json_obj(&pj_path);
    if let Some(cid) = lc.get(target).and_then(|v| v.as_str()) {
        if !cid.is_empty() {
            ids_target.insert(cid.to_string());
        }
    }

    let mut n = 0u64;
    for cid in ids_target.difference(&ids_other) {
        // SAFETY: conversation ids must be single path components.
        if cid.contains('/') || cid.contains("..") || cid.is_empty() {
            continue;
        }
        let mut removed = false;
        for ext in [".db", ".pb"] {
            let p = conv_dir.join(format!("{cid}{ext}"));
            if p.exists() && std::fs::remove_file(&p).is_ok() {
                removed = true;
            }
        }
        let bp = root.join("brain").join(cid);
        if bp.is_dir() && bp.starts_with(&root) && std::fs::remove_dir_all(&bp).is_ok() {
            removed = true;
        }
        for (sub, ext) in [("annotations", ".pbtxt"), ("implicit", ".pb")] {
            let p = root.join(sub).join(format!("{cid}{ext}"));
            if p.exists() {
                let _ = std::fs::remove_file(&p);
            }
        }
        if removed {
            n += 1;
        }
    }

    if lc.remove(target).is_some() {
        dump_json(&lc_path, &lc);
    }
    if pj.remove(target).is_some() {
        dump_json(&pj_path, &pj);
    }
    if rewrite_hist {
        let tmp = hist.with_extension("jsonl.tmp");
        if std::fs::write(&tmp, kept.concat()).is_ok() {
            let _ = std::fs::rename(&tmp, &hist);
        }
    }
    n
}

fn grok_purge(paths: &Paths, target: &str) -> u64 {
    let root = paths.grok_root();
    let mut n = 0u64;
    if root.is_dir() {
        if let Ok(rd) = std::fs::read_dir(&root) {
            for e in rd.flatten() {
                let d = e.path();
                if !d.is_dir() {
                    continue;
                }
                let Some(enc) = d.file_name().and_then(|x| x.to_str()) else { continue };
                let decoded = percent_encoding::percent_decode_str(enc)
                    .decode_utf8_lossy()
                    .into_owned();
                if decoded != target {
                    continue;
                }
                if let Ok(rd2) = std::fs::read_dir(&d) {
                    for se in rd2.flatten() {
                        if se.path().is_dir() {
                            n += 1;
                        }
                    }
                }
                // SAFETY: `d` is a direct child of grok_root by construction.
                debug_assert_eq!(d.parent(), Some(root.as_path()));
                let _ = std::fs::remove_dir_all(&d);
            }
        }
    }
    // sqlite index cleanup via the system sqlite3 CLI (python used sqlite3
    // module; we avoid a rusqlite dependency). Failure is silent, like AAA_PY.
    let db = root.join("session_search.sqlite");
    if db.is_file() {
        let escaped = target.replace('\'', "''");
        let sql = format!("DELETE FROM session_docs WHERE cwd = '{escaped}'; SELECT changes();");
        if let Ok(out) = std::process::Command::new("sqlite3")
            .arg(&db)
            .arg(&sql)
            .output()
        {
            if out.status.success() {
                if let Ok(m) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>() {
                    n = n.max(m);
                }
            }
        }
    }
    n
}

fn rnx_purge(paths: &Paths, target: &str) -> u64 {
    let mut n = 0u64;
    let mut cands = vec![target.to_string()];
    let rp = realpath(target);
    if rp != target {
        cands.push(rp);
    }
    let root = paths.rnx_root();
    for c in &cands {
        let d = rnx_dir(paths, c);
        if !d.is_dir() {
            continue;
        }
        // SAFETY: target is an absolute path so `/` -> `-` yields a single
        // path component; enforce that d is a direct child of rnx_root.
        if d.parent() != Some(root.as_path()) {
            continue;
        }
        for fnm in visible_files(&d.join("sessions")) {
            let Some(name) = fnm.file_name().and_then(|x| x.to_str()) else { continue };
            if name.ends_with(".jsonl") && !name.ends_with(".events.jsonl") {
                n += 1;
            }
        }
        let _ = std::fs::remove_dir_all(&d);
    }
    n
}

/// Purge all agent stores for `target` (an absolute, realpath'd cwd).
/// Returns `(label, count)` pairs for stores that had hits, in the same order
/// and with the same labels as AAA_PY `cmd_purge`.
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

    let mut n = 0u64;
    for r in codex_sessions(paths, cache) {
        if r.cwd == target && std::fs::remove_file(&r.path).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        report.push(("Codex".to_string(), n));
    }

    let mut n = 0u64;
    for r in pi_sessions(paths, cache) {
        if r.cwd == target && std::fs::remove_file(&r.path).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        report.push(("Pi".to_string(), n));
    }

    for (label, f) in [
        ("agy", agy_purge as fn(&Paths, &str) -> u64),
        ("Grok", grok_purge),
        ("Reasonix", rnx_purge),
    ] {
        let n = f(paths, target);
        if n > 0 {
            report.push((label.to_string(), n));
        }
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
        // 无法便宜验证的 agent 保持 best-effort
        assert_eq!(id_exists(&paths, "codex", "whatever"), None);
        assert_eq!(id_exists(&paths, "pi", "whatever"), None);
        // 别让奇怪的 id 变成路径穿越
        assert_eq!(id_exists(&paths, "claude", "../../etc/passwd"), Some(false));
        assert_eq!(id_exists(&paths, "claude", ""), Some(false));
    }
}
