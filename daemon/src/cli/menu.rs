//! The interactive menu — `aaa` with no arguments.
//!
//! Same shape as the zsh menu it replaces (banner, arrow keys, digit
//! shortcuts, `q` to leave), with one structural change: live sessions are
//! listed above the actions. In the daemon era the most common thing you want
//! is not "start something", it is "get back to the thing that is waiting for
//! me".

use std::io::Write;

use crate::attach::{self, Outcome};
use crate::client::{AgentInfo, Client, Project, Session};
use crate::render;
use crate::tty::{self, Key, Raw, BOLD, CYAN, DIM, EOL, GREY, MAGENTA, RESET, YELLOW};

/// Idle redraw cadence: fast enough that a session going amber shows up
/// while you are looking at the list, cheap enough to leave running.
const REFRESH_MS: i32 = 1500;

enum Item {
    Session(usize),
    Projects,
    Perms,
    NewAgent(usize),
}

pub fn run(c: &Client) -> Result<(), String> {
    if !tty::is_tty() {
        return Err("交互菜单需要终端；试试 aaa ls / aaa help".into());
    }
    let health = c.health()?;
    let agents: Vec<AgentInfo> = c.agents().unwrap_or_default();
    let raw = Raw::enter(true).ok_or("无法进入 raw 模式")?;
    tty::clear();

    let mut idx = 0usize;
    let mut sessions = c.sessions().unwrap_or_default();
    let mut status = String::new();
    loop {
        live_only(&mut sessions);
        let items = build_items(&sessions, &agents);
        if idx >= items.len() {
            idx = items.len().saturating_sub(1);
        }
        draw(&sessions, &agents, &items, idx, &health, c, &status);

        match tty::read_key(REFRESH_MS) {
            None => {
                sessions = c.sessions().unwrap_or(sessions);
                continue;
            }
            Some(Key::Up) => idx = if idx == 0 { items.len() - 1 } else { idx - 1 },
            Some(Key::Down) => idx = (idx + 1) % items.len(),
            Some(Key::Char(d @ '1'..='9')) => {
                let n = d as usize - '1' as usize;
                if n < items.len() {
                    idx = n;
                    status = activate(c, &raw, &items[idx], &sessions, &agents)?;
                    sessions = c.sessions().unwrap_or_default();
                    tty::clear();
                }
            }
            Some(Key::Enter) => {
                status = activate(c, &raw, &items[idx], &sessions, &agents)?;
                sessions = c.sessions().unwrap_or_default();
                tty::clear();
            }
            Some(Key::Char('k')) => {
                if let Item::Session(i) = items[idx] {
                    let s = &sessions[i];
                    if confirm(&raw, &format!("结束会话 {} · {}?", s.project_name, s.title)) {
                        status = match c.kill(&s.id) {
                            Ok(_) => format!("已结束 {}", s.project_name),
                            Err(e) => e,
                        };
                    }
                    sessions = c.sessions().unwrap_or_default();
                    tty::clear();
                }
            }
            Some(Key::Char('r')) => {
                sessions = c.sessions().unwrap_or_default();
                status.clear();
            }
            Some(Key::Char('q')) | Some(Key::Esc) | Some(Key::Ctrl('c')) => break,
            _ => {}
        }
    }
    drop(raw);
    tty::clear();
    Ok(())
}

fn live_only(sessions: &mut Vec<Session>) {
    sessions.retain(|s| s.is_live());
    // waiting first (that is the row that wants a human), then most recent
    sessions.sort_by(|a, b| {
        let rank = |s: &Session| if s.state == "waiting" { 0 } else { 1 };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_output_at.cmp(&a.last_output_at))
    });
}

fn build_items(sessions: &[Session], agents: &[AgentInfo]) -> Vec<Item> {
    let mut items: Vec<Item> = (0..sessions.len()).map(Item::Session).collect();
    items.push(Item::Projects);
    items.push(Item::Perms);
    items.extend((0..agents.len()).map(Item::NewAgent));
    items
}

