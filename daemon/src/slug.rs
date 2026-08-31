//! slugify + timestamped-empty-dir cleanup, ported from the aaa zsh script.

/// Port of aaa `slugify`: keep case / CJK / digits / letters, drop chars that
/// are unfriendly to the filesystem or display. Order matters and mirrors the
/// zsh implementation:
///   1. strip control chars; 2. whitespace -> `-`; 3. drop `/` and `:`;
///   4. collapse `--`; 5. strip leading `.`s, leading and trailing `-`s.
pub fn slugify(input: &str) -> String {
    let mut s: String = input.chars().filter(|c| !c.is_control()).collect();
    s = s
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    s = s.chars().filter(|&c| c != '/' && c != ':').collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_start_matches('.');
    let s = s.trim_start_matches('-');
    let s = s.trim_end_matches('-');
    s.to_string()
}

/// `date +%Y-%m-%d-%H%M` (local time), used when the project name is empty.
pub fn timestamp_name() -> String {
    chrono::Local::now().format("%Y-%m-%d-%H%M").to_string()
}

/// Matches `^[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{4}(-[a-z0-9._-]+)?$`.
pub fn is_timestamped_name(name: &str) -> bool {
    let b = name.as_bytes();
    if b.len() < 15 {
        return false;
    }
    let d = |i: usize| b[i].is_ascii_digit();
    let head_ok = d(0)
        && d(1)
        && d(2)
        && d(3)
        && b[4] == b'-'
        && d(5)
        && d(6)
        && b[7] == b'-'
        && d(8)
        && d(9)
        && b[10] == b'-'
        && d(11)
        && d(12)
        && d(13)
        && d(14);
    if !head_ok {
        return false;
    }
    if b.len() == 15 {
        return true;
    }
    if b[15] != b'-' || b.len() == 16 {
        return false;
    }
    b[16..]
        .iter()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// Startup cleanup ported from `cleanup_empty_timestamped`: remove empty,
/// timestamp-named subdirectories of `root` untouched for > 60 minutes.
/// Never creates anything; ignores all errors.
pub fn cleanup_empty_timestamped(root: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60);
    for entry in rd.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
        if !is_timestamped_name(name) {
            continue;
        }
        // empty check includes hidden entries (ls -A semantics)
        let empty = std::fs::read_dir(&p)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if !empty {
            continue;
        }
        let old = std::fs::metadata(&p)
            .and_then(|m| m.modified())
            .map(|m| m < cutoff)
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_dir(&p); // remove_dir refuses non-empty; safe
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_ports_zsh_behaviour() {
        assert_eq!(slugify("hello world"), "hello-world");
        assert_eq!(slugify("  a  b  "), "a-b");
        assert_eq!(slugify("a/b:c"), "abc");
        assert_eq!(slugify("...hidden"), "hidden");
        assert_eq!(slugify("--x--"), "x");
        assert_eq!(slugify("中文 项目"), "中文-项目");
        assert_eq!(slugify("tab\there"), "tabhere"); // TAB is [[:cntrl:]], removed before space folding
        assert_eq!(slugify("ctrl\u{7}char"), "ctrlchar");
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("aaa-ui 交互原型"), "aaa-ui-交互原型");
    }

    #[test]
    fn timestamped_name_matcher() {
        assert!(is_timestamped_name("2026-08-30-1234"));
        assert!(is_timestamped_name("2026-08-30-1234-foo.bar_1"));
        assert!(!is_timestamped_name("2026-08-30-123"));
        assert!(!is_timestamped_name("2026-08-30-1234-"));
        assert!(!is_timestamped_name("2026-08-30-1234-FOO"));
        assert!(!is_timestamped_name("x2026-08-30-1234"));
        assert!(!is_timestamped_name("aaa-ui"));
    }

    #[test]
    fn cleanup_only_removes_old_empty_timestamped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let old_empty = root.join("2020-01-01-0000");
        let fresh_empty = root.join("2026-01-01-0000");
        let old_nonempty = root.join("2020-01-01-0001");
        let normal = root.join("myproject");
        for d in [&old_empty, &fresh_empty, &old_nonempty, &normal] {
            std::fs::create_dir(d).unwrap();
        }
        std::fs::write(old_nonempty.join("f"), "x").unwrap();
        // age the two "old" dirs
        let past = filetime_set(&old_empty);
        filetime_set(&old_nonempty);
        assert!(past);
        cleanup_empty_timestamped(root);
        assert!(!old_empty.exists(), "old empty timestamped dir removed");
        assert!(fresh_empty.exists(), "fresh dir kept");
        assert!(old_nonempty.exists(), "non-empty dir kept");
        assert!(normal.exists(), "non-timestamped dir kept");
    }

    /// Set mtime to 2 hours ago without extra deps (utimes via libc).
    fn filetime_set(p: &std::path::Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let t = libc::timeval { tv_sec: now - 7200, tv_usec: 0 };
        let times = [t, t];
        unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) == 0 }
    }
}
