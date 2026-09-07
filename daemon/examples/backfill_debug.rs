//! 离线补清单：给 `~/.local/state/aaa-daemon/sessions/*.json` 里没有清单的 claude 会话按
//! transcript 生成一份（haiku），原子写回元数据文件。daemon 下次起来 restore 时就带着清单，
//! 会话日志随之同步，看板就有进度了。跑着的 daemon 只在那条会话变化时才会重写它的元数据
//! （已退出的很少变），所以现在跑是安全的。`--dry-run` 只找不写。
use aaa_daemon::paths::Paths;
use aaa_daemon::summary::{locate_transcript, summarize_transcript};

fn main() {
    let dry = std::env::args().any(|a| a == "--dry-run");
    let limit: usize = std::env::args().skip(1).find(|a| a.parse::<usize>().is_ok()).and_then(|a| a.parse().ok()).unwrap_or(500);
    let paths = Paths::from_env();
    let dir = paths.sessions_dir();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json")).collect();
    files.sort();
    let (mut found, mut done, mut missing) = (0, 0, 0);
    for f in files {
        if done >= limit { break; }
        let Ok(body) = std::fs::read(&f) else { continue };
        let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&body) else { continue };
        let get = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (agent, summary, rid, path, created, last, pname, title) = (get(&v, "agent"), get(&v, "summary"), get(&v, "resume_id"), get(&v, "project_path"), get(&v, "created_at"), get(&v, "last_output_at"), get(&v, "project_name"), get(&v, "title"));
        if agent != "claude" || !summary.trim().is_empty() { continue; }
        let Some(file) = locate_transcript(&paths, (!rid.is_empty()).then_some(rid.as_str()), &path, &created, &last) else {
            missing += 1;
            eprintln!("  {}: 找不到 transcript（{} · {}）", f.file_name().unwrap().to_string_lossy(), pname, title);
            continue;
        };
        found += 1;
        if dry { continue; }
        match summarize_transcript(&paths.home, &file) {
            Some(text) => {
                v["summary"] = serde_json::Value::String(text.clone());
                let out = serde_json::to_vec_pretty(&v).unwrap();
                aaa_daemon::paths::write_atomic(&f, &out).unwrap();
                done += 1;
                println!("✓ {} · {}\n{}\n", pname, title, text.lines().map(|l| "    ".to_string() + l).collect::<Vec<_>>().join("\n"));
            }
            None => eprintln!("  {}: haiku 没出东西（{}）", f.file_name().unwrap().to_string_lossy(), title),
        }
    }
    println!("--- 找到 transcript {found} · 补上 {done} · 找不到 {missing}");
}