fn draw(
    sessions: &[Session],
    agents: &[AgentInfo],
    items: &[Item],
    idx: usize,
    health: &crate::client::Health,
    c: &Client,
    status: &str,
) {
    let (cols, _) = tty::size();
    let w = cols as usize;
    let mut out = String::new();
    tty::home();
    out.push_str(&format!("{BOLD}{MAGENTA}"));
    out.push_str("   ░█▀█░█▀█░█▀█\n");
    out.push_str(&format!(
        "   ░█▀█░█▀█░█▀█{RESET}{DIM}    {}:{} · v{} · {}{EOL}\n",
        c.host,
        c.port,
        health.version,
        if health.ssd_mounted { "SSD 已挂载" } else { "SSD 未挂载" }
    ));
    out.push_str(&format!("{BOLD}{MAGENTA}   ░▀░▀░▀░▀░▀░▀{RESET}{EOL}\n\n"));

    let waiting = sessions.iter().filter(|s| s.state == "waiting").count();
    let head = if sessions.is_empty() {
        format!("{DIM}会话（无）{RESET}")
    } else if waiting > 0 {
        format!("{DIM}会话 {} 个 · {YELLOW}{waiting} 个等待输入{RESET}", sessions.len())
    } else {
        format!("{DIM}会话 {} 个{RESET}", sessions.len())
    };
    out.push_str(&format!("  {head}{EOL}\n"));

    for (n, item) in items.iter().enumerate() {
        let sel = n == idx;
        let marker = if sel {
            format!("{BOLD}{CYAN}▶{RESET}")
        } else {
            " ".into()
        };
        let num = if n < 9 {
            format!("{DIM}{}{RESET}", n + 1)
        } else {
            " ".into()
        };
        let body = match item {
            Item::Session(i) => render::session_row(&sessions[*i], w.saturating_sub(6)),
            Item::Projects => {
                out.push_str(&format!("{EOL}\n  {DIM}新建{RESET}{EOL}\n"));
                format!("{BOLD}项目管理{RESET}  {DIM}{}{RESET}", health.project_root)
            }
            Item::Perms => format!("{BOLD}macOS 权限{RESET}  {DIM}一次点完，之后手机远程不再被弹窗卡住{RESET}"),
            Item::NewAgent(i) => {
                let a = &agents[*i];
                let mark = if a.available { "" } else { " (未安装)" };
                format!(
                    "New {}{}{}{}",
                    render::agent_color(&a.id),
                    a.label,
                    RESET,
                    format_args!("{GREY}{mark}{RESET}")
                )
            }
        };
        out.push_str(&format!(" {marker} {num} {body}{EOL}\n"));
    }

    out.push_str(&format!("{EOL}\n"));
    if !status.is_empty() {
        out.push_str(&format!("  {CYAN}{}{RESET}{EOL}\n", tty::truncate(status, w - 4)));
    }
    out.push_str(&format!(
        "  {DIM}↑↓ 移动 · Enter 进入 · 数字直选 · k 结束会话 · r 刷新 · q 退出{RESET}{EOL}\x1b[J"
    ));
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// Act on the highlighted row. Returns a one-line status for the next frame.
fn activate(
    c: &Client,
    raw: &Raw,
    item: &Item,
    sessions: &[Session],
    agents: &[AgentInfo],
) -> Result<String, String> {
    match item {
        Item::Session(i) => {
            let s = &sessions[*i];
            let title = format!("{} · {}", s.project_name, s.title);
            match attach::attach(c, &s.id, &title) {
                Ok(Outcome::Detached) => Ok(format!("已脱离 {}（仍在运行）", s.project_name)),
                Ok(Outcome::Ended) => Ok(format!("{} 会话已结束", s.project_name)),
                Err(e) => Ok(e),
            }
        }
        Item::Projects => project_menu(c, raw),
        Item::Perms => perms_menu(c, raw),
        Item::NewAgent(i) => new_project(c, raw, &agents[*i].id),
    }
}

// ---------- macOS 权限 ----------

/// The permission screen. Everything here happens in the *daemon's* process —
/// the CLI only asks — so the dialogs land on the Mac and the grants apply to
/// every agent the daemon spawns, whoever started it.
fn perms_menu(c: &Client, raw: &Raw) -> Result<String, String> {
    let mut perms = match c.permissions() {
        Ok(p) => p,
        Err(e) => return Ok(e),
    };
    let mut idx = 0usize;
    let mut status = String::new();
    tty::clear();
    loop {
        if idx >= perms.len() {
            idx = perms.len().saturating_sub(1);
        }
        tty::home();
        let mut out = format!("  {BOLD}macOS 权限{RESET}{EOL}\n");
        out.push_str(&format!(
            "  {DIM}授权归到 daemon；agent 都是它的子进程，点一次全线共享{RESET}{EOL}\n\n"
        ));
        for (n, p) in perms.iter().enumerate() {
            let marker = if n == idx { format!("{BOLD}{CYAN}▶{RESET}") } else { " ".into() };
            let (color, label) = render::perm_status(&p.status);
            out.push_str(&format!(
                " {marker} {DIM}{}{RESET} {}  {color}{label}{RESET}{EOL}\n",
                n + 1,
                tty::cell(&p.label, 22)
            ));
        }
        out.push_str(&format!("{EOL}\n"));
        if !status.is_empty() {
            out.push_str(&format!("  {CYAN}{status}{RESET}{EOL}\n"));
        }
        out.push_str(&format!(
            "  {DIM}Enter 申请这一项 · a 一键申请全部 · r 刷新 · q 返回{RESET}{EOL}\x1b[J"
        ));
        print!("{out}");
        let _ = std::io::stdout().flush();

        let request = |ids: Vec<String>| -> String {
            match c.request_permissions(&ids) {
                Ok(v) => summarize_request(&v),
                Err(e) => e,
            }
        };
        match tty::read_key(-1) {
            Some(Key::Up) => idx = if idx == 0 { perms.len().saturating_sub(1) } else { idx - 1 },
            Some(Key::Down) => {
                if !perms.is_empty() {
                    idx = (idx + 1) % perms.len()
                }
            }
            Some(Key::Enter) => {
                if !perms.is_empty() {
                    status = request(vec![perms[idx].id.clone()]);
                    perms = c.permissions().unwrap_or(perms);
                }
            }
            Some(Key::Char('a')) => {
                if confirm(raw, "弹窗会出现在这台 Mac 上，逐个允许。开始?") {
                    status = request(vec!["all".to_string()]);
                    perms = c.permissions().unwrap_or(perms);
                }
                tty::clear();
            }
            Some(Key::Char('r')) => {
                perms = c.permissions().unwrap_or(perms);
                status.clear();
            }
            Some(Key::Char('q')) | Some(Key::Esc) | Some(Key::Ctrl('c')) => {
                tty::clear();
                return Ok(status);
            }
            _ => {}
        }
    }
}

/// The request endpoint answers with what it prompted for and what it could
/// only open a settings pane for — the second list is the actionable half.
pub fn summarize_request(v: &serde_json::Value) -> String {
    let list = |key: &str| {
        v.get(key)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str()).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    let triggered = list("triggered");
    let opened = list("opened_settings");
    let mut parts = Vec::new();
    if !triggered.is_empty() {
        parts.push(format!("已弹窗: {triggered}"));
    }
    if !opened.is_empty() {
        parts.push(format!("已打开设置面板（需手动开）: {opened}"));
    }
    if parts.is_empty() {
        "没有需要申请的项".into()
    } else {
        parts.join(" · ")
    }
}

// ---------- 项目管理 ----------

fn project_menu(c: &Client, raw: &Raw) -> Result<String, String> {
    tty::clear();
    print!("  {DIM}读取项目…{RESET}");
    let _ = std::io::stdout().flush();
    let mut projects = match c.projects() {
        Ok(p) => p,
        Err(e) => return Ok(e),
    };
    let mut idx = 0usize;
    let mut status = String::new();
    tty::clear();
    loop {
        if projects.is_empty() {
            tty::clear();
            println!("  {DIM}还没有项目。n 新建，q 返回{RESET}");
        } else {
            if idx >= projects.len() {
                idx = projects.len() - 1;
            }
            draw_projects(&projects, idx, &status);
        }
        match tty::read_key(-1) {
            Some(Key::Up) => idx = if idx == 0 { projects.len().saturating_sub(1) } else { idx - 1 },
            Some(Key::Down) => {
                if !projects.is_empty() {
                    idx = (idx + 1) % projects.len()
                }
            }
            Some(Key::Char(d @ '1'..='9')) => {
                let n = d as usize - '1' as usize;
                if n < projects.len() {
                    idx = n;
                    status = open_project(c, &projects[idx])?;
                    tty::clear();
                }
            }
            Some(Key::Enter) => {
                if !projects.is_empty() {
                    status = open_project(c, &projects[idx])?;
                    tty::clear();
                }
            }
            Some(Key::Char('a')) => {
                if !projects.is_empty() {
                    if let Some(a) = pick_agent(c, raw, "换 agent") {
                        status = match c.set_agent(&projects[idx].path, &a) {
                            Ok(_) => {
                                projects[idx].agent = a.clone();
                                format!("{} → {a}", projects[idx].name)
                            }
                            Err(e) => e,
                        };
                    }
                    tty::clear();
                }
            }
            Some(Key::Char('d')) => {
                if !projects.is_empty() {
                    let p = projects[idx].clone();
                    if confirm(raw, &format!("删除 {} 及其全部 agent 会话记录?", p.name)) {
                        status = match c.delete_projects(std::slice::from_ref(&p.path)) {
                            Ok(v) => {
                                projects.remove(idx);
                                purge_summary(&p.name, &v)
                            }
                            Err(e) => e,
                        };
                    }
                    tty::clear();
                }
            }
            Some(Key::Char('n')) => {
                if let Some(a) = pick_agent(c, raw, "新项目用哪个 agent") {
                    status = new_project(c, raw, &a)?;
                    projects = c.projects().unwrap_or(projects);
                    tty::clear();
                }
            }
            Some(Key::Char('r')) => {
                projects = c.projects().unwrap_or(projects);
                status.clear();
            }
            Some(Key::Char('q')) | Some(Key::Esc) | Some(Key::Ctrl('c')) => {
                tty::clear();
                return Ok(status);
            }
            _ => {}
        }
    }
}

fn draw_projects(projects: &[Project], idx: usize, status: &str) {
    let (cols, rows) = tty::size();
    let w = cols as usize;
    let mut out = String::new();
    tty::home();
    out.push_str(&format!("  {BOLD}项目管理{RESET}  {DIM}共 {} 个{RESET}{EOL}\n\n", projects.len()));
    // leave room for header, hint and status
    let cap = (rows as usize).saturating_sub(8).max(5);
    let start = idx.saturating_sub(cap - 1);
    for (n, p) in projects.iter().enumerate().skip(start).take(cap) {
        let marker = if n == idx {
            format!("{BOLD}{CYAN}▶{RESET}")
        } else {
            " ".into()
        };
        let num = if n < 9 {
            format!("{DIM}{}{RESET}", n + 1)
        } else {
            " ".into()
        };
        out.push_str(&format!(" {marker} {num} {}{EOL}\n", render::project_row(p, w.saturating_sub(6))));
    }
    out.push_str(&format!("{EOL}\n"));
    if !status.is_empty() {
        out.push_str(&format!("  {CYAN}{}{RESET}{EOL}\n", tty::truncate(status, w - 4)));
    }
    out.push_str(&format!(
        "  {DIM}Enter 进入 · a 换 agent · n 新建 · d 删除 · r 刷新 · q 返回{RESET}{EOL}\x1b[J"
    ));
    print!("{out}");
    let _ = std::io::stdout().flush();
}

/// Enter a project: re-attach a live session if it has one, otherwise ask the
/// daemon to resume the agent's most recent session in that directory (the
/// aaa CLI's behaviour, now surviving the terminal).
fn open_project(c: &Client, p: &Project) -> Result<String, String> {
    let live = c
        .sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.is_live() && s.project_path == p.path)
        .max_by(|a, b| a.last_output_at.cmp(&b.last_output_at));
    let sess = match live {
        Some(s) => s,
        None => match c.create_session(&p.path, &p.agent, true) {
            Ok(s) => s,
            Err(e) => return Ok(e),
        },
    };
    let title = format!("{} · {}", p.name, sess.title);
    Ok(match attach::attach(c, &sess.id, &title) {
        Ok(Outcome::Detached) => format!("已脱离 {}（仍在运行）", p.name),
        Ok(Outcome::Ended) => format!("{} 会话已结束", p.name),
        Err(e) => e,
    })
}

