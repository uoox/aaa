//! Talking to aaa-daemon: connection discovery, REST calls, response types.
//!
//! The CLI is a daemon client and nothing else — every listing, launch and
//! kill it performs is the same API call the Mac app and the phone make, so
//! all three see one set of sessions.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub project_path: String,
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub question: Option<Question>,
    #[serde(default)]
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub last_output_at: String,
}

impl Session {
    pub fn is_live(&self) -> bool {
        self.state != "exited"
    }
    /// What the row says about this session: the pending question when there
    /// is one (that is the thing needing a human), else the title.
    pub fn headline(&self) -> &str {
        if let Some(q) = &self.question {
            if !q.text.is_empty() {
                return &q.text;
            }
        }
        &self.title
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Question {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct QuestionOption {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub label: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Project {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mtime: String,
    #[serde(default)]
    pub dir_size: u64,
    #[serde(default)]
    pub ctx_size: Option<u64>,
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub session_title: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Health {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub ssd_mounted: bool,
    #[serde(default)]
    pub project_root: String,
    #[serde(default)]
    pub uptime_s: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Permission {
    pub id: String,
    #[serde(default)]
    pub label: String,
    /// granted | denied | undetermined | needs_settings | unknown
    #[serde(default)]
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AgentInfo {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub available: bool,
}

pub struct Client {
    agent: ureq::Agent,
    /// `http://host:port`
    pub base: String,
    pub host: String,
    pub port: u16,
    pub token: String,
    pub local: bool,
}

fn config_path() -> PathBuf {
    let home = std::env::var("AAA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| "/".into());
    PathBuf::from(home).join(".config/aaa-daemon/config.toml")
}

impl Client {
    /// `AAA_HOST`/`AAA_TOKEN` win (that is how you drive a daemon on another
    /// machine over tailscale); otherwise read the daemon's own config, which
    /// is the single source of truth for port and token on this Mac.
    pub fn discover() -> Result<Client, String> {
        let env_host = std::env::var("AAA_HOST").ok().filter(|s| !s.is_empty());
        let (host, port, token) = match env_host {
            Some(h) => {
                let (host, port) = split_host(&h, 2730);
                let token = std::env::var("AAA_TOKEN")
                    .map_err(|_| "AAA_HOST 已设置但缺少 AAA_TOKEN".to_string())?;
                (host, port, token)
            }
            None => {
                let p = config_path();
                let body = std::fs::read_to_string(&p).map_err(|e| {
                    format!(
                        "读不到 {}: {e}\n先在本机跑一次 aaa-daemon run，或设置 AAA_HOST/AAA_TOKEN 指向远端",
                        p.display()
                    )
                })?;
                let cfg: aaa_daemon::config::Config = toml::from_str(&body)
                    .map_err(|e| format!("{}: {e}", p.display()))?;
                ("127.0.0.1".to_string(), cfg.port, cfg.token)
            }
        };
        let local = host == "127.0.0.1" || host == "localhost" || host == "::1";
        Ok(Client {
            agent: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(if local { 2 } else { 6 }))
                .timeout(Duration::from_secs(90))
                .build(),
            base: format!("http://{}:{port}", bracket(&host)),
            host,
            port,
            token,
            local,
        })
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.token)
    }

    fn run<T: serde::de::DeserializeOwned>(&self, req: ureq::Request, body: Option<serde_json::Value>) -> Result<T, String> {
        let req = req.set("Authorization", &self.auth());
        let resp = match body {
            Some(v) => req.send_json(v),
            None => req.call(),
        };
        match resp {
            Ok(r) => r.into_json::<T>().map_err(|e| format!("响应解析失败: {e}")),
            Err(ureq::Error::Status(code, r)) => {
                let text = r.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|v| {
                        v.get("message")
                            .or_else(|| v.get("error"))
                            .and_then(|m| m.as_str().map(String::from))
                    })
                    .unwrap_or(text);
                Err(format!("HTTP {code}: {msg}"))
            }
            Err(e) => Err(format!("连接 {} 失败: {e}", self.base)),
        }
    }

    pub fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.run(self.agent.get(&format!("{}{path}", self.base)), None)
    }

    pub fn post<T: serde::de::DeserializeOwned>(&self, path: &str, body: serde_json::Value) -> Result<T, String> {
        self.run(self.agent.post(&format!("{}{path}", self.base)), Some(body))
    }

    pub fn delete<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        self.run(self.agent.delete(&format!("{}{path}", self.base)), None)
    }

    pub fn health(&self) -> Result<Health, String> {
        self.get("/api/v1/health")
    }

    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        self.get("/api/v1/sessions")
    }

    pub fn projects(&self) -> Result<Vec<Project>, String> {
        self.get("/api/v1/projects")
    }

    pub fn agents(&self) -> Result<Vec<AgentInfo>, String> {
        self.get("/api/v1/agents")
    }

    pub fn create_project(&self, name: &str, agent: &str) -> Result<Project, String> {
        self.post("/api/v1/projects", serde_json::json!({"name": name, "agent": agent}))
    }

    pub fn create_session(&self, project_path: &str, agent: &str, resume: bool) -> Result<Session, String> {
        self.post(
            "/api/v1/sessions",
            serde_json::json!({"project_path": project_path, "agent": agent, "resume": resume}),
        )
    }

    pub fn input(&self, id: &str, text: &str, enter: bool) -> Result<serde_json::Value, String> {
        self.post(
            &format!("/api/v1/sessions/{id}/input"),
            serde_json::json!({"text": text, "enter": enter}),
        )
    }

    pub fn kill(&self, id: &str) -> Result<serde_json::Value, String> {
        self.post(&format!("/api/v1/sessions/{id}/kill"), serde_json::json!({}))
    }

    pub fn remove(&self, id: &str) -> Result<serde_json::Value, String> {
        self.delete(&format!("/api/v1/sessions/{id}"))
    }

    pub fn rename(&self, id: &str, title: &str) -> Result<serde_json::Value, String> {
        self.post(&format!("/api/v1/sessions/{id}/rename"), serde_json::json!({"title": title}))
    }

    pub fn set_agent(&self, path: &str, agent: &str) -> Result<serde_json::Value, String> {
        self.post("/api/v1/projects/agent", serde_json::json!({"path": path, "agent": agent}))
    }

    pub fn delete_projects(&self, paths: &[String]) -> Result<serde_json::Value, String> {
        self.post("/api/v1/projects/delete", serde_json::json!({"paths": paths}))
    }

    pub fn permissions(&self) -> Result<Vec<Permission>, String> {
        self.get("/api/v1/mac/permissions")
    }

    /// The dialogs open on the Mac, in the daemon's name — which is the whole
    /// point: agents are its children, so one round of clicking covers every
    /// session started from anywhere, including the phone.
    pub fn request_permissions(&self, ids: &[String]) -> Result<serde_json::Value, String> {
        self.post("/api/v1/mac/permissions/request", serde_json::json!({"ids": ids}))
    }

    /// Nudge a local daemon that is installed but not running. launchd owns it
    /// in the normal setup, and `kickstart` on an already-running job is a
    /// no-op, so this is safe to call speculatively — but only for localhost.
    pub fn wake_local(&self) -> bool {
        if !self.local {
            return false;
        }
        let uid = unsafe { libc::getuid() };
        let ok = std::process::Command::new("launchctl")
            .args(["kickstart", &format!("gui/{uid}/com.aaa.daemon")])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return false;
        }
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(100));
            if self.health().is_ok() {
                return true;
            }
        }
        false
    }
}

