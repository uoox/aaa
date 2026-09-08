//! `~/.config/aaa-daemon/config.toml` — generated on first run.

use std::io;
use std::path::{Path, PathBuf};

use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    pub token: String,
    #[serde(default = "default_project_root")]
    pub project_root: PathBuf,
    #[serde(default = "default_true")]
    pub namer: bool,
    /// Press Enter on Claude Code's「Do you trust the files in this folder?」
    /// for the user — they already picked the folder in AAA. See trust.rs.
    #[serde(default = "default_true")]
    pub auto_trust: bool,
}

fn default_port() -> u16 {
    2730
}
fn default_project_root() -> PathBuf {
    crate::paths::Paths::from_env().home.join("project")
}
fn default_true() -> bool {
    true
}

pub fn generate_token() -> String {
    let bytes: [u8; 16] = rand::rng().random();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("aaa_tk_{hex}")
}

impl Config {
    fn fresh() -> Self {
        Config {
            port: default_port(),
            token: generate_token(),
            project_root: default_project_root(),
            namer: true,
            auto_trust: true,
        }
    }
}

/// Load config; create it (with a random token) on first run.
pub fn load_or_create(path: &Path) -> io::Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let cfg = Config::fresh();
            write_config(path, &cfg)?;
            Ok(cfg)
        }
        Err(e) => Err(e),
    }
}

pub fn write_config(path: &Path, cfg: &Config) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = toml::to_string_pretty(cfg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_shape() {
        let t = generate_token();
        assert!(t.starts_with("aaa_tk_"));
        assert_eq!(t.len(), "aaa_tk_".len() + 32);
        assert!(t["aaa_tk_".len()..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn v1_0_config_without_new_sections_still_parses() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        // exactly what a first-generation daemon wrote; the retired [ntfy] /
        // [watchdog] / [checkpoint] sections must still be tolerated
        std::fs::write(
            &p,
            "port = 2730\ntoken = \"aaa_tk_old\"\nproject_root = \"/Volumes/SSD/project\"\nnamer = true\n\n[ntfy]\nurl = \"https://ntfy.example.com\"\ntopic = \"aaa\"\n",
        )
        .unwrap();
        let cfg = load_or_create(&p).unwrap();
        assert_eq!(cfg.token, "aaa_tk_old");
    }

    #[test]
    fn partial_new_sections_get_field_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "token = \"aaa_tk_x\"\n[checkpoint]\nenabled = false\n[watchdog]\nauto_kill = true\nstall_minutes = 3\n",
        )
        .unwrap();
        let cfg = load_or_create(&p).unwrap();
        assert!(cfg.auto_trust, "missing fields default");
    }

    #[test]
    fn create_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cfg").join("config.toml");
        let cfg = load_or_create(&p).unwrap();
        assert_eq!(cfg.port, 2730);
        assert!(cfg.namer);
        let again = load_or_create(&p).unwrap();
        assert_eq!(cfg.token, again.token, "token must be stable across loads");
    }
}
