//! `/sessions/:id/ports` — listening TCP ports of the session's process tree,
//! via `ps` (tree) + `lsof` (listeners).

use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct PortEntry {
    pub port: u16,
    pub cmd: String,
}

/// pids: root pid -> all descendants (inclusive).
fn process_tree(root: u32) -> HashSet<u32> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    if let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid="])
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = line.split_whitespace();
            let (Some(pid), Some(ppid)) = (it.next(), it.next()) else { continue };
            let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else { continue };
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut seen = HashSet::new();
    let mut stack = vec![root];
    while let Some(p) = stack.pop() {
        if !seen.insert(p) {
            continue;
        }
        if let Some(kids) = children.get(&p) {
            stack.extend(kids.iter().copied());
        }
    }
    seen
}

pub fn listening_ports(root_pid: u32) -> Vec<PortEntry> {
    let tree = process_tree(root_pid);
    let Ok(out) = std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"])
        .output()
    else {
        return vec![];
    };
    parse_lsof(&String::from_utf8_lossy(&out.stdout), &tree)
}

/// Parse `lsof -Fpcn` machine output, keeping only pids in `tree`.
fn parse_lsof(out: &str, tree: &HashSet<u32>) -> Vec<PortEntry> {
    let mut cur_pid: Option<u32> = None;
    let mut cur_cmd = String::new();
    let mut found: Vec<PortEntry> = Vec::new();
    let mut seen_ports: HashSet<u16> = HashSet::new();
    for line in out.lines() {
        let (tag, rest) = match line.split_at_checked(1) {
            Some(x) => x,
            None => continue,
        };
        match tag {
            "p" => {
                cur_pid = rest.parse::<u32>().ok();
                cur_cmd.clear();
            }
            "c" => cur_cmd = rest.to_string(),
            "n" => {
                let Some(pid) = cur_pid else { continue };
                if !tree.contains(&pid) {
                    continue;
                }
                // n*:8080 or n127.0.0.1:3000 or n[::1]:9000
                let Some(idx) = rest.rfind(':') else { continue };
                let Ok(port) = rest[idx + 1..].parse::<u16>() else { continue };
                if seen_ports.insert(port) {
                    found.push(PortEntry { port, cmd: cur_cmd.clone() });
                }
            }
            _ => {}
        }
    }
    found.sort_by_key(|e| e.port);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lsof_field_output() {
        let out = "p123\ncnode\nn*:3000\nn127.0.0.1:3000\np999\ncother\nn*:9999\np124\ncvite\nn[::1]:5173\n";
        let tree: HashSet<u32> = [123u32, 124].into_iter().collect();
        let entries = parse_lsof(out, &tree);
        assert_eq!(
            entries,
            vec![
                PortEntry { port: 3000, cmd: "node".into() },
                PortEntry { port: 5173, cmd: "vite".into() },
            ]
        );
    }
}
