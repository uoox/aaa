//! Disk-format compatibility tests for the AAA_PY port: fixtures are built in
//! a tempdir laid out exactly like the aaa CLI stores (layouts documented in
//! ~/.local/bin/aaa), then find / detect / collect / purge run against them.
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

    fn codex_session(&self, ymd: (&str, &str, &str), name: &str, sid: &str, cwd: &str, mtime: i64) -> std::path::PathBuf {
        let f = self
            .paths
            .codex_root()
            .join(ymd.0)
            .join(ymd.1)
            .join(ymd.2)
            .join(format!("rollout-{name}.jsonl"));
        write(
            &f,
            &format!(
                "{}\n{}\n",
                serde_json::json!({"type":"session_meta","payload":{"id":sid,"cwd":cwd}}),
                serde_json::json!({"type":"turn","payload":{}}),
            ),
        );
        set_mtime(&f, mtime);
        f
    }

    fn pi_session(&self, sub: &str, name: &str, sid: &str, cwd: &str, mtime: i64) -> std::path::PathBuf {
        let f = self.paths.pi_root().join(sub).join(format!("{name}.jsonl"));
        write(
            &f,
            &format!("{}\n", serde_json::json!({"type":"session","id":sid,"cwd":cwd})),
        );
        set_mtime(&f, mtime);
        f
    }

    fn rnx_session(&self, cwd: &str, sid: &str, preview: &str, mtime: i64) -> std::path::PathBuf {
        let d = self.paths.rnx_root().join(cwd.replace('/', "-")).join("sessions");
        let f = d.join(format!("{sid}.jsonl"));
        write(&f, "{}\n");
        write(&d.join(format!("{sid}.jsonl.meta")), &serde_json::json!({"preview":preview}).to_string());
        // sidecar events file must not be counted as a session
        write(&d.join(format!("{sid}.events.jsonl")), "{}\n");
        set_mtime(&f, mtime);
        f
    }

    fn grok_session(&self, cwd: &str, sid: &str) -> std::path::PathBuf {
        let enc: String = cwd
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        let d = self.paths.grok_root().join(enc).join(sid);
        write(&d.join("session.json"), "{}");
        d
    }

    fn agy_store(&self, cwd: &str, cid: &str, shared_cid: &str, other_cwd: &str) {
        let root = self.paths.agy_root();
        write(
            &root.join("cache").join("last_conversations.json"),
            &serde_json::json!({cwd: cid, other_cwd: shared_cid}).to_string(),
        );
        write(
            &root.join("history.jsonl"),
            &format!(
                "{}\n{}\n{}\n",
                serde_json::json!({"workspace":cwd,"conversationId":cid}),
                serde_json::json!({"workspace":cwd,"conversationId":shared_cid}),
                serde_json::json!({"workspace":other_cwd,"conversationId":shared_cid}),
            ),
        );
        for c in [cid, shared_cid] {
            write(&root.join("conversations").join(format!("{c}.db")), "db");
            write(&root.join("brain").join(c).join("state.pb"), "pb");
            write(&root.join("annotations").join(format!("{c}.pbtxt")), "a");
            write(&root.join("implicit").join(format!("{c}.pb")), "i");
        }
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

#[test]
fn find_codex_and_pi_and_reasonix_and_agy() {
    let fx = fixture();
    fx.codex_session(("2026", "08", "29"), "a", "codex-old", &fx.cwd, 1_000_000);
    fx.codex_session(("2026", "08", "30"), "b", "codex-new", &fx.cwd, 2_000_000);
    fx.pi_session("s1", "t1", "pi-old", &fx.cwd, 1_000_000);
    fx.pi_session("s1", "t2", "pi-new", &fx.cwd, 2_000_000);
    fx.rnx_session(&fx.cwd, "rnx-sid", "preview text", 1_500_000);
    fx.agy_store(&fx.cwd, "agy-cid", "agy-shared", &fx.other);
    let mut cache = CwdCache::load(&fx.cache_path);
    assert_eq!(stores::find(&fx.paths, &mut cache, "codex", &fx.cwd), "codex-new");
    assert_eq!(stores::find(&fx.paths, &mut cache, "pi", &fx.cwd), "pi-new");
    assert_eq!(stores::find(&fx.paths, &mut cache, "reasonix", &fx.cwd), "rnx-sid");
    assert_eq!(stores::find(&fx.paths, &mut cache, "agy", &fx.cwd), "agy-cid");
    // agy falls back to case-insensitive match (macOS)
    assert_eq!(
        stores::find(&fx.paths, &mut cache, "agy", &fx.cwd.to_uppercase()),
        "agy-cid"
    );
    assert_eq!(stores::find(&fx.paths, &mut cache, "shell", &fx.cwd), "");
}

// ---------- detect ----------

#[test]
fn detect_picks_most_recent_agent() {
    let fx = fixture();
    fx.claude_session("p1", "c1", &fx.cwd, 1_000_000);
    fx.codex_session(("2026", "08", "30"), "b", "x1", &fx.cwd, 5_000_000);
    fx.pi_session("s1", "t1", "p1", &fx.cwd, 2_000_000);
    let mut cache = CwdCache::load(&fx.cache_path);
    assert_eq!(stores::detect(&fx.paths, &mut cache, &fx.cwd), "codex");
    assert_eq!(stores::detect(&fx.paths, &mut cache, &fx.other), "");
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
fn purge_all_six_stores_reports_counts_and_spares_others() {
    let fx = fixture();
    // target cwd sessions
    fx.claude_session("p1", "c1", &fx.cwd, 1_000_000);
    fx.claude_session("p1", "c2", &fx.cwd, 1_100_000);
    fx.codex_session(("2026", "08", "30"), "a", "x1", &fx.cwd, 1_000_000);
    fx.pi_session("s1", "t1", "pi1", &fx.cwd, 1_000_000);
    fx.rnx_session(&fx.cwd, "r1", "p", 1_000_000);
    fx.rnx_session(&fx.cwd, "r2", "p", 1_100_000);
    let grok_dir = fx.grok_session(&fx.cwd, "g1");
    fx.agy_store(&fx.cwd, "agy-cid", "agy-shared", &fx.other);
    // other cwd sessions that must survive
    let keep_claude = fx.claude_session("p1", "keep", &fx.other, 1_000_000);
    let keep_codex = fx.codex_session(("2026", "08", "30"), "k", "kx", &fx.other, 1_000_000);
    let keep_pi = fx.pi_session("s1", "keep", "kp", &fx.other, 1_000_000);
    let keep_grok = fx.grok_session(&fx.other, "gk");

    let mut cache = CwdCache::load(&fx.cache_path);
    let report = stores::purge(&fx.paths, &mut cache, &fx.cwd);
    let as_map: std::collections::HashMap<String, u64> = report.iter().cloned().collect();
    assert_eq!(as_map["Claude"], 2);
    assert_eq!(as_map["Codex"], 1);
    assert_eq!(as_map["Pi"], 1);
    assert_eq!(as_map["agy"], 1, "only the non-shared conversation is removed");
    assert_eq!(as_map["Grok"], 1);
    assert_eq!(as_map["Reasonix"], 2, ".events.jsonl not counted");
    // label order matches AAA_PY output order
    let labels: Vec<&str> = report.iter().map(|(l, _)| l.as_str()).collect();
    assert_eq!(labels, vec!["Claude", "Codex", "Pi", "agy", "Grok", "Reasonix"]);

    // target store contents gone
    assert!(!grok_dir.exists());
    assert!(!fx.paths.rnx_root().join(fx.cwd.replace('/', "-")).exists());
    let agy = fx.paths.agy_root();
    assert!(!agy.join("conversations").join("agy-cid.db").exists());
    assert!(!agy.join("brain").join("agy-cid").exists());
    assert!(agy.join("conversations").join("agy-shared.db").exists(), "shared conv kept");
    let lc: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(agy.join("cache").join("last_conversations.json")).unwrap(),
    )
    .unwrap();
    assert!(lc.get(&fx.cwd).is_none(), "target removed from last_conversations");
    assert!(lc.get(&fx.other).is_some());
    let hist = std::fs::read_to_string(agy.join("history.jsonl")).unwrap();
    assert!(!hist.contains(&format!("\"workspace\":\"{}\"", fx.cwd)));
    assert!(hist.contains("agy-shared"));

    // unrelated cwd untouched
    assert!(keep_claude.exists());
    assert!(keep_codex.exists());
    assert!(keep_pi.exists());
    assert!(keep_grok.exists());

    // second purge: nothing left to report
    let report2 = stores::purge(&fx.paths, &mut cache, &fx.cwd);
    assert!(report2.is_empty());

    // non-absolute target refused outright
    assert!(stores::purge(&fx.paths, &mut cache, "relative/path").is_empty());
}
