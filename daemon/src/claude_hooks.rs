//! `aaa-daemon install-claude-hooks` — merge Notification / Stop hooks into
//! `~/.claude/settings.json`. Atomic, idempotent, preserves existing config.
//! Only ever run explicitly; tests exercise it against an AAA_HOME tempdir.

use std::io;

use serde_json::{json, Value};

use crate::paths::Paths;

const MARKER: &str = "/api/v1/hooks/claude";

fn hook_command(port: u16) -> String {
    format!(
        "curl -sf -m 3 -X POST -H 'Content-Type: application/json' --data-binary @- \
http://127.0.0.1:{port}{MARKER} >/dev/null 2>&1 || true"
    )
}

/// Merge our hook into one event array; returns true if anything changed.
fn merge_event(hooks_obj: &mut serde_json::Map<String, Value>, event: &str, cmd: &str) -> bool {
    let arr = hooks_obj
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(vec![]));
    if !arr.is_array() {
        *arr = Value::Array(vec![]);
    }
    let arr = arr.as_array_mut().unwrap();
    // idempotency: look for an existing entry that posts to our endpoint
    for matcher_entry in arr.iter_mut() {
        let Some(hooks) = matcher_entry.get_mut("hooks").and_then(|h| h.as_array_mut()) else {
            continue;
        };
        for h in hooks.iter_mut() {
            let existing = h.get("command").and_then(|c| c.as_str()).unwrap_or("");
            if existing.contains(MARKER) {
                if existing == cmd {
                    return false; // already correct
                }
                h["command"] = json!(cmd); // refresh (e.g. port changed)
                return true;
            }
        }
    }
    arr.push(json!({"hooks": [{"type": "command", "command": cmd}]}));
    true
}

/// Returns a human summary of what happened.
pub fn install(paths: &Paths, port: u16) -> io::Result<String> {
    let path = paths.claude_settings();
    let mut root: Value = match std::fs::read_to_string(&path) {
        Ok(body) => serde_json::from_str(&body).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not valid JSON ({e}); refusing to overwrite", path.display()),
            )
        })?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e),
    };
    if !root.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} top level is not an object; refusing to overwrite", path.display()),
        ));
    }
    let obj = root.as_object_mut().unwrap();
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "settings.hooks is not an object; refusing to overwrite",
        ));
    }
    let hooks_obj = hooks.as_object_mut().unwrap();
    let cmd = hook_command(port);
    let c1 = merge_event(hooks_obj, "Notification", &cmd);
    let c2 = merge_event(hooks_obj, "Stop", &cmd);
    if !(c1 || c2) {
        return Ok(format!("already installed: {}", path.display()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string_pretty(&root)?;
    let tmp = path.with_extension("json.aaa-tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, &path)?;
    Ok(format!("hooks merged into {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_settings(paths: &Paths) -> Value {
        serde_json::from_str(&std::fs::read_to_string(paths.claude_settings()).unwrap()).unwrap()
    }

    #[test]
    fn creates_fresh_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        install(&paths, 2730).unwrap();
        let v = read_settings(&paths);
        for ev in ["Notification", "Stop"] {
            let cmd = v["hooks"][ev][0]["hooks"][0]["command"].as_str().unwrap();
            assert!(cmd.contains("http://127.0.0.1:2730/api/v1/hooks/claude"));
            assert_eq!(v["hooks"][ev][0]["hooks"][0]["type"], "command");
        }
    }

    #[test]
    fn idempotent_and_preserves_existing() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            paths.claude_settings(),
            serde_json::to_string(&json!({
                "model": "opus",
                "hooks": {
                    "Notification": [
                        {"matcher": "", "hooks": [{"type": "command", "command": "existing-hook"}]}
                    ],
                    "PreToolUse": [{"matcher": "Bash", "hooks": [{"type":"command","command":"rtk-hook"}]}]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        install(&paths, 2730).unwrap();
        let first = read_settings(&paths);
        // run again: no change
        let msg = install(&paths, 2730).unwrap();
        assert!(msg.contains("already installed"));
        let second = read_settings(&paths);
        assert_eq!(first, second);
        // existing entries preserved
        assert_eq!(second["model"], "opus");
        assert_eq!(
            second["hooks"]["Notification"][0]["hooks"][0]["command"],
            "existing-hook"
        );
        assert_eq!(second["hooks"]["PreToolUse"][0]["matcher"], "Bash");
        // ours appended
        let ours = second["hooks"]["Notification"][1]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(ours.contains(MARKER));
        // port change refreshes in place instead of duplicating
        install(&paths, 9999).unwrap();
        let third = read_settings(&paths);
        assert_eq!(third["hooks"]["Notification"].as_array().unwrap().len(), 2);
        assert!(third["hooks"]["Notification"][1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(":9999"));
    }

    #[test]
    fn refuses_corrupt_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(paths.claude_settings(), "{not json").unwrap();
        assert!(install(&paths, 2730).is_err());
        // untouched
        assert_eq!(
            std::fs::read_to_string(paths.claude_settings()).unwrap(),
            "{not json"
        );
    }
}