/// New project = new folder + a fresh session, no git anything. Projects are
/// often just a task directory, so nothing here assumes a repo.
fn new_project(c: &Client, raw: &Raw, agent: &str) -> Result<String, String> {
    let Some(name) = prompt(raw, "项目名（留空=时间戳）") else {
        tty::clear();
        return Ok(String::new());
    };
    let p = match c.create_project(name.trim(), agent) {
        Ok(p) => p,
        Err(e) => {
            tty::clear();
            return Ok(e);
        }
    };
    let sess = match c.create_session(&p.path, agent, false) {
        Ok(s) => s,
        Err(e) => {
            tty::clear();
            return Ok(e);
        }
    };
    let title = format!("{} · 新会话", p.name);
    Ok(match attach::attach(c, &sess.id, &title) {
        Ok(Outcome::Detached) => format!("已建 {} 并脱离（仍在运行）", p.name),
        Ok(Outcome::Ended) => format!("{} 会话已结束", p.name),
        Err(e) => e,
    })
}

fn pick_agent(c: &Client, _raw: &Raw, title: &str) -> Option<String> {
    let agents = c.agents().unwrap_or_default();
    if agents.is_empty() {
        return None;
    }
    let labels: Vec<String> = agents
        .iter()
        .map(|a| {
            format!(
                "{}{}{RESET}{}",
                render::agent_color(&a.id),
                a.label,
                if a.available { String::new() } else { format!("{GREY} (未安装){RESET}") }
            )
        })
        .collect();
    select(title, &labels).map(|i| agents[i].id.clone())
}

