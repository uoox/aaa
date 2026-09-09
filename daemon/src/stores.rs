//! 各 agent 会话存储的读写：find / collect / purge。claude 那部分是
//! `~/.local/bin/aaa` 里 AAA_PY 的逐行移植（cwd 缓存的键因此保持兼容）。
//!
//! Store layout:
//!   claude:   ~/.claude/projects/*/<sid>.jsonl          (cwd field in lines)
//!   agy:      ~/.gemini/antigravity-cli/cache/last_conversations.json (cwd -> id)
//!             + conversations/<id>.db                   (对话本体，SQLite)
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

/// 项目路径的**唯一**写法：能 canonicalize 就 canonicalize（解 `..`、软链接、`/var`
/// vs `/private/var`），目录已经不在了就至少去掉尾斜杠——`/p/a` 与 `/p/a/` 必须是同一个
/// 项目，否则收件箱、静音、未读这些以路径为 key 的东西会各存一份（v1.22 补齐尾
/// 斜杠这一半：以前 canonicalize 失败就原样返回，删掉的目录一带斜杠就分裂成两行）。
/// 根目录 `/` 不动。
pub fn realpath(p: &str) -> String {
    if let Ok(x) = std::fs::canonicalize(p) {
        return x.to_string_lossy().into_owned();
    }
    let t = p.trim_end_matches('/');
    if t.is_empty() { p.to_string() } else { t.to_string() }
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

/// 注册表第三列兜底 id 还在不在。目的：agent 那边把对话 GC 掉之后，别拿着坏 id
/// 反复 resume 失败。claude 的会话文件名就是 id，扫一层目录即可；agy 看对话文件在不在。
pub fn id_exists(paths: &Paths, agent: &str, id: &str) -> bool {
    if !safe_id(id) {
        return false;
    }
    match agent {
        "claude" => {
            let name = format!("{id}.jsonl");
            let Ok(rd) = std::fs::read_dir(paths.claude_root()) else { return false };
            rd.flatten().any(|e| e.path().join(&name).is_file())
        }
        "agy" => agy_conv_mtime(paths, id).is_some(),
        _ => false,
    }
}

/// 会话 id 只当一个文件名用。**白名单**，不是黑名单：两边的 id 都是 uuid 形状，
/// 而 `purge` 会拿它去 `remove_dir_all(brain/<id>)`——放过一个 `.` 就等于删掉整个
/// `brain/`（`join(".")` 解析成父目录本身）。存储里出现坏 id 就当没有，不冒这个险。
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
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

// ---- agy (Antigravity CLI) ----
//
// 与 claude 完全不同的形状：`cache/last_conversations.json` 本身就是
// `cwd -> 最近对话 id` 的现成映射（每个 cwd 只留最后一条），所以没有
// 「扫一遍所有会话」这一步。对话本体（`conversations/<id>.db`）是 SQLite 读不动，
// 但消息流不看它——看的是 `brain/<id>/.system_generated/logs/transcript.jsonl`
// （见 [`agy_transcript`]）。agy 仍不出标题：SQLite 里那份摘要读不出来。

fn agy_map(paths: &Paths) -> serde_json::Map<String, Value> {
    let p = paths.agy_root().join("cache").join("last_conversations.json");
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// 表里对应 `target` 的键。`target` 是我们 realpath 过的，表里的是 agy 自己记下的
/// cwd——同一个目录可能写成软链接路径（`/tmp` vs `/private/tmp`），也可能只差大小写
/// （macOS 文件系统大小写不敏感）。三步：原样 → 解到同一个真实路径 → 大小写无关。
fn agy_key(map: &serde_json::Map<String, Value>, target: &str) -> Option<String> {
    if map.contains_key(target) {
        return Some(target.to_string());
    }
    map.keys()
        .find(|k| realpath(k) == target)
        .or_else(|| map.keys().find(|k| k.eq_ignore_ascii_case(target)))
        .cloned()
}

/// 对话本体的 mtime；`.db`（当前）与 `.pb`（旧版）都认，都不在 = 已被 GC。
fn agy_conv_mtime(paths: &Paths, id: &str) -> Option<f64> {
    if !safe_id(id) {
        return None;
    }
    let dir = paths.agy_root().join("conversations");
    [".db", ".pb"].iter().find_map(|ext| {
        std::fs::metadata(dir.join(format!("{id}{ext}")))
            .ok()
            .map(|m| mtime_f(&m))
    })
}

/// 这个对话的 transcript（v1.30 消息流）：`brain/<id>/.system_generated/logs/transcript.jsonl`。
/// 一行一步、只追加，daemon 按偏移量尾随。文件不在（老对话 / 还没写第一步）→ None。
pub fn agy_transcript(paths: &Paths, id: &str) -> Option<std::path::PathBuf> {
    if !safe_id(id) {
        return None;
    }
    let p = paths
        .agy_root()
        .join("brain")
        .join(id)
        .join(".system_generated")
        .join("logs")
        .join("transcript.jsonl");
    p.is_file().then_some(p)
}

/// 这个 cwd 最近一次 agy 对话的 id（表里指着已被 GC 的对话时是空串）。
pub fn agy_find(paths: &Paths, target: &str) -> String {
    let map = agy_map(paths);
    let id = agy_key(&map, target)
        .and_then(|k| map.get(&k).and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_default();
    // 表里可能指着一个已经被删掉的对话：那就别 resume，回落开新的
    if agy_conv_mtime(paths, &id).is_some() { id } else { String::new() }
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

/// `cwd` 下该 agent 最近一次会话的 id（找不到 = 空串，调用方回落开新会话）。
/// `target` 由调用方 realpath 过。
pub fn find(paths: &Paths, cache: &mut CwdCache, target: &str, agent: &str) -> String {
    match agent {
        "claude" => newest(
            claude_sessions(paths, cache)
                .into_iter()
                .filter(|r| r.cwd == target)
                .map(|r| (r.mtime, r.sid)),
        ),
        "agy" => agy_find(paths, target),
        _ => String::new(),
    }
}

// ---- collect ----

#[derive(Clone, Debug)]
pub struct ProjectRow {
    pub path: String,
    pub name: String,
    pub mtime: f64,
    pub dir_size: u64,
    /// agent of the most recent session found in any store (None = none found)
    pub det_agent: Option<String>,
    /// session file path usable by the namer (claude jsonl / reasonix .meta)
    pub det_path: String,
}

pub fn collect(paths: &Paths, cache: &mut CwdCache, root: &Path) -> Vec<ProjectRow> {
    use std::collections::HashMap;
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

    for r in claude_sessions(paths, cache) {
        mark(&mut det, &mut memo, &r.cwd, r.mtime, "claude", &r.path.to_string_lossy());
    }
    // agy 没有可读的 transcript，`det_path` 留空——标题回退链因此跳过「从存储里
    // 读上次对话名」这一档，直接落到目录名
    for (cwd, id) in agy_map(paths) {
        if let Some(mt) = id.as_str().and_then(|id| agy_conv_mtime(paths, id)) {
            mark(&mut det, &mut memo, &cwd, mt, "agy", "");
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

    rows.into_iter()
        .map(|(mt, p, name)| {
            let rp = realpath_memo(&mut memo, &p);
            let dt = det.get(&rp).or_else(|| det.get(&p));
            ProjectRow {
                dir_size: dir_size_one_level(Path::new(&p)),
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

/// 清掉各 agent 存储里属于 `target`（绝对、已 realpath 的 cwd）的会话；
/// 返回 `[(agent 标签, 删掉几个)]`，只列真删掉了东西的那些——线上的
/// `purged: [{agent_label, count}]` 就是它（PROTOCOL.md「删除项目」）。
pub fn purge(paths: &Paths, cache: &mut CwdCache, target: &str) -> Vec<(&'static str, u64)> {
    if !target.starts_with('/') {
        return vec![]; // safety: only absolute cwds, same contract as the CLI
    }
    let mut out = Vec::new();
    let mut n = 0u64;
    for r in claude_sessions(paths, cache) {
        if r.cwd == target && std::fs::remove_file(&r.path).is_ok() {
            n += 1;
        }
    }
    if n > 0 {
        out.push(("Claude", n));
    }
    if agy_purge(paths, target) {
        out.push(("Antigravity", 1));
    }
    out
}

/// agy 侧的删除。**非删不可的是 `cache/last_conversations.json` 里这个 cwd 的
/// 条目**：那张表按 cwd 记，留着的话同名目录重建之后第一次 resume 会接到上一个
/// 项目的对话上。对话本体（`conversations/<id>.db` 与 brain / annotations /
/// implicit 三处衍生文件）只在没有别的 cwd 也指着它时才删。
///
/// `history.jsonl`（上翻箭头用的提示词历史）不动：agy 正开着的时候我们重写它会
/// 丢行，而它不影响 resume。
fn agy_purge(paths: &Paths, target: &str) -> bool {
    let root = paths.agy_root();
    let mut map = agy_map(paths);
    let Some(key) = agy_key(&map, target) else { return false };
    let id = map
        .remove(&key)
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    // 还被别的目录指着的对话，本体不能删——先问，再把表写回去（`map` 就地交出去）
    let shared = map.values().any(|v| v.as_str() == Some(id.as_str()));
    let lc = root.join("cache").join("last_conversations.json");
    if let Ok(body) = serde_json::to_vec(&Value::Object(map)) {
        let _ = crate::paths::write_atomic(&lc, &body);
    }
    if !safe_id(&id) || shared {
        return true; // 条目摘掉了就算删过
    }
    // `-wal` / `-shm`：agy 正常退出会 checkpoint 掉，崩了就会留下，留着是纯垃圾
    for ext in [".db", ".db-wal", ".db-shm", ".pb"] {
        let _ = std::fs::remove_file(root.join("conversations").join(format!("{id}{ext}")));
    }
    let _ = std::fs::remove_dir_all(root.join("brain").join(&id));
    let _ = std::fs::remove_file(root.join("annotations").join(format!("{id}.pbtxt")));
    let _ = std::fs::remove_file(root.join("implicit").join(format!("{id}.pb")));
    true
}

#[cfg(test)]
mod id_exists_tests {
    use super::*;

    /// `/p/a` 与 `/p/a/` 必须是同一个项目：路径是所有本地状态（静音 / 未读 / 收件箱）
    /// 的 key，分裂成两个的后果是黄点消不掉。
    #[test]
    fn realpath_trims_the_trailing_slash_when_the_dir_is_gone() {
        assert_eq!(realpath("/nope/gone/"), "/nope/gone");
        assert_eq!(realpath("/nope/gone"), "/nope/gone");
        assert_eq!(realpath("/"), "/", "根目录不动");
        assert_eq!(realpath(""), "");
    }

    #[test]
    fn claude_id_found_by_filename_scan() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let proj = paths.claude_root().join("-Volumes-SSD-project-x");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("live-id.jsonl"), "{}\n").unwrap();
        assert!(id_exists(&paths, "claude", "live-id"));
        assert!(!id_exists(&paths, "claude", "gone-id"), "被 GC 的 id 要报 false");
        // 终端没有可验证的存储
        assert!(!id_exists(&paths, "shell", "live-id"));
        // 别让奇怪的 id 变成路径：`.` 尤其致命，purge 会拿它去 remove_dir_all(brain/<id>)
        for bad in ["../../etc/passwd", "", ".", "..", ".hidden", "a/b"] {
            assert!(!id_exists(&paths, "claude", bad), "{bad} 不该被当成 id");
            assert!(!id_exists(&paths, "agy", bad), "{bad} 不该被当成 id");
        }
    }

    /// agy 的三件事一起验：查得到、指向已删对话时不 resume、删项目之后 cwd 条目
    /// 必须消失（不消失的话同名目录重建会接到上一个项目的对话上）。
    #[test]
    fn agy_find_and_purge() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let root = paths.agy_root();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        std::fs::write(root.join("conversations").join("cid-1.db"), b"x").unwrap();
        std::fs::write(
            root.join("cache").join("last_conversations.json"),
            r#"{"/p/a":"cid-1","/p/b":"cid-gone"}"#,
        )
        .unwrap();
        let mut cache = CwdCache::load(&paths.cwd_cache());

        assert_eq!(find(&paths, &mut cache, "/p/a", "agy"), "cid-1");
        assert_eq!(find(&paths, &mut cache, "/p/b", "agy"), "", "对话文件没了就别 resume");
        assert_eq!(find(&paths, &mut cache, "/p/a", "claude"), "", "别拿 agy 的 id 喂 claude");
        assert!(id_exists(&paths, "agy", "cid-1") && !id_exists(&paths, "agy", "cid-gone"));

        assert_eq!(purge(&paths, &mut cache, "/p/a"), vec![("Antigravity", 1)]);
        assert!(!root.join("conversations").join("cid-1.db").exists());
        assert_eq!(find(&paths, &mut cache, "/p/a", "agy"), "");
        let left = std::fs::read_to_string(root.join("cache").join("last_conversations.json")).unwrap();
        assert!(!left.contains("/p/a") && left.contains("/p/b"), "只摘自己那一条：{left}");
    }

    /// 存储里出现坏 id（手改花了 / 别的程序写坏了）时，purge 只许摘掉那条 cwd 记录，
    /// 绝不能顺着 id 去删目录——`brain/.` 就是 `brain/` 本身。
    #[test]
    fn agy_purge_never_follows_a_bad_id() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let root = paths.agy_root();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::create_dir_all(root.join("brain").join("someone-elses")).unwrap();
        std::fs::write(
            root.join("cache").join("last_conversations.json"),
            r#"{"/p/a":"."}"#,
        )
        .unwrap();
        let mut cache = CwdCache::load(&paths.cwd_cache());
        purge(&paths, &mut cache, "/p/a");
        assert!(root.join("brain").join("someone-elses").is_dir(), "brain/ 不能被删空");
        let left = std::fs::read_to_string(root.join("cache").join("last_conversations.json")).unwrap();
        assert!(!left.contains("/p/a"), "那条 cwd 记录还是要摘掉：{left}");
    }

    /// 同一个对话被两个 cwd 指着时，删掉其中一个项目不能删对话本体。
    #[test]
    fn agy_purge_keeps_a_conversation_another_dir_still_points_at() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let root = paths.agy_root();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::create_dir_all(root.join("conversations")).unwrap();
        std::fs::write(root.join("conversations").join("shared.db"), b"x").unwrap();
        std::fs::write(
            root.join("cache").join("last_conversations.json"),
            r#"{"/p/a":"shared","/p/b":"shared"}"#,
        )
        .unwrap();
        let mut cache = CwdCache::load(&paths.cwd_cache());
        purge(&paths, &mut cache, "/p/a");
        assert!(root.join("conversations").join("shared.db").exists());
        assert_eq!(find(&paths, &mut cache, "/p/b", "agy"), "shared");
    }
}
