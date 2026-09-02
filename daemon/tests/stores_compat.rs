//! Disk-format compatibility tests for the Claude Code store (the only agent
//! store AAA reads since 2026-09-03): fixtures are built in a tempdir laid out
//! like ~/.claude/projects, then find / detect / collect / purge run against them.
//! Nothing here touches the real HOME.

use std::path::Path;

use aaa_daemon::cache::CwdCache;
use aaa_daemon::paths::Paths;
use aaa_daemon::stores;

fn set_mtime(p: &Path, epoch: i64) {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
    let t = libc::timeval { tv_sec: epoch, tv_usec: 0 };
    let times = [t, t];
    assert_eq!(unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) }, 0);
}

struct Fix {
    _dir: tempfile::TempDir,
    paths: Paths,
    cache_path: std::path::PathBuf,
    /// canonicalized project cwd used as session target
    cwd: String,
    /// a second, unrelated cwd that must never be touched
    other: String,
}

fn fixture() -> Fix {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let proj = dir.path().join("proj");
    let other = dir.path().join("other");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let cwd = std::fs::canonicalize(&proj).unwrap().to_string_lossy().into_owned();
    let other = std::fs::canonicalize(&other).unwrap().to_string_lossy().into_owned();
    let paths = Paths::new(&home);
    let cache_path = paths.cwd_cache();
    Fix { _dir: dir, paths, cache_path, cwd, other }
}

fn write(p: &Path, body: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

impl Fix {
    fn claude_session(&self, proj_dir: &str, sid: &str, cwd: &str, mtime: i64) -> std::path::PathBuf {
        let f = self.paths.claude_root().join(proj_dir).join(format!("{sid}.jsonl"));
        write(
            &f,
            &format!(
                "{}\n{}\n",
                serde_json::json!({"type":"summary","summary":"irrelevant"}),
                serde_json::json!({"type":"user","cwd":cwd,"message":{"content":"hi"}}),
            ),
        );
        set_mtime(&f, mtime);
        f
    }
}

// ---------- find ----------

#[test]
fn find_claude_newest_by_mtime() {
    let fx = fixture();
    fx.claude_session("p1", "sid-old", &fx.cwd, 1_000_000);
    fx.claude_session("p1", "sid-new", &fx.cwd, 2_000_000);
    fx.claude_session("p2", "sid-elsewhere", &fx.other, 3_000_000);
    let mut cache = CwdCache::load(&fx.cache_path);
    assert_eq!(stores::find(&fx.paths, &mut cache, "claude", &fx.cwd), "sid-new");
    assert_eq!(stores::find(&fx.paths, &mut cache, "claude", "/nope"), "");
    // cache write-back uses aaa-compatible `claude:<path>` keys
    cache.save();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fx.cache_path).unwrap()).unwrap();
    let keys: Vec<&String> = raw.as_object().unwrap().keys().collect();
    assert!(keys.iter().all(|k| k.starts_with("claude:")));
    assert_eq!(keys.len(), 3);
}

#[test]
fn find_honours_preexisting_cli_cache() {
    let fx = fixture();
    let f = fx.claude_session("p1", "sid-x", &fx.cwd, 1_000_000);
    // an aaa-CLI-written cache entry claims a different cwd: it must win
    // (proves we read the shared cache rather than re-scanning)
    std::fs::create_dir_all(fx.cache_path.parent().unwrap()).unwrap();
    std::fs::write(
        &fx.cache_path,
        serde_json::json!({format!("claude:{}", f.display()): "/somewhere/else"}).to_string(),
    )
    .unwrap();
    let mut cache = CwdCache::load(&fx.cache_path);
    assert_eq!(stores::find(&fx.paths, &mut cache, "claude", &fx.cwd), "");
    assert_eq!(
        stores::find(&fx.paths, &mut cache, "claude", "/somewhere/else"),
        "sid-x"
    );
}

// ---------- detect ----------

#[test]
fn detect_reports_claude_or_nothing() {
    let fx = fixture();
    fx.claude_session("p1", "c1", &fx.cwd, 1_000_000);
    let mut cache = CwdCache::load(&fx.cache_path);
    assert_eq!(stores::detect(&fx.paths, &mut cache, &fx.cwd), "claude");
    assert_eq!(stores::detect(&fx.paths, &mut cache, &fx.other), "");
    // 其它 agent 的存储不再被识别
    assert_eq!(stores::find(&fx.paths, &mut cache, "codex", &fx.cwd), "");
    assert_eq!(stores::find(&fx.paths, &mut cache, "shell", &fx.cwd), "");
}

// ---------- collect ----------

#[test]
fn collect_lists_projects_with_ctx_and_agent() {
    let fx = fixture();
    let root = fx.paths.home.join("projroot");
    let p_a = root.join("alpha");
    let p_b = root.join("beta");
    let p_hidden = root.join(".hidden");
    std::fs::create_dir_all(&p_a).unwrap();
    std::fs::create_dir_all(&p_b).unwrap();
    std::fs::create_dir_all(&p_hidden).unwrap();
    std::fs::write(p_a.join("file.txt"), vec![b'x'; 1000]).unwrap();
    let a_cwd = std::fs::canonicalize(&p_a).unwrap().to_string_lossy().into_owned();
    fx.claude_session("p1", "sid-a", &a_cwd, 2_000_000);
    set_mtime(&p_a, 4_000_000);
    set_mtime(&p_b, 3_000_000);
    let mut cache = CwdCache::load(&fx.cache_path);
    let rows = stores::collect(&fx.paths, &mut cache, &root);
    assert_eq!(rows.len(), 2, "hidden dirs excluded");
    assert_eq!(rows[0].name, "alpha", "sorted by mtime desc");
    assert_eq!(rows[1].name, "beta");
    assert_eq!(rows[0].det_agent.as_deref(), Some("claude"));
    assert!(rows[0].ctx_size.unwrap() > 0);
    assert!(rows[0].dir_size >= 1000);
    assert_eq!(rows[1].det_agent, None);
    assert_eq!(rows[1].ctx_size, None);
}

// ---------- purge ----------

#[test]
fn purge_removes_claude_sessions_of_the_target_only() {
    let fx = fixture();
    fx.claude_session("p1", "c1", &fx.cwd, 1_000_000);
    fx.claude_session("p1", "c2", &fx.cwd, 1_100_000);
    let keep_claude = fx.claude_session("p1", "keep", &fx.other, 1_000_000);

    let mut cache = CwdCache::load(&fx.cache_path);
    let report = stores::purge(&fx.paths, &mut cache, &fx.cwd);
    assert_eq!(report, vec![("Claude".to_string(), 2)]);
    assert!(keep_claude.exists(), "unrelated cwd untouched");

    // second purge: nothing left to report
    assert!(stores::purge(&fx.paths, &mut cache, &fx.cwd).is_empty());
    // non-absolute target refused outright
    assert!(stores::purge(&fx.paths, &mut cache, "relative/path").is_empty());
}