/// A plain list picker for the small sub-menus.
fn select(title: &str, items: &[String]) -> Option<usize> {
    let mut idx = 0usize;
    tty::clear();
    loop {
        tty::home();
        let mut out = format!("  {BOLD}{title}{RESET}{EOL}\n\n");
        for (n, it) in items.iter().enumerate() {
            let marker = if n == idx { format!("{BOLD}{CYAN}▶{RESET}") } else { " ".into() };
            out.push_str(&format!(" {marker} {DIM}{}{RESET} {it}{EOL}\n", n + 1));
        }
        out.push_str(&format!("{EOL}\n  {DIM}↑↓ · Enter 选择 · q 取消{RESET}{EOL}\x1b[J"));
        print!("{out}");
        let _ = std::io::stdout().flush();
        match tty::read_key(-1) {
            Some(Key::Up) => idx = if idx == 0 { items.len() - 1 } else { idx - 1 },
            Some(Key::Down) => idx = (idx + 1) % items.len(),
            Some(Key::Enter) => return Some(idx),
            Some(Key::Char(d @ '1'..='9')) => {
                let n = d as usize - '1' as usize;
                if n < items.len() {
                    return Some(n);
                }
            }
            Some(Key::Char('q')) | Some(Key::Esc) | Some(Key::Ctrl('c')) => return None,
            _ => {}
        }
    }
}

