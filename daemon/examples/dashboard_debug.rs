//! 拿真实的 `~/.local/state/aaa-daemon/` 跑一遍看板聚合，只读不写：
//! `cargo run --example dashboard_debug`
use aaa_daemon::history::{Days, History, KEEP, dashboard, today_local};

fn main() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".local/state/aaa-daemon");
    let entries = History::load(&dir).list(KEEP);
    let days = Days::load(&dir);
    let d = dashboard(&entries, &days, &Default::default(), &Default::default(), &today_local());
    println!("今天 {:?}\n近7天 {:?}\n待办 {} 条", d.today, d.week, d.open.len());
    for i in d.open.iter().take(8) {
        println!("  ☐ [{}] {} · {}", i.project_name, i.text, i.title);
    }
    for day in d.days.iter().take(4) {
        println!("{} · {} 会话 · 做完 {} 没做 {}", day.date, day.sessions, day.done, day.open);
        for l in day.text.lines() {
            println!("   {l}");
        }
    }
    println!("spark {:?}", &d.spark[48..]);
}
