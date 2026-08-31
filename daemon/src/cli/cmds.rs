//! Non-interactive verbs.
//!
//! The menu is for hands; these are for muscle memory and for scripts. Every
//! listing has a `--json` form, so `aaa` composes with jq the way the old
//! interactive-only script never could.

use std::io::Write;

use crate::attach::{self, Outcome};
use crate::client::{Client, Session};
use crate::render;
use crate::tty::{self, BOLD, CYAN, DIM, GREY, RED, RESET, YELLOW};

pub const HELP: &str = "\
aaa — AAA daemon 的命令行前端

  aaa                      交互菜单（活会话在上；数字选会话，p 项目 m 权限 n 新建）
  aaa ls [-a] [--json]     会话列表（-a 含已退出）
  aaa ps [--json]          项目列表
  aaa new [名字] [-a AGENT]  新建项目目录 + 开一个会话并接入
  aaa open <目标>           进入项目（有活会话就接回，否则 resume）
  aaa attach <目标>         接入会话（Ctrl-] 脱离，会话继续跑）
  aaa say <目标> <文本…>     把一句话写进会话并回车（回答提问用）
  aaa kill <目标>           结束会话
  aaa rm <目标>             删除会话记录
  aaa wait [--json]         只列等待输入的会话（脚本/通知用）
  aaa status [--json]       daemon 状态
  aaa perms [--json]        macOS 权限状态
  aaa perms all | <id…>     申请权限（弹窗出现在 Mac 上）
  aaa help

目标可以是：会话 id 或其前缀、aaa ls 里的序号、项目名（取最近的活会话）、
或 `.` 表示当前目录所属的项目。

连接：默认读 ~/.config/aaa-daemon/config.toml 连本机 daemon；
      设 AAA_HOST=主机:2730 AAA_TOKEN=… 可指向另一台机器的 daemon。
";

