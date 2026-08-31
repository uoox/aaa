//! Is the project root actually usable?
//!
//! `is_dir()` is not enough. The project root lives on an external volume, and
//! macOS gates removable volumes behind TCC — a process launchd started has no
//! responsible app holding that grant, so `stat` succeeds while `opendir`
//! either fails with EPERM or, worse, **blocks forever** waiting for a consent
//! dialog nobody is in front of. A daemon whose whole point is being reachable
//! when the user is away must never wait on that.
//!
//! So every check runs on a throwaway thread with a deadline, and a root we
//! cannot read is reported as its own state rather than being confused with an
//! unmounted disk: the two need completely different advice.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long to wait before calling a silent `opendir` a denial. Generous for a
/// spinning disk, far short of "hung until someone clicks something".
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    Ok,
    /// not mounted (or never existed) — the SSD guard's original case
    Missing,
    /// mounted but unreadable: TCC, or permissions
    Denied,
}

impl RootState {
    pub fn is_ok(self) -> bool {
        self == RootState::Ok
    }
    /// What `/health.ssd_mounted` has always meant to clients: "can I use it?"
    pub fn usable(self) -> bool {
        self.is_ok()
    }
    pub fn code(self) -> &'static str {
        match self {
            RootState::Ok => "ok",
            RootState::Missing => "unmounted",
            RootState::Denied => "denied",
        }
    }
    pub fn as_u8(self) -> u8 {
        match self {
            RootState::Ok => 0,
            RootState::Missing => 1,
            RootState::Denied => 2,
        }
    }
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => RootState::Ok,
            1 => RootState::Missing,
            _ => RootState::Denied,
        }
    }
}

/// Probe the root without ever blocking the caller for long.
pub fn check(root: &Path) -> RootState {
    let root = root.to_path_buf();
    probe_with_timeout(root, PROBE_TIMEOUT)
}

fn probe_with_timeout(root: PathBuf, timeout: Duration) -> RootState {
    // read_dir, not metadata: stat is allowed through the TCC gate that
    // opendir is not, so only opening the directory proves readability.
    with_deadline(timeout, move || match std::fs::read_dir(&root) {
        Ok(_) => RootState::Ok,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RootState::Missing,
        Err(_) => RootState::Denied,
    })
}

/// Run `f` on a throwaway thread; a probe that misses the deadline counts as a
/// denial. The thread stays parked on the syscall — there is no way to cancel
/// a blocked `open` — but the daemon gets on with starting up.
fn with_deadline(timeout: Duration, f: impl FnOnce() -> RootState + Send + 'static) -> RootState {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(timeout).unwrap_or(RootState::Denied)
}

/// The one-line fix, printed at startup and returned in API errors so it
/// reaches whoever is holding a phone rather than only the Mac's log.
pub fn advice(state: RootState, root: &Path) -> String {
    match state {
        RootState::Ok => String::new(),
        RootState::Missing => format!("项目根 {} 未挂载；挂上后自动恢复", root.display()),
        RootState::Denied => format!(
            "项目根 {} 读不了：launchd 启动的进程没有可移动卷访问权。\n  \
             到「系统设置 → 隐私与安全性 → 完全磁盘访问权限」把 aaa-daemon 加进去并打开，然后\n  \
             aaa-daemon service uninstall && aaa-daemon service install",
            root.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_readable_dir_is_ok() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(check(d.path()), RootState::Ok);
    }

    #[test]
    fn a_missing_dir_is_missing_not_denied() {
        // the two states carry different advice, so they must not collapse
        assert_eq!(check(Path::new("/nope/not/here")), RootState::Missing);
        assert!(advice(RootState::Missing, Path::new("/x")).contains("未挂载"));
        assert!(advice(RootState::Denied, Path::new("/x")).contains("完全磁盘访问"));
    }

    #[test]
    fn a_probe_that_never_answers_is_a_denial_not_a_hang() {
        // the TCC case: opendir parks forever waiting for a consent dialog
        let start = std::time::Instant::now();
        let state = with_deadline(Duration::from_millis(80), || {
            std::thread::sleep(Duration::from_secs(30));
            RootState::Ok
        });
        assert_eq!(state, RootState::Denied);
        assert!(start.elapsed() < Duration::from_secs(2), "必须立刻返回，不能等人来点");
    }

    #[test]
    fn a_probe_that_answers_in_time_wins() {
        assert_eq!(with_deadline(Duration::from_secs(2), || RootState::Ok), RootState::Ok);
    }
}
