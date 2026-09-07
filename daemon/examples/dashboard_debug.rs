//! 拿真实的 `~/.local/state/aaa-daemon/history.json` 跑一遍看板聚合，只读不写
//! （池子里的状态拿不到，全按 paused）：`cargo run --example dashboard_debug`
use aaa_daemon::history::{History, KEEP, dashboard};

fn main() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/state/aaa-daemon");
    let entries = History::load(&dir).list(KEEP);
    let d = dashboard(&entries, &Default::default(), &Default::default());
    println!("{:?}", d.counts);
    for c in d.sessions.iter().take(8) {
        println!("  [{}] {} · {} · {}/{}  {}", c.status, c.project_name, c.title, c.done, c.done + c.open, c.items.iter().filter(|i| !i.done).map(|i| i.text.as_str()).take(2).collect::<Vec<_>>().join(" / "));
    }
}
