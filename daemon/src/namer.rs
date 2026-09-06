//! AI session naming — port of the AAA_PY naming chain.
//!
//! Priority (claude): customTitle > ai-title > summary > haiku summary >
//! first user message > slug. Haiku results are cached by file size
//! (`ainame:<fn>`, regenerate when grown >30% + 4KB); final result cached by
//! mtime (`cname2:<fn>`). Cache keys are bidirectionally compatible with the
//! aaa CLI.

use std::path::Path;

use serde_json::Value;

use crate::cache::CwdCache;
use crate::stores::{fstat, jsonl_head, jsonl_tail};

const HAIKU_PROMPT_PREFIX: &str = "给下面这段对话起一个不超过 12 个字的简短标题, 概括在做的事情 (不要写结论), 使用对话的主要语言, 直接输出标题本身, 不要引号、句号或任何前缀:\n\n";

fn clean_title(s: &str) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > 48 {
        let mut t: String = chars[..48].iter().collect();
        t.push('…');
        t
    } else {
        joined
    }
}

/// message.content may be a string or a list of {type, text} objects.
fn first_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => {
            for c in a {
                if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                    if !t.is_empty() {
                        return t.to_string();
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

// Injected-content filter, shared with the message-stream parser.
use crate::messages::usable_user_text as usable_prompt;

/// Truncate to `n` characters (python `s[:n]` semantics).
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Run `claude -p --model haiku` with the prompt on stdin, 60s timeout.
/// Returns None on any failure (silent downgrade).
pub fn run_haiku(exe: &Path, prompt: &str) -> Option<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .args(["-p", "--model", "haiku"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
        // drop closes the pipe
    }
    let stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut out = String::new();
        let mut r = stdout;
        let _ = r.read_to_string(&mut out);
        let _ = tx.send(out);
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
    rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

/// Test hook signature: replaces the real `claude -p --model haiku` call.
pub type FakeHaiku = Box<dyn Fn(&str) -> Option<String> + Send>;

pub struct Namer<'a> {
    pub paths: &'a crate::paths::Paths,
    pub enabled: bool,
    /// test hook: when set, used instead of invoking the real `claude` binary
    pub fake_haiku: Option<FakeHaiku>,
}

impl<'a> Namer<'a> {
    pub fn new(paths: &'a crate::paths::Paths, enabled: bool) -> Self {
        Namer { paths, enabled, fake_haiku: None }
    }

    fn ai_summary(&self, cache: &mut CwdCache, fnm: &Path, excerpt: &str) -> String {
        if !self.enabled || excerpt.is_empty() {
            return String::new();
        }
        let key = format!("ainame:{}", fnm.display());
        let (sz, _) = fstat(fnm);
        if let Some(Value::Array(a)) = cache.get(&key) {
            if a.len() == 2 {
                let cached_sz = a[0].as_f64().unwrap_or(-1.0);
                if (sz as f64) <= cached_sz * 1.3 + 4096.0 {
                    return a[1].as_str().unwrap_or("").to_string();
                }
            }
        }
        let prompt = format!("{HAIKU_PROMPT_PREFIX}{}", take_chars(excerpt, 4000));
        let out = if let Some(fake) = &self.fake_haiku {
            fake(&prompt)
        } else {
            match crate::agents::which("claude", &self.paths.home) {
                Some(exe) => run_haiku(&exe, &prompt),
                None => return String::new(),
            }
        };
        let Some(out) = out else { return String::new() };
        let name = out
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches(|c| matches!(c, '"' | '“' | '”' | '。'))
            .to_string();
        if name.is_empty() || name.chars().count() > 80 {
            return String::new();
        }
        cache.put(key, serde_json::json!([sz, name]));
        name
    }

    fn claude_excerpt(&self, fnm: &Path) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut n = 0;
        for o in jsonl_head(fnm, 200) {
            if o.get("type").and_then(|t| t.as_str()) == Some("user")
                && !o.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                let t = first_text(o.get("message").and_then(|m| m.get("content")));
                if usable_prompt(&t) {
                    parts.push(format!("用户: {}", take_chars(&t, 400)));
                    n += 1;
                    if n >= 2 {
                        break;
                    }
                }
            }
        }
        let mut lu = String::new();
        let mut la = String::new();
        for o in jsonl_tail(fnm, 65536).iter().rev() {
            let m = o.get("message");
            if la.is_empty() && o.get("type").and_then(|t| t.as_str()) == Some("assistant") {
                let t = first_text(m.and_then(|m| m.get("content")));
                if !t.is_empty() && !t.starts_with('<') {
                    la = t;
                }
            }
            if lu.is_empty()
                && o.get("type").and_then(|t| t.as_str()) == Some("user")
                && !o.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false)
            {
                let t = first_text(m.and_then(|m| m.get("content")));
                if usable_prompt(&t) {
                    lu = t;
                }
            }
            if !la.is_empty() && !lu.is_empty() {
                break;
            }
        }
        if !lu.is_empty() {
            let candidate = format!("用户: {}", take_chars(&lu, 400));
            if !parts.contains(&candidate) {
                parts.push(format!("用户(最近): {}", take_chars(&lu, 400)));
            }
        }
        if !la.is_empty() {
            parts.push(format!("助手(最近): {}", take_chars(&la, 400)));
        }
        parts.join("\n")
    }

    pub fn claude_name(&self, cache: &mut CwdCache, fnm: &Path) -> String {
        let key = format!("cname2:{}", fnm.display());
        let (_, mt) = fstat(fnm);
        if let Some(Value::Array(a)) = cache.get(&key) {
            if a.len() == 2 && a[0].as_f64() == Some(mt) {
                return a[1].as_str().unwrap_or("").to_string();
            }
        }
        let tail = jsonl_tail(fnm, 65536);
        let mut name = String::new();
        for o in tail.iter().rev() {
            if let Some(t) = o.get("customTitle").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    name = t.to_string();
                    break;
                }
            }
        }
        if name.is_empty() {
            // CC-generated short title; latest rewrite wins, fall back to head
            for o in tail.iter().rev() {
                if o.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                    if let Some(t) = o.get("aiTitle").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            name = t.to_string();
                            break;
                        }
                    }
                }
            }
            if name.is_empty() {
                for o in jsonl_head(fnm, 300) {
                    if o.get("type").and_then(|t| t.as_str()) == Some("ai-title") {
                        if let Some(t) = o.get("aiTitle").and_then(|v| v.as_str()) {
                            if !t.is_empty() {
                                name = t.to_string(); // no break: last one wins (AAA_PY behaviour)
                            }
                        }
                    }
                }
            }
        }
        if name.is_empty() {
            for o in tail.iter().rev() {
                if o.get("type").and_then(|t| t.as_str()) == Some("summary") {
                    if let Some(t) = o.get("summary").and_then(|v| v.as_str()) {
                        if !t.is_empty() {
                            name = t.to_string();
                            break;
                        }
                    }
                }
            }
        }
        if name.is_empty() {
            let excerpt = self.claude_excerpt(fnm);
            name = self.ai_summary(cache, fnm, &excerpt);
        }
        if name.is_empty() {
            for o in jsonl_head(fnm, 200) {
                if o.get("type").and_then(|t| t.as_str()) == Some("user")
                    && !o.get("isSidechain").and_then(|v| v.as_bool()).unwrap_or(false)
                {
                    let t = first_text(o.get("message").and_then(|m| m.get("content")));
                    if usable_prompt(&t) {
                        name = t;
                        break;
                    }
                }
            }
        }
        if name.is_empty() {
            // slug (lowest information) last
            'outer: for src in [&tail, &jsonl_head(fnm, 80)] {
                for o in src.iter() {
                    if let Some(s) = o.get("slug").and_then(|v| v.as_str()) {
                        if !s.is_empty() {
                            name = s.to_string();
                            break 'outer;
                        }
                    }
                }
            }
        }
        let name = if name.is_empty() { String::new() } else { clean_title(&name) };
        cache.put(key, serde_json::json!([mt, name]));
        name
    }

    /// SESSION_NAMER dispatch table.
    pub fn name_for(&self, cache: &mut CwdCache, agent: &str, path: &Path) -> String {
        match agent {
            "claude" => self.claude_name(cache, path),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;

    fn setup() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        (dir, paths)
    }

    fn write_jsonl(p: &Path, lines: &[Value]) {
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn priority_chain() {
        let (dir, paths) = setup();
        let cache_path = dir.path().join("cache.json");
        let namer = Namer::new(&paths, false); // haiku off

        // customTitle wins over everything
        let f = dir.path().join("a.jsonl");
        write_jsonl(
            &f,
            &[
                serde_json::json!({"type":"user","message":{"content":"first prompt"}}),
                serde_json::json!({"type":"summary","summary":"a summary"}),
                serde_json::json!({"type":"ai-title","aiTitle":"AI Title"}),
                serde_json::json!({"customTitle":"My Custom"}),
            ],
        );
        let mut cache = CwdCache::load(&cache_path);
        assert_eq!(namer.claude_name(&mut cache, &f), "My Custom");

        // ai-title beats summary beats first message
        let f2 = dir.path().join("b.jsonl");
        write_jsonl(
            &f2,
            &[
                serde_json::json!({"type":"user","message":{"content":"first prompt"}}),
                serde_json::json!({"type":"summary","summary":"a summary"}),
                serde_json::json!({"type":"ai-title","aiTitle":"AI Title"}),
            ],
        );
        assert_eq!(namer.claude_name(&mut cache, &f2), "AI Title");

        let f3 = dir.path().join("c.jsonl");
        write_jsonl(
            &f3,
            &[
                serde_json::json!({"type":"user","message":{"content":"first prompt"}}),
                serde_json::json!({"type":"summary","summary":"a summary"}),
            ],
        );
        assert_eq!(namer.claude_name(&mut cache, &f3), "a summary");

        // first user message; skips injected/caveat/slash content
        let f4 = dir.path().join("d.jsonl");
        write_jsonl(
            &f4,
            &[
                serde_json::json!({"type":"user","message":{"content":"<environment_context>x</environment_context>"}}),
                serde_json::json!({"type":"user","message":{"content":"/compact"}}),
                serde_json::json!({"type":"user","isSidechain":true,"message":{"content":"sidechain"}}),
                serde_json::json!({"type":"user","message":{"content":[{"type":"input_text","text":"real question here"}]}}),
                serde_json::json!({"slug":"reactive-wobbling-tide"}),
            ],
        );
        assert_eq!(namer.claude_name(&mut cache, &f4), "real question here");

        // slug as last resort
        let f5 = dir.path().join("e.jsonl");
        write_jsonl(&f5, &[serde_json::json!({"slug":"reactive-wobbling-tide"})]);
        assert_eq!(namer.claude_name(&mut cache, &f5), "reactive-wobbling-tide");
    }

    #[test]
    fn cname2_cache_roundtrip_and_mtime_invalidations() {
        let (dir, paths) = setup();
        let cache_path = dir.path().join("cache.json");
        let f = dir.path().join("s.jsonl");
        write_jsonl(&f, &[serde_json::json!({"customTitle":"T1"})]);
        let namer = Namer::new(&paths, false);
        let mut cache = CwdCache::load(&cache_path);
        assert_eq!(namer.claude_name(&mut cache, &f), "T1");
        cache.save();
        // cached entry has aaa-compatible shape [mtime, name]
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&cache_path).unwrap()).unwrap();
        let ent = &raw[format!("cname2:{}", f.display())];
        assert!(ent[0].is_number());
        assert_eq!(ent[1], "T1");
        // simulate an aaa-CLI-written cache entry: it must be honoured
        let (_, mt) = fstat(&f);
        let mut cache2 = CwdCache::load(&cache_path);
        cache2.put(format!("cname2:{}", f.display()), serde_json::json!([mt, "来自CLI的标题"]));
        assert_eq!(namer.claude_name(&mut cache2, &f), "来自CLI的标题");
    }

    #[test]
    fn haiku_fallback_via_fake() {
        let (dir, paths) = setup();
        let cache_path = dir.path().join("cache.json");
        let f = dir.path().join("s.jsonl");
        write_jsonl(
            &f,
            &[serde_json::json!({"type":"user","message":{"content":"做一个交互原型"}})],
        );
        let mut namer = Namer::new(&paths, true);
        namer.fake_haiku = Some(Box::new(|prompt: &str| {
            assert!(prompt.contains("做一个交互原型"));
            Some("“原型设计”。\n".to_string())
        }));
        let mut cache = CwdCache::load(&cache_path);
        assert_eq!(namer.claude_name(&mut cache, &f), "原型设计");
        // cached under ainame: with [size, name]
        let key = format!("ainame:{}", f.display());
        let cached = cache.get(&key).unwrap();
        assert_eq!(cached[1], "原型设计");
    }

    #[test]
    fn clean_title_truncation() {
        let long: String = "字".repeat(60);
        let t = clean_title(&long);
        assert_eq!(t.chars().count(), 49);
        assert!(t.ends_with('…'));
        assert_eq!(clean_title("  a \t b  "), "a b");
    }

}
