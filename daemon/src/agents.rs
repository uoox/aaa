//! Agent table — must stay identical to PROTOCOL.md / the aaa CLI.

pub struct AgentDef {
    pub id: &'static str,
    pub label: &'static str,
    pub cmd: &'static str,
    /// Resume template; `%ID%` replaced with the shell-quoted session id.
    pub resume_cmd: Option<&'static str>,
}

pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        label: "Claude",
        cmd: "claude --dangerously-skip-permissions",
        resume_cmd: Some("claude --resume %ID% --dangerously-skip-permissions"),
    },
    AgentDef {
        id: "codex",
        label: "Codex",
        cmd: "codex --dangerously-bypass-approvals-and-sandbox",
        resume_cmd: Some("codex resume %ID% --dangerously-bypass-approvals-and-sandbox"),
    },
    AgentDef {
        id: "pi",
        label: "Pi",
        cmd: "pi",
        // pi sessions are stored per-cwd; --continue means "most recent in this
        // directory" (the found id only triggers the resume branch).
        resume_cmd: Some("pi --continue"),
    },
    AgentDef {
        id: "reasonix",
        label: "Reasonix",
        cmd: "reasonix --permission-mode bypassPermissions",
        resume_cmd: Some("reasonix --continue --permission-mode bypassPermissions"),
    },
    AgentDef {
        id: "agy",
        label: "Antigravity",
        cmd: "agy --dangerously-skip-permissions",
        resume_cmd: Some("agy --conversation %ID% --dangerously-skip-permissions"),
    },
    AgentDef {
        id: "shell",
        label: "终端",
        cmd: "exec zsh -l",
        resume_cmd: None,
    },
];

pub fn get(id: &str) -> Option<&'static AgentDef> {
    AGENTS.iter().find(|a| a.id == id)
}

/// POSIX single-quote shell quoting (equivalent in effect to zsh `${(q)}` for
/// our purposes: the result is safe to embed in a `zsh -lc` command line).
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b'+' | b':' | b'@' | b'%' | b',')
        })
    {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Build the resume command from the template, or None if the agent has no
/// resume entry.
pub fn build_resume_cmd(agent: &AgentDef, sid: &str) -> Option<String> {
    agent
        .resume_cmd
        .map(|tpl| tpl.replace("%ID%", &shell_quote(sid)))
}

/// The argv used to spawn a session: `zsh -lc 'cd <dir> && <cmd>'`.
pub fn spawn_argv(dir: &str, cmd: &str) -> Vec<String> {
    vec![
        "zsh".to_string(),
        "-lc".to_string(),
        format!("cd {} && {}", shell_quote(dir), cmd),
    ]
}

/// First word of the agent command = executable name (for availability check).
pub fn agent_bin(agent: &AgentDef) -> &'static str {
    let cmd = agent.cmd;
    // "exec zsh -l" -> zsh
    let mut words = cmd.split_whitespace();
    let first = words.next().unwrap_or(cmd);
    if first == "exec" {
        words.next().unwrap_or(first)
    } else {
        first
    }
}

/// PATH lookup with fallbacks to the usual install locations (the daemon may
/// run under launchd with a minimal PATH).
pub fn which(bin: &str, home: &std::path::Path) -> Option<std::path::PathBuf> {
    if bin.contains('/') {
        let p = std::path::PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if dir.is_empty() {
                continue;
            }
            let p = std::path::Path::new(dir).join(bin);
            if is_executable(&p) {
                return Some(p);
            }
        }
    }
    for dir in [
        home.join(".local").join("bin"),
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/local/bin"),
        std::path::PathBuf::from("/usr/bin"),
        std::path::PathBuf::from("/bin"),
    ] {
        let p = dir.join(bin);
        if is_executable(&p) {
            return Some(p);
        }
    }
    None
}

fn is_executable(p: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_protocol() {
        assert_eq!(AGENTS.len(), 6);
        assert_eq!(get("claude").unwrap().cmd, "claude --dangerously-skip-permissions");
        assert_eq!(
            build_resume_cmd(get("codex").unwrap(), "abc-123").unwrap(),
            "codex resume abc-123 --dangerously-bypass-approvals-and-sandbox"
        );
        assert_eq!(build_resume_cmd(get("pi").unwrap(), "x").unwrap(), "pi --continue");
        assert!(build_resume_cmd(get("shell").unwrap(), "x").is_none());
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("/a/b-c"), "/a/b-c");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        let argv = spawn_argv("/tmp/x y", "claude --foo");
        assert_eq!(argv[2], "cd '/tmp/x y' && claude --foo");
    }

    #[test]
    fn bin_extraction() {
        assert_eq!(agent_bin(get("shell").unwrap()), "zsh");
        assert_eq!(agent_bin(get("claude").unwrap()), "claude");
    }
}