pub fn ls(c: &Client, all: bool, json: bool) -> Result<(), String> {
    if json {
        let v: serde_json::Value = c.get("/api/v1/sessions")?;
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    // Number over the *full* sorted list, then drop the exited tail. Exited
    // sessions always sort last, so a live row keeps the same number whether
    // or not `-a` is given — the number you read is the number you can type.
    let mut sessions = c.sessions()?;
    sort_for_listing(&mut sessions);
    if !all {
        sessions.retain(|s| s.is_live());
    }
    if sessions.is_empty() {
        println!("{DIM}没有会话{RESET}");
        return Ok(());
    }
    let (cols, _) = tty::size();
    for (i, s) in sessions.iter().enumerate() {
        println!(
            "{DIM}{:>2}{RESET} {}",
            i + 1,
            render::session_row(s, (cols as usize).saturating_sub(4))
        );
    }
    Ok(())
}

pub fn ps(c: &Client, json: bool) -> Result<(), String> {
    if json {
        let v: serde_json::Value = c.get("/api/v1/projects")?;
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    let projects = c.projects()?;
    let (cols, _) = tty::size();
    for (i, p) in projects.iter().enumerate() {
        println!("{DIM}{:>2}{RESET} {}", i + 1, render::project_row(p, (cols as usize).saturating_sub(4)));
    }
    Ok(())
}

pub fn wait(c: &Client, json: bool) -> Result<(), String> {
    let mut sessions = c.sessions()?;
    sessions.retain(|s| s.state == "waiting");
    if json {
        println!("{}", serde_json::to_string_pretty(&waiting_json(&sessions)).unwrap_or_default());
        return Ok(());
    }
    if sessions.is_empty() {
        println!("{DIM}没有会话在等你{RESET}");
        return Ok(());
    }
    for s in &sessions {
        println!("{YELLOW}●{RESET} {BOLD}{}{RESET}  {DIM}{}{RESET}", s.project_name, s.id);
        if let Some(q) = &s.question {
            if !q.text.is_empty() {
                println!("   {}", q.text);
            }
            for o in &q.options {
                println!("   {CYAN}{}{RESET} {}", display_key(&o.key), o.label);
            }
        }
    }
    Ok(())
}

fn waiting_json(sessions: &[Session]) -> serde_json::Value {
    serde_json::json!(sessions
        .iter()
        .map(|s| serde_json::json!({
            "id": s.id,
            "project": s.project_name,
            "path": s.project_path,
            "agent": s.agent,
            "question": s.question.as_ref().map(|q| q.text.clone()).unwrap_or_default(),
            "options": s.question.as_ref().map(|q| q.options.iter()
                .map(|o| serde_json::json!({"key": o.key, "label": o.label}))
                .collect::<Vec<_>>()).unwrap_or_default(),
        }))
        .collect::<Vec<_>>())
}

/// Arrow-key options carry escape sequences as their key; show something a
/// human can read instead of dumping ESC bytes into the terminal.
fn display_key(key: &str) -> String {
    if key.is_empty() {
        "·".into()
    } else if key.starts_with('\x1b') {
        "↓".repeat(key.matches('\x1b').count())
    } else {
        key.to_string()
    }
}

pub fn perms(c: &Client, json: bool) -> Result<(), String> {
    if json {
        let v: serde_json::Value = c.get("/api/v1/mac/permissions")?;
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    for p in c.permissions()? {
        let (color, label) = render::perm_status(&p.status);
        println!("  {} {color}{label}{RESET}  {DIM}{}{RESET}", tty::cell(&p.label, 22), p.id);
    }
    Ok(())
}

/// `ids` empty means "all". Prompts appear on the Mac; from a phone or an SSH
/// session you get the summary but somebody has to be at the keyboard.
/// "all" 同时触发 AAA.app 自己的弹窗——它是独立的 TCC 主体，daemon 的授权
/// 覆盖不了它（`open --args` 对已运行实例送不到参数，所以先礼貌退出再启动；
/// 会话都在 daemon 上，App 重启无损）。
pub fn perms_request(c: &Client, ids: &[String]) -> Result<(), String> {
    let all = ids.is_empty();
    let ids = if all { vec!["all".to_string()] } else { ids.to_vec() };
    let v = c.request_permissions(&ids)?;
    println!("daemon  {}", crate::menu::summarize_request(&v));
    if all && c.host_is_local() {
        let _ = std::process::Command::new("osascript")
            .args(["-e", "quit app \"AAA\""])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(800));
        let ok = std::process::Command::new("open")
            .args(["-a", "AAA", "--args", "--request-permissions"])
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if ok {
            println!("App     已触发 AAA.app 的弹窗（辅助功能 / 屏幕录制 / FDA 面板 / 测试通知）");
        } else {
            println!("App     AAA.app 未安装或启动失败，仅授了 daemon");
        }
    } else if all {
        println!("App     远程调用只授 daemon；AAA.app 的弹窗要在 Mac 本机跑 aaa perms all");
    }
    println!("{DIM}弹窗出现在 Mac 上，逐个允许即可；完全磁盘访问 / 相机 / 麦克风无法弹窗，已打开对应设置面板{RESET}");
    Ok(())
}

pub fn status(c: &Client, json: bool) -> Result<(), String> {
    let h = c.health()?;
    let sessions = c.sessions().unwrap_or_default();
    let live = sessions.iter().filter(|s| s.is_live()).count();
    let waiting = sessions.iter().filter(|s| s.state == "waiting").count();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "host": c.host, "port": c.port, "version": h.version,
                "ssd_mounted": h.ssd_mounted, "root_state": root_state(&h),
                "project_root": h.project_root,
                "uptime_s": h.uptime_s, "sessions": live, "waiting": waiting,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!("{BOLD}aaa-daemon{RESET} v{}  {DIM}{}:{}{RESET}", h.version, c.host, c.port);
    println!("  项目根   {} {}", h.project_root, root_note(&h));
    println!("  运行时长 {}", uptime(h.uptime_s));
    println!("  会话     {live} 活跃 · {waiting} 等待输入 · {} 总计", sessions.len());
    if root_state(&h) == "denied" {
        println!(
            "\n{YELLOW}项目根读不了{RESET}：launchd 起的 daemon 没有可移动卷访问权。\n  \
             系统设置 → 隐私与安全性 → 完全磁盘访问权限，加入 {DIM}~/.local/bin/aaa-daemon{RESET}，\n  \
             然后 {BOLD}aaa-daemon service uninstall && aaa-daemon service install{RESET}"
        );
    }
    Ok(())
}

/// Older daemons only had `ssd_mounted`; treat a missing field as the old
/// two-state world rather than inventing a diagnosis.
fn root_state(h: &crate::client::Health) -> &str {
    if h.root_state.is_empty() {
        if h.ssd_mounted {
            "ok"
        } else {
            "unmounted"
        }
    } else {
        &h.root_state
    }
}

fn root_note(h: &crate::client::Health) -> String {
    match root_state(h) {
        "ok" => String::new(),
        "denied" => format!("{RED}(读不了 · 缺完全磁盘访问权限){RESET}"),
        _ => format!("{RED}(未挂载!){RESET}"),
    }
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    if d > 0 {
        format!("{d}天 {h}小时")
    } else if h > 0 {
        format!("{h}小时 {m}分")
    } else {
        format!("{m}分")
    }
}

pub fn new_project(c: &Client, name: &str, agent: &str) -> Result<(), String> {
    let p = c.create_project(name, agent)?;
    let s = c.create_session(&p.path, agent, false)?;
    if tty::is_tty() {
        let _ = attach::attach(c, &s.id, &format!("{} · 新会话", p.name));
        println!("{DIM}{} · {}{RESET}", p.path, s.id);
    } else {
        println!("{}", s.id);
    }
    Ok(())
}

pub fn open(c: &Client, target: &str) -> Result<(), String> {
    // an existing live session wins; otherwise resume the project's agent
    if let Ok(s) = resolve(c, target) {
        return do_attach(c, &s);
    }
    let projects = c.projects()?;
    let want = normalize(target, c)?;
    let p = projects
        .iter()
        .find(|p| p.path == want || p.name == want)
        .or_else(|| projects.iter().find(|p| p.name.to_lowercase().contains(&want.to_lowercase())))
        .ok_or_else(|| format!("没有匹配 {target} 的项目"))?;
    let s = c.create_session(&p.path, &p.agent, true)?;
    do_attach(c, &s)
}

pub fn attach_cmd(c: &Client, target: &str) -> Result<(), String> {
    let s = resolve(c, target)?;
    do_attach(c, &s)
}

fn do_attach(c: &Client, s: &Session) -> Result<(), String> {
    let title = format!("{} · {}", s.project_name, s.title);
    match attach::attach(c, &s.id, &title)? {
        Outcome::Detached => println!("{DIM}已脱离 {}（仍在运行）{RESET}", s.id),
        Outcome::Ended => println!("{DIM}{} 已结束{RESET}", s.id),
    }
    Ok(())
}

pub fn say(c: &Client, target: &str, text: &str) -> Result<(), String> {
    let s = resolve(c, target)?;
    c.input(&s.id, text, true)?;
    println!("{DIM}→ {} · {}{RESET}", s.project_name, s.id);
    Ok(())
}

pub fn kill(c: &Client, target: &str) -> Result<(), String> {
    let (s, how) = resolve_for_destruction(c, target)?;
    if !confirm_destructive(&s, how, "结束")? {
        println!("{DIM}已取消{RESET}");
        return Ok(());
    }
    c.kill(&s.id)?;
    println!("已结束 {} · {}", s.project_name, s.id);
    Ok(())
}

pub fn remove(c: &Client, target: &str) -> Result<(), String> {
    let (s, how) = resolve_for_destruction(c, target)?;
    if !confirm_destructive(&s, how, "删除记录")? {
        println!("{DIM}已取消{RESET}");
        return Ok(());
    }
    c.remove(&s.id)?;
    println!("已删除记录 {} · {}", s.project_name, s.id);
    Ok(())
}

pub fn rename(c: &Client, target: &str, title: &str) -> Result<(), String> {
    let s = resolve(c, target)?;
    c.rename(&s.id, title)?;
    println!("{} → {title}", s.id);
    Ok(())
}

/// `.` means "the project this shell is standing in".
fn normalize(target: &str, c: &Client) -> Result<String, String> {
    if target != "." {
        return Ok(target.to_string());
    }
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let root = std::path::PathBuf::from(&c.health()?.project_root);
    let rel = cwd
        .strip_prefix(&root)
        .map_err(|_| format!("当前目录不在项目根 {} 下", root.display()))?;
    rel.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .ok_or_else(|| "当前目录就是项目根".to_string())
}

/// id → id prefix → `aaa ls` index → project name.
///
/// Everything resolves against the same order `ls` prints, over *all*
/// sessions including exited ones — `rm` exists precisely to clear an exited
/// record, so refusing to name one would be absurd. Live sessions sort first,
/// so a bare project name still reaches the running session.
pub fn resolve(c: &Client, target: &str) -> Result<Session, String> {
    let target = normalize(target, c)?;
    let mut ordered = c.sessions()?;
    sort_for_listing(&mut ordered);
    resolve_in(&ordered, &target).map(|(s, _)| s)
}

/// Same, but keeping how the session was named — for the verbs that end it.
fn resolve_for_destruction(c: &Client, target: &str) -> Result<(Session, Certainty), String> {
    let target = normalize(target, c)?;
    let mut ordered = c.sessions()?;
    sort_for_listing(&mut ordered);
    resolve_in(&ordered, &target)
}

/// Ask before ending something the user named by position.
///
/// `aaa kill 2` resolves against a list that reshuffles whenever an agent
/// prints; the row you read and the row you type are not guaranteed to be the
/// same session. So a positional target gets echoed and confirmed, and an id
/// goes straight through. Non-interactive callers (scripts, the phone) get no
/// prompt — they should be passing ids.
fn confirm_destructive(s: &Session, how: Certainty, verb: &str) -> Result<bool, String> {
    if how == Certainty::Exact || !tty::is_tty() {
        return Ok(true);
    }
    print!(
        "{verb} {BOLD}{}{RESET} · {} {DIM}{}{RESET} [y/N]: ",
        s.project_name,
        render::state_label(s),
        s.id
    );
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// How sure we are that the session we resolved is the one the user meant.
///
/// An id names a session for as long as it exists. A row number does not: the
/// listing re-sorts by `last_output_at`, so between `aaa ls` and `aaa kill 2`
/// any agent producing output can slide a different session into row 2. The
/// destructive verbs use this to decide whether to ask first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certainty {
    /// named by id (or a unique id prefix) — stable
    Exact,
    /// named by row number or a fuzzy project match — could have moved
    Positional,
}

fn resolve_in(ordered: &[Session], target: &str) -> Result<(Session, Certainty), String> {
    if let Some(s) = ordered.iter().find(|s| s.id == target) {
        return Ok((s.clone(), Certainty::Exact));
    }

    if let Ok(n) = target.parse::<usize>() {
        return ordered
            .get(n.wrapping_sub(1))
            .cloned()
            .map(|s| (s, Certainty::Positional))
            .ok_or_else(|| format!("序号 {n} 超出范围（当前 {} 个会话）", ordered.len()));
    }

    let by_prefix: Vec<&Session> = ordered.iter().filter(|s| s.id.starts_with(target)).collect();
    match by_prefix.len() {
        1 => return Ok((by_prefix[0].clone(), Certainty::Exact)),
        // ambiguity is an error, never a silent pick: killing the wrong
        // session is not recoverable
        n if n > 1 => return Err(format!("{target} 匹配到 {n} 个会话，请写全 id")),
        _ => {}
    }

    let lower = target.to_lowercase();
    // an exact project name is unambiguous enough to act on; live sessions
    // sort first, so this reaches the running one
    if let Some(s) = ordered
        .iter()
        .find(|s| s.project_name.to_lowercase() == lower || s.project_path == target)
    {
        return Ok((s.clone(), Certainty::Positional));
    }

    let fuzzy: Vec<&Session> = ordered
        .iter()
        .filter(|s| s.project_name.to_lowercase().contains(&lower))
        .collect();
    match fuzzy.len() {
        1 => Ok((fuzzy[0].clone(), Certainty::Positional)),
        // substring matches were guessing silently; refuse like prefixes do
        n if n > 1 => Err(format!(
            "{target} 模糊匹配到 {n} 个会话（{}），写全项目名或 id",
            fuzzy.iter().map(|s| s.project_name.as_str()).collect::<Vec<_>>().join(" ")
        )),
        _ => Err(format!("没有匹配 {target} 的会话（aaa ls -a 看看）")),
    }
}

/// The order every listing and every index refers to: waiting first, then
/// most recent output.
pub fn sort_for_listing(sessions: &mut [Session]) {
    sessions.sort_by(|a, b| {
        let rank = |s: &Session| match s.state.as_str() {
            "waiting" => 0,
            "exited" => 2,
            _ => 1,
        };
        rank(a)
            .cmp(&rank(b))
            .then_with(|| b.last_output_at.cmp(&a.last_output_at))
    });
}

pub fn die(msg: &str) -> ! {
    let _ = writeln!(std::io::stderr(), "{GREY}aaa:{RESET} {msg}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sess(id: &str, project: &str, state: &str, last: &str) -> Session {
        serde_json::from_value(serde_json::json!({
            "id": id, "project_name": project, "state": state, "last_output_at": last
        }))
        .unwrap()
    }

    #[test]
    fn listing_order_puts_waiting_first_and_exited_last() {
        let mut v = vec![
            sess("s_1", "a", "running", "2026-08-31T10:00:00Z"),
            sess("s_2", "b", "exited", "2026-08-31T23:00:00Z"),
            sess("s_3", "c", "waiting", "2026-08-31T01:00:00Z"),
            sess("s_4", "d", "idle", "2026-08-31T12:00:00Z"),
        ];
        sort_for_listing(&mut v);
        let ids: Vec<&str> = v.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["s_3", "s_4", "s_1", "s_2"]);
    }

    #[test]
    fn an_exited_session_is_still_addressable() {
        // `rm` exists to clear exactly these records; resolving only over live
        // sessions made the one verb that needs them unable to name them.
        let mut v = vec![
            sess("s_live", "alpha", "running", "2026-08-31T10:00:00Z"),
            sess("s_dead", "beta", "exited", "2026-08-31T09:00:00Z"),
        ];
        sort_for_listing(&mut v);
        assert_eq!(resolve_in(&v, "beta").unwrap().0.id, "s_dead");
        assert_eq!(resolve_in(&v, "s_dead").unwrap().0.id, "s_dead");
        assert_eq!(resolve_in(&v, "2").unwrap().0.id, "s_dead", "序号覆盖 exited");
    }

    #[test]
    fn a_project_name_reaches_the_live_session_first() {
        let mut v = vec![
            sess("s_old", "aaa", "exited", "2026-08-31T23:00:00Z"),
            sess("s_now", "aaa", "idle", "2026-08-31T01:00:00Z"),
        ];
        sort_for_listing(&mut v);
        assert_eq!(resolve_in(&v, "aaa").unwrap().0.id, "s_now", "活的优先，哪怕更旧");
    }

    #[test]
    fn ambiguous_id_prefix_refuses_rather_than_guesses() {
        let v = vec![
            sess("s_ab1", "a", "idle", ""),
            sess("s_ab2", "b", "idle", ""),
        ];
        assert!(resolve_in(&v, "s_ab").is_err());
        assert!(resolve_in(&v, "9").unwrap_err().contains("超出范围"));
        assert!(resolve_in(&v, "0").is_err(), "序号从 1 开始，0 不能回绕");
    }

    #[test]
    fn a_denied_root_is_not_reported_as_unmounted() {
        // the two need different fixes; saying "未挂载" sends you to the wrong place
        let denied: crate::client::Health = serde_json::from_value(serde_json::json!({
            "ssd_mounted": false, "root_state": "denied"
        }))
        .unwrap();
        assert_eq!(root_state(&denied), "denied");
        assert!(root_note(&denied).contains("完全磁盘访问"));
    }

    #[test]
    fn a_pre_1_0_daemon_still_reads_sensibly() {
        // no root_state field at all: fall back to the old two-state world
        let old: crate::client::Health =
            serde_json::from_value(serde_json::json!({"ssd_mounted": true})).unwrap();
        assert_eq!(root_state(&old), "ok");
        assert!(root_note(&old).is_empty());
        let gone: crate::client::Health =
            serde_json::from_value(serde_json::json!({"ssd_mounted": false})).unwrap();
        assert_eq!(root_state(&gone), "unmounted");
        assert!(root_note(&gone).contains("未挂载"));
    }

    #[test]
    fn an_id_is_certain_and_a_row_number_is_not() {
        // 序号解析的那份列表按 last_output_at 排序，两次命令之间任何 agent 有输出
        // 都会把别的会话滑进这一行——所以 kill/rm 对序号要先问一句。
        let mut v = vec![
            sess("s_one", "alpha", "running", "2026-08-31T10:00:00Z"),
            sess("s_two", "beta", "running", "2026-08-31T09:00:00Z"),
        ];
        sort_for_listing(&mut v);
        assert_eq!(resolve_in(&v, "s_one").unwrap().1, Certainty::Exact);
        assert_eq!(resolve_in(&v, "s_o").unwrap().1, Certainty::Exact, "唯一前缀也算确定");
        assert_eq!(resolve_in(&v, "1").unwrap().1, Certainty::Positional);
        assert_eq!(resolve_in(&v, "alpha").unwrap().1, Certainty::Positional);
    }

    #[test]
    fn an_ambiguous_substring_refuses_instead_of_guessing() {
        let v = vec![
            sess("s_1", "aaa-ui", "running", "2026-08-31T10:00:00Z"),
            sess("s_2", "aaa-daemon", "running", "2026-08-31T09:00:00Z"),
        ];
        let err = resolve_in(&v, "aaa-").unwrap_err();
        assert!(err.contains("模糊匹配到 2"), "{err}");
        assert_eq!(resolve_in(&v, "aaa-ui").unwrap().0.id, "s_1", "完整项目名仍然直达");
    }

    #[test]
    fn arrow_option_keys_render_readably() {
        assert_eq!(display_key("y"), "y");
        assert_eq!(display_key(""), "·");
        assert_eq!(display_key("\x1b[B"), "↓");
        assert_eq!(display_key("\x1b[B\x1b[B"), "↓↓");
    }

    #[test]
    fn uptime_reads_in_the_biggest_useful_unit() {
        assert_eq!(uptime(90), "1分");
        assert_eq!(uptime(3660), "1小时 1分");
        assert_eq!(uptime(90000), "1天 1小时");
    }

    #[test]
    fn waiting_json_carries_question_and_options() {
        let s: Session = serde_json::from_value(serde_json::json!({
            "id": "s_1", "project_name": "aaa", "state": "waiting",
            "question": {"text": "继续?", "options": [{"key":"y","label":"Yes"}]}
        }))
        .unwrap();
        let v = waiting_json(&[s]);
        assert_eq!(v[0]["question"], "继续?");
        assert_eq!(v[0]["options"][0]["key"], "y");
    }
}
