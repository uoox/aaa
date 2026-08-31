//! Debug tool mirroring `aaa_py <cmd> ...`: exercises the ported store logic
//! against `AAA_HOME`. Used for cross-validation with the aaa CLI's python.
//!
//!   cargo run --example store_debug -- find <agent> <cwd>
//!   cargo run --example store_debug -- detect <cwd>
//!   cargo run --example store_debug -- collect <root>
//!   cargo run --example store_debug -- purge <cwd>
//!   cargo run --example store_debug -- cname <claude-jsonl>

use aaa_daemon::cache::CwdCache;
use aaa_daemon::namer::Namer;
use aaa_daemon::paths::Paths;
use aaa_daemon::stores;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::from_env();
    let mut cache = CwdCache::load(&paths.cwd_cache());
    match (args.first().map(String::as_str), args.get(1), args.get(2)) {
        (Some("find"), Some(agent), Some(cwd)) => {
            let sid = stores::find(&paths, &mut cache, agent, cwd);
            if !sid.is_empty() {
                println!("{sid}");
            }
        }
        (Some("detect"), Some(cwd), _) => {
            let a = stores::detect(&paths, &mut cache, cwd);
            if !a.is_empty() {
                println!("{a}");
            }
        }
        (Some("collect"), Some(root), _) => {
            for r in stores::collect(&paths, &mut cache, std::path::Path::new(root)) {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    r.path,
                    r.name,
                    r.dir_size,
                    r.ctx_size.map(|c| c.to_string()).unwrap_or_else(|| "-".into()),
                    r.det_agent.as_deref().unwrap_or("-"),
                );
            }
        }
        (Some("purge"), Some(cwd), _) => {
            for (label, n) in stores::purge(&paths, &mut cache, cwd) {
                println!("{label}\t{n}");
            }
        }
        (Some("cname"), Some(f), _) => {
            let namer = Namer::new(&paths, std::env::var("AAA_NAMER").as_deref() != Ok("off"));
            println!("{}", namer.claude_name(&mut cache, std::path::Path::new(f)));
        }
        _ => {
            eprintln!("usage: find <agent> <cwd> | detect <cwd> | collect <root> | purge <cwd> | cname <file>");
            std::process::exit(2);
        }
    }
    cache.save();
}