/// Line input in cooked mode — backspace, kill-word and IME composition all
/// come free from the line discipline, which matters for Chinese names.
fn prompt(raw: &Raw, label: &str) -> Option<String> {
    raw.cooked(|| {
        tty::clear();
        print!("  {BOLD}{label}{RESET}: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => None,
            Ok(_) => Some(line.trim_end_matches(['\n', '\r']).to_string()),
            Err(_) => None,
        }
    })
}

fn confirm(raw: &Raw, question: &str) -> bool {
    raw.cooked(|| {
        print!("\n  {YELLOW}{question}{RESET} [y/N]: ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes")
    })
}

fn purge_summary(name: &str, v: &serde_json::Value) -> String {
    let mut parts = vec![format!("已删除 {name}")];
    if let Some(results) = v.get("results").and_then(|r| r.as_array()) {
        for r in results {
            if let Some(purged) = r.get("purged").and_then(|p| p.as_array()) {
                for p in purged {
                    let label = p.get("agent_label").and_then(|l| l.as_str()).unwrap_or("");
                    let count = p.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                    if count > 0 {
                        parts.push(format!("{label} {count} 条"));
                    }
                }
            }
        }
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, state: &str, last: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "id": id, "state": state, "last_output_at": last, "project_name": "p"
        }))
        .unwrap()
    }

    #[test]
    fn waiting_sessions_float_to_the_top() {
        let mut v = vec![
            sess("a", "running", "2026-08-31T10:00:00Z"),
            sess("b", "waiting", "2026-08-31T09:00:00Z"),
            sess("c", "idle", "2026-08-31T11:00:00Z"),
            sess("d", "exited", "2026-08-31T12:00:00Z"),
        ];
        live_only(&mut v);
        let ids: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"], "等待优先，其余按最近输出，exited 不进菜单");
    }

    #[test]
    fn items_cover_sessions_then_actions() {
        let sessions = vec![sess("a", "idle", "")];
        let agents: Vec<AgentInfo> =
            serde_json::from_value(serde_json::json!([{"id":"claude"},{"id":"shell"}])).unwrap();
        let items = build_items(&sessions, &agents);
        assert_eq!(items.len(), 5);
        assert!(matches!(items[0], Item::Session(0)));
        assert!(matches!(items[1], Item::Projects));
        assert!(matches!(items[2], Item::Perms));
        assert!(matches!(items[4], Item::NewAgent(1)));
    }

    #[test]
    fn request_summary_separates_prompts_from_manual_panes() {
        let v = serde_json::json!({
            "triggered": ["accessibility", "screen_recording"],
            "opened_settings": ["full_disk_access"]
        });
        let s = summarize_request(&v);
        assert!(s.contains("accessibility screen_recording"));
        assert!(s.contains("full_disk_access"));
        assert_eq!(summarize_request(&serde_json::json!({})), "没有需要申请的项");
    }

    #[test]
    fn purge_summary_lists_each_agent() {
        let v = serde_json::json!({"results":[{"path":"/p","ok":true,"purged":[
            {"agent_label":"Claude","count":3},{"agent_label":"Codex","count":0}]}]});
        assert_eq!(purge_summary("demo", &v), "已删除 demo · Claude 3 条");
    }
}