/// Split `host[:port]`, keeping bracketed IPv6 literals intact.
pub fn split_host(s: &str, default_port: u16) -> (String, u16) {
    let s = s.trim().trim_start_matches("http://").trim_end_matches('/');
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((h, tail)) = rest.split_once(']') {
            let port = tail.strip_prefix(':').and_then(|p| p.parse().ok()).unwrap_or(default_port);
            return (h.to_string(), port);
        }
    }
    match s.rsplit_once(':') {
        // a bare IPv6 literal has several colons and no port
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().unwrap_or(default_port)),
        _ => (s.to_string(), default_port),
    }
}

/// Wrap an IPv6 literal in brackets so it can go in a URL authority.
pub fn bracket(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_parsing() {
        assert_eq!(split_host("100.64.0.1:2730", 1), ("100.64.0.1".into(), 2730));
        assert_eq!(split_host("mac.tail.ts.net", 2730), ("mac.tail.ts.net".into(), 2730));
        assert_eq!(split_host("http://mac:9999/", 2730), ("mac".into(), 9999));
        assert_eq!(split_host("fd7a::1", 2730), ("fd7a::1".into(), 2730), "bare v6 keeps default port");
        assert_eq!(split_host("[fd7a::1]:2730", 1), ("fd7a::1".into(), 2730));
    }

    #[test]
    fn v6_authority_is_bracketed() {
        assert_eq!(bracket("fd7a::1"), "[fd7a::1]");
        assert_eq!(bracket("127.0.0.1"), "127.0.0.1");
        assert_eq!(bracket("[fd7a::1]"), "[fd7a::1]");
    }

    #[test]
    fn headline_prefers_the_pending_question() {
        let mut s: Session = serde_json::from_value(serde_json::json!({
            "id": "s_1", "title": "重构 imap", "state": "waiting"
        }))
        .unwrap();
        assert_eq!(s.headline(), "重构 imap");
        s.question = Some(Question { text: "要继续吗？".into(), options: vec![] });
        assert_eq!(s.headline(), "要继续吗？", "等待中的问题优先于标题");
    }
}
