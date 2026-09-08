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

/// The login shell sessions run under: zsh where present (macOS default),
/// otherwise bash (Linux boxes without zsh).
pub fn login_shell() -> &'static str {
    if std::path::Path::new("/bin/zsh").exists() || std::path::Path::new("/usr/bin/zsh").exists() {
        "zsh"
    } else {
        "bash"
    }
}

/// The agent's command line with the login shell filled in (the table says
/// `zsh`; a Linux host without zsh gets bash).
pub fn spawn_cmd(agent: &AgentDef) -> String {
    if agent.id == "shell" {
        format!("exec {} -l", login_shell())
    } else {
        agent.cmd.to_string()
    }
}

/// The argv used to spawn a session: `<shell> -lc 'cd <dir> && <cmd>'`.
pub fn spawn_argv(dir: &str, cmd: &str) -> Vec<String> {
    vec![
        login_shell().to_string(),
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

/// The PATH agents are spawned with — and the one `which` searches, so
/// "available" in `GET /agents` can never disagree with what actually spawns.
///
/// Under launchd the daemon inherits a bare `/usr/bin:/bin:/usr/sbin:/sbin`,
/// and `zsh -lc` does not rescue it: a *non-interactive* login zsh sources
/// `.zprofile` but never `.zshrc`, which is where PATH is normally maintained.
/// So every agent dies with "command not found" while `shell` works fine —
/// and the failure hides completely if the daemon was ever started by hand,
/// because then it inherits a terminal's PATH.
///
/// We therefore ask an interactive login shell what PATH should be, once, and
/// merge the usual install directories in behind it as a floor.
static AGENT_PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn agent_path(home: &std::path::Path) -> String {
    AGENT_PATH
        .get_or_init(|| merge_path(probe_login_path().as_deref(), home))
        .clone()
}

/// Probed PATH first (it is the user's own ordering), then the fallback dirs
/// that are not already in it. Either half may be missing.
fn merge_path(probed: Option<&str>, home: &std::path::Path) -> String {
    let mut dirs: Vec<String> = Vec::new();
    let push = |dirs: &mut Vec<String>, d: String| {
        if !d.is_empty() && !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    for d in probed.unwrap_or("").split(':') {
        push(&mut dirs, d.to_string());
    }
    for d in [
        home.join(".local").join("bin"),
        // npm's default prefix for a user-level install (pi, reasonix)
        home.join(".npm-global").join("bin"),
        home.join(".cargo").join("bin"),
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/opt/homebrew/sbin"),
        std::path::PathBuf::from("/usr/local/bin"),
        std::path::PathBuf::from("/usr/bin"),
        std::path::PathBuf::from("/bin"),
        std::path::PathBuf::from("/usr/sbin"),
        std::path::PathBuf::from("/sbin"),
    ] {
        push(&mut dirs, d.to_string_lossy().into_owned());
    }
    dirs.join(":")
}

/// `zsh -lic` sources `.zshrc`, so this reflects what the user actually gets
/// in a terminal. Anything `.zshrc` prints lands on stdout ahead of the PATH,
/// so we take the last line and only trust it if it looks like a PATH. Runs
/// on its own thread with a deadline: a `.zshrc` that blocks must not wedge
/// daemon startup.
/// 起一个子进程，把 `stdin_data` 喂进去，最多等 `timeout`，超时就杀掉；返回 stdout。
/// 任何失败都返回 `None`——两个调用方都是「拿不到就降级」。
///
/// 2026-09-08 合并：namer 的 `run_haiku` 手写过一遍（读线程 + 100ms 轮询 `try_wait`
/// + 手算 deadline），这里的 `probe_login_path` 又写了个简版，而且超时之后子进程
/// 没人收。合成一个，顺手把那个漏掉的 kill 补上。
///
/// 刻意不看退出码：两个调用方要的都是 stdout，成不成由它们自己按内容判断。
pub(crate) fn run_with_timeout(
    mut cmd: std::process::Command,
    stdin_data: Option<&[u8]>,
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    let mut child = cmd
        .stdin(if stdin_data.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(data) = stdin_data {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(data);
            // drop 关掉管道，子进程才看得到 EOF
        }
    }
    // stdout 必须另起一个线程读：管道缓冲区满了子进程会阻塞在写上，永远等不到它退出
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = String::new();
        let _ = stdout.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

fn probe_login_path() -> Option<String> {
    let mut cmd = std::process::Command::new("zsh");
    cmd.args(["-lic", "printf %s \"$PATH\""]);
    let raw = run_with_timeout(cmd, None, std::time::Duration::from_secs(5))?;
    let last = raw.lines().last()?.trim();
    (last.contains(':') && last.split(':').any(|d| d == "/usr/bin")).then(|| last.to_string())
}

/// The PATH to hand a spawned agent. The daemon resolves it at startup so the
/// login-shell probe never lands in the latency of opening a session; the
/// fallback only fires in tests, which never spawn a real agent.
pub fn spawn_path() -> String {
    AGENT_PATH.get().cloned().unwrap_or_else(|| {
        let home = std::env::var("AAA_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/".into());
        merge_path(None, std::path::Path::new(&home))
    })
}

/// Absolute path to a tool on the agent PATH.
///
/// `Command::new("tailscale")` cannot find anything outside the launchd
/// default PATH: program lookup happens against the *daemon's* environment,
/// and `.env("PATH", …)` on the child does not change it. So resolve first,
/// then spawn the absolute path.
pub fn tool(bin: &str) -> Option<std::path::PathBuf> {
    if bin.contains('/') {
        let p = std::path::PathBuf::from(bin);
        return p.is_file().then_some(p);
    }
    spawn_path()
        .split(':')
        .map(|dir| std::path::Path::new(dir).join(bin))
        .find(|p| is_executable(p))
}

pub fn which(bin: &str, home: &std::path::Path) -> Option<std::path::PathBuf> {
    // initialises the shared PATH if the daemon has not yet
    let _ = agent_path(home);
    tool(bin)
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
        // 2026-09-03 起只支持 Claude Code；shell 是终端面板，不是 agent
        assert_eq!(AGENTS.len(), 2);
        assert_eq!(get("claude").unwrap().cmd, "claude --dangerously-skip-permissions");
        assert_eq!(
            build_resume_cmd(get("claude").unwrap(), "abc-123").unwrap(),
            "claude --resume abc-123 --dangerously-skip-permissions"
        );
        assert!(get("codex").is_none() && get("agy").is_none());
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
    fn merged_path_keeps_the_users_order_and_adds_the_floor() {
        let home = std::path::Path::new("/Users/x");
        let p = merge_path(Some("/opt/homebrew/bin:/usr/bin"), home);
        let dirs: Vec<&str> = p.split(':').collect();
        assert_eq!(dirs[0], "/opt/homebrew/bin", "探测到的顺序在前");
        assert_eq!(dirs[1], "/usr/bin");
        assert!(dirs.contains(&"/Users/x/.local/bin"), "claude/codex/agy 装在这里");
        assert!(dirs.contains(&"/Users/x/.npm-global/bin"), "pi/reasonix 装在这里");
        assert_eq!(dirs.iter().filter(|d| **d == "/usr/bin").count(), 1, "不重复");
    }

    #[test]
    fn merged_path_without_a_probe_still_finds_the_agents() {
        // the launchd case: probe failed, fallbacks alone must be enough
        let p = merge_path(None, std::path::Path::new("/Users/x"));
        for want in ["/Users/x/.local/bin", "/Users/x/.npm-global/bin", "/usr/bin"] {
            assert!(p.split(':').any(|d| d == want), "{want} 不在 {p}");
        }
        assert!(!p.starts_with(':'), "空的探测结果不该留下空目录");
    }

    #[test]
    fn bin_extraction() {
        assert_eq!(agent_bin(get("shell").unwrap()), "zsh");
        assert_eq!(agent_bin(get("claude").unwrap()), "claude");
    }
}
