//! `aaa` — the command-line front end for aaa-daemon.
//!
//! The old `aaa` was a zsh menu that ran agents as children of the terminal
//! it was invoked from; it now lives on as `aaal`. This one owns nothing: it
//! asks the daemon, so a session started here can be picked up on the Mac app
//! or the phone, and survives closing the window.

mod attach;
mod client;
mod cmds;
mod menu;
mod render;
mod tty;

use client::Client;
use cmds::die;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");

    if matches!(cmd, "help" | "-h" | "--help") {
        print!("{}", cmds::HELP);
        return;
    }
    if matches!(cmd, "-V" | "--version" | "version") {
        println!("aaa {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let c = match Client::discover() {
        Ok(c) => c,
        Err(e) => die(&e),
    };
    // One health probe up front: it turns every "connection refused" deep in
    // a command into one clear message, and gives a launchd-managed daemon a
    // chance to come back before we give up.
    if c.health().is_err() && !c.wake_local() {
        die(&format!(
            "连不上 daemon {}:{}\n  本机：aaa-daemon service install（或 aaa-daemon run）\n  远端：检查 AAA_HOST / tailscale",
            c.host, c.port
        ));
    }

    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let json = rest.contains(&"--json");
    let result = match cmd {
        "" => menu::run(&c),
        "ls" => cmds::ls(&c, rest.contains(&"-a") || rest.contains(&"--all"), json),
        "ps" | "projects" => cmds::ps(&c, json),
        "wait" => cmds::wait(&c, json),
        "status" | "st" => cmds::status(&c, json),
        "perms" | "permissions" => {
            let pos = positional(&rest);
            if pos.is_empty() {
                cmds::perms(&c, json)
            } else {
                let ids = if pos == ["all"] {
                    Vec::new()
                } else {
                    pos.iter().map(|s| s.to_string()).collect()
                };
                cmds::perms_request(&c, &ids)
            }
        }
        "new" | "n" => {
            let name = positional(&rest).first().copied().unwrap_or("").to_string();
            let agent = flag(&rest, "-a").or_else(|| flag(&rest, "--agent")).unwrap_or_else(|| "claude".into());
            cmds::new_project(&c, &name, &agent)
        }
        "open" | "o" => with_target(&rest, |t| cmds::open(&c, t)),
        "attach" | "a" => with_target(&rest, |t| cmds::attach_cmd(&c, t)),
        "kill" | "k" => with_target(&rest, |t| cmds::kill(&c, t)),
        "rm" => with_target(&rest, |t| cmds::remove(&c, t)),
        "say" | "answer" => {
            let pos = positional(&rest);
            match pos.split_first() {
                Some((t, words)) if !words.is_empty() => cmds::say(&c, t, &words.join(" ")),
                _ => Err("用法: aaa say <目标> <文本…>".into()),
            }
        }
        "rename" => {
            let pos = positional(&rest);
            match pos.split_first() {
                Some((t, words)) if !words.is_empty() => cmds::rename(&c, t, &words.join(" ")),
                _ => Err("用法: aaa rename <目标> <标题>".into()),
            }
        }
        other => Err(format!("未知命令 {other}；aaa help 看用法")),
    };
    if let Err(e) = result {
        die(&e);
    }
}

fn with_target(rest: &[&str], f: impl FnOnce(&str) -> Result<(), String>) -> Result<(), String> {
    match positional(rest).first() {
        Some(t) => f(t),
        None => Err("缺少目标；aaa ls 看有哪些会话".into()),
    }
}

/// Everything that is not a flag or a flag's value.
fn positional<'a>(rest: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in rest {
        if skip {
            skip = false;
            continue;
        }
        if *a == "-a" || *a == "--agent" {
            skip = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        out.push(*a);
    }
    out
}

fn flag(rest: &[&str], name: &str) -> Option<String> {
    rest.iter().position(|a| *a == name).and_then(|i| rest.get(i + 1)).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positional_skips_flags_and_their_values() {
        let rest = ["mytask", "-a", "codex", "--json"];
        assert_eq!(positional(&rest), ["mytask"]);
        assert_eq!(flag(&rest, "-a"), Some("codex".into()));
    }

    #[test]
    fn ls_all_is_not_mistaken_for_the_agent_flag() {
        // `-a` means --all for ls and --agent for new; positional() drops the
        // token either way, so a bare `aaa ls -a` still has no target.
        let rest = ["-a"];
        assert!(positional(&rest).is_empty());
        assert_eq!(flag(&rest, "-a"), None, "末尾的 -a 没有值");
    }

    #[test]
    fn say_needs_both_target_and_words() {
        let rest = ["aaa-ui", "继续", "跑"];
        let pos = positional(&rest);
        let (t, words) = pos.split_first().unwrap();
        assert_eq!(*t, "aaa-ui");
        assert_eq!(words.join(" "), "继续 跑");
    }
}
