//! Injectable filesystem roots (+ the shared atomic-write helper).
//!
//! Everything the daemon reads or writes outside the project root is derived
//! from `home`, which honours `AAA_HOME` (same convention as the AAA_PY
//! embedded in `~/.local/bin/aaa`) so tests can point it at a tempdir.

use std::path::{Path, PathBuf};

/// Atomic file write: same-directory tmp (`<name>.tmp`) + rename, so readers
/// never observe a partial file. Callers that may race on the same path must
/// serialize themselves (the tmp name is not unique per writer).
pub fn write_atomic(path: &Path, body: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::var("AAA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".to_string());
        Self { home: PathBuf::from(home) }
    }

    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    // ---- Claude Code 会话存储（本应用只认 Claude Code） ----
    pub fn claude_root(&self) -> PathBuf {
        self.home.join(".claude").join("projects")
    }

    /// `~/.cache/aaa-cwds.json` — shared with the aaa CLI, format-compatible.
    pub fn cwd_cache(&self) -> PathBuf {
        self.home.join(".cache").join("aaa-cwds.json")
    }

    // ---- daemon-owned state ----
    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".local").join("state").join("aaa-daemon")
    }
    pub fn sessions_dir(&self) -> PathBuf {
        self.state_dir().join("sessions")
    }
    pub fn port_file(&self) -> PathBuf {
        self.state_dir().join("daemon.port")
    }

    /// Config path; `AAA_DAEMON_CONFIG` overrides (tests).
    pub fn config_path(&self) -> PathBuf {
        std::env::var("AAA_DAEMON_CONFIG")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                self.home
                    .join(".config")
                    .join("aaa-daemon")
                    .join("config.toml")
            })
    }

    pub fn claude_settings(&self) -> PathBuf {
        self.home.join(".claude").join("settings.json")
    }

    /// Linux：systemd 用户单元
    pub fn systemd_unit(&self) -> PathBuf {
        self.home
            .join(".config")
            .join("systemd")
            .join("user")
            .join("aaa-daemon.service")
    }

    pub fn launchd_plist(&self) -> PathBuf {
        self.home
            .join("Library")
            .join("LaunchAgents")
            .join("com.aaa.daemon.plist")
    }
}
