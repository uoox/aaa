//! 拿一份真实数据的**副本**演练整根迁移（只动副本，AAA_HOME 指向临时目录）：
//! `AAA_HOME=/tmp/x cargo run --example migrate_debug -- <old_root> <new_root>`
//! `--stores-only`：不搬目录，只对副本里的存储做改指向（old/new 用真实路径）
use aaa_daemon::{migrate, paths::Paths};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let stores_only = args.iter().position(|a| a == "--stores-only").map(|i| args.remove(i)).is_some();
    let (old, new) = (std::path::PathBuf::from(&args[0]), std::path::PathBuf::from(&args[1]));
    let paths = Paths::from_env();
    if stores_only {
        println!("{}", serde_json::to_string_pretty(&migrate::rewrite_stores(&paths, &old, &new)).unwrap());
        return;
    }
    match migrate::migrate_root(&paths, &old, &new) {
        Ok(rep) => println!("{}", serde_json::to_string_pretty(&rep).unwrap()),
        Err(e) => {
            eprintln!("FAILED: {e}");
            std::process::exit(1);
        }
    }
}
