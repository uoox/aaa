//! 项目根整体迁移（`PUT /config {project_root, migrate:true}`）：把 `<old>` 整个
//! rename 到 `<new>`，并让**所有会话跟着过去**——注册表、daemon 自己的会话元数据 /
//! 会话日志 / 置顶、cwd 缓存，以及 Claude Code 那边按 cwd 命名的 transcript 目录
//! （`~/.claude/projects/<slug>/`，连里面 jsonl 的 `cwd` 字段一起改）和 `~/.claude.json`
//! 的 `projects` 键。2026-09-07 之前只改注册表，迁完 daemon 里每条会话、每条历史都
//! 还指着旧路径，Claude 也在旧 slug 目录下找不到「这个目录的对话」。
//!
//! 顺序：先采对话 id 进注册表 → 注册表键改新前缀（原子写，失败整体中止）→ 整根
//! rename（同卷原子；失败回滚注册表）→ 之后的重写全是 best-effort，逐项记进
//! `Report.warnings`，因为根已经搬了，没有回头路，能改多少改多少，剩下的说清楚。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::cache::CwdCache;
use crate::paths::Paths;
use crate::registry::Registry;
use crate::stores;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub session_metas: usize,
    pub history: usize,
    pub pins: usize,
    pub cwd_cache: usize,
    pub claude_dirs: usize,
    pub claude_files: usize,
    pub claude_json: usize,
    pub warnings: Vec<String>,
}

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.trim_end_matches('/').to_string()
}

/// `p` 在旧根下 → 换成新根下的同一相对路径；不在 → None。等于旧根本身也算。
pub fn reroot(p: &str, old: &Path, new: &Path) -> Option<String> {
    let (o, n) = (norm(old), norm(new));
    if p == o {
        return Some(n);
    }
    p.strip_prefix(&format!("{o}/")).map(|rest| format!("{n}/{rest}"))
}

/// Claude Code 给 `~/.claude/projects/` 下目录起名的规则：路径里每个非 ASCII 字母数字
/// 的字符换成 `-`（`/Volumes/SSD/project/aaa继续升级` → `-Volumes-SSD-project-aaa----`；
/// 中文每个字一个 `-`，所以不同目录会撞名，Claude 自己也这么过）。
pub fn claude_slug(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// 迁移前的便宜检查——**先于收会话调用**：这里能拒绝的（跨卷、目标非空、嵌套、
/// 拼错路径）都是「注定失败」，不该先把人家跑着的会话杀掉再发现（gpt-6 审阅指出）。
pub fn preflight(old: &Path, new: &Path) -> Result<(), String> {
    if !old.is_dir() {
        return Err(format!("旧项目根不存在：{}", old.display()));
    }
    if new == old {
        return Err("新旧目录相同".into());
    }
    if new.starts_with(old) || old.starts_with(new) {
        return Err("新目录不能嵌套在旧目录内（或反之）".into());
    }
    if new.exists() {
        let empty = std::fs::read_dir(new).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty {
            return Err(format!("目标已存在且非空：{}", new.display()));
        }
    }
    // 同卷才能 rename；跨卷明说手动拷
    let Some(parent) = new.parent().filter(|p| p.is_dir()) else {
        return Err(format!("目标的上级目录不存在：{}", new.parent().map(|p| p.display().to_string()).unwrap_or_default()));
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (a, b) = (std::fs::metadata(old).map_err(|e| e.to_string())?, std::fs::metadata(parent).map_err(|e| e.to_string())?);
        if a.dev() != b.dev() {
            return Err(format!("{} 与 {} 不在同一个卷上，rename 做不到；请手动 cp 后只改配置（migrate=false）", old.display(), parent.display()));
        }
    }
    Ok(())
}

/// Move the project root wholesale and keep every project resumable.
/// Caller guarantees no live sessions. Same-volume only (`rename`).
pub fn migrate_root(paths: &Paths, old: &Path, new: &Path) -> Result<Report, String> {
    preflight(old, new)?;
    // ① 迁移前把每个项目当前的对话 id 落进注册表：迁走后 agent 存储按旧
    //    cwd 查不到会话，id 只能现在采
    let _reg_lock = crate::registry::lock();
    let mut reg = Registry::load(old);
    let mut cache = CwdCache::load(&paths.cwd_cache());
    let mut collected: Vec<(String, String, String)> = Vec::new();
    for (dir, agent, _id) in reg.entries() {
        if agent == "shell" || !Path::new(&dir).starts_with(old) {
            continue;
        }
        // 一律现采：注册表里的 id 只在经 daemon resume 时回写过，用户可能
        // 之后用 aaal / 裸 agent 在该目录开过更新的对话。find 落空才留旧 id。
        let sid = stores::find(paths, &mut cache, &dir);
        if !sid.is_empty() {
            collected.push((dir, agent, sid));
        }
    }
    // 一次 flush 落盘。这一步失败必须中止：rename 之后就没有回头路了，
    // id 会永远丢在旧根的存储结构里
    reg.set_ids(&collected)
        .map_err(|e| format!("采集对话 id 失败（未做任何移动）: {e}"))?;
    // ② 先把注册表键改成新前缀——趁文件还在旧根，这一步是原子写，失败就
    //    整体中止，什么都没动过。指向新根的键在 is_live 眼里是「根外路径」，
    //    会被原样保留。（评审教训：搬完再重写，失败就只剩打日志一条路，
    //    而名册过滤会让整批项目从所有客户端消失。）
    reg.rewrite_prefix(old, new)
        .map_err(|e| format!("注册表预重写失败（未做任何移动）: {e}"))?;
    // ③ 整根 rename（同卷原子）。目标若已存在必须是空目录——不删它，
    //    rename(2) 本来就能原子替换空目录；非空直接拒绝。跨卷不装聪明——
    //    rename 会失败，明说手动拷。
    if let Err(e) = std::fs::rename(old, new) {
        // 根没动，把注册表键改回旧前缀；这次重写失败的概率极低（同一文件
        // 刚刚才成功原子写过），仍失败也只是 resume 兜底受损，根与配置一致。
        if let Err(e2) = Registry::load(old).rewrite_prefix(new, old) {
            eprintln!("migrate: 回滚注册表失败（根未移动，仅影响 resume 兜底）: {e2}");
        }
        return Err(format!("移动失败（跨卷迁移请手动 cp 后仅改配置）: {e}"));
    }
    // ④ 根已经在新地方了。下面每一步都 best-effort，失败记 warning 不中止
    Ok(rewrite_stores(paths, old, new))
}

/// 迁根之后各处存储的改指向（不搬目录）。单独暴露是为了拿真实数据的副本演练
/// （`examples/migrate_debug.rs --stores-only`）——rename 那一步没法在副本上演。
pub fn rewrite_stores(paths: &Paths, old: &Path, new: &Path) -> Report {
    let mut rep = Report::default();
    rewrite_state(paths, old, new, &mut rep);
    rewrite_claude_store(paths, old, new, &mut rep);
    rewrite_cwd_cache(paths, old, new, &mut rep);
    rep
}

fn warn(rep: &mut Report, what: &str, e: impl std::fmt::Display) {
    let msg = format!("{what}: {e}");
    eprintln!("migrate: {msg}");
    rep.warnings.push(msg);
}

/// 一个 JSON 值里所有落在旧根下的字符串换成新根（对象 / 数组递归）。返回改了几处。
fn reroot_value(v: &mut Value, old: &Path, new: &Path) -> usize {
    match v {
        Value::String(s) => match reroot(s, old, new) {
            Some(n) => {
                *s = n;
                1
            }
            None => 0,
        },
        Value::Array(a) => a.iter_mut().map(|x| reroot_value(x, old, new)).sum(),
        Value::Object(m) => m.values_mut().map(|x| reroot_value(x, old, new)).sum(),
        _ => 0,
    }
}

fn read_json(p: &Path) -> Result<Value, String> {
    let body = std::fs::read(p).map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

/// 原子写回，权限照旧：临时文件**先**改成原文件的权限再 rename（Claude 的文件是
/// 0600，含 token / 对话；用 write_atomic 那种「写完再 chmod」会有一瞬是 0644，
/// chmod 失败还会永久留成 0644——gpt-6 审阅指出）。chmod 失败就报错不 rename。
fn write_json_like(p: &Path, body: &[u8]) -> std::io::Result<()> {
    let perm = std::fs::metadata(p)?.permissions();
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    let tmp = p.with_file_name(format!("{name}.tmp"));
    std::fs::write(&tmp, body)?;
    if let Err(e) = std::fs::set_permissions(&tmp, perm) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, p)
}

/// daemon 自己的状态：会话元数据（`sessions/*.json` 的 project_path）、会话日志、置顶
fn rewrite_state(paths: &Paths, old: &Path, new: &Path, rep: &mut Report) {
    // 会话元数据：逐文件改 project_path。当 Value 改，不走 Meta 结构体——老版本
    // 多出来的字段也原样保留
    let sdir = paths.sessions_dir();
    match std::fs::read_dir(&sdir) {
        Err(e) if sdir.exists() => warn(rep, "会话元数据目录", e),
        Err(_) => {}
        Ok(rd) => for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let mut v = match read_json(&p) {
                Ok(v) => v,
                Err(err) => {
                    warn(rep, &format!("会话元数据 {} 读不了，仍指向旧路径", p.display()), err);
                    continue;
                }
            };
            let Some(pp) = v.get("project_path").and_then(|x| x.as_str()) else { continue };
            let Some(np) = reroot(pp, old, new) else { continue };
            v["project_path"] = Value::String(np);
            match serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()).and_then(|b| write_json_like(&p, &b).map_err(|e| e.to_string())) {
                Ok(()) => rep.session_metas += 1,
                Err(err) => warn(rep, &format!("会话元数据 {}", p.display()), err),
            }
        },
    }
    // 会话日志 history.json：[{project_path, …}]
    let hist = paths.state_dir().join("history.json");
    if hist.is_file() {
        match read_json(&hist) {
            Ok(mut v) => {
                let mut n = 0;
                if let Some(arr) = v.as_array_mut() {
                    for e in arr.iter_mut() {
                        if let Some(pp) = e.get("project_path").and_then(|x| x.as_str()) {
                            if let Some(np) = reroot(pp, old, new) {
                                e["project_path"] = Value::String(np);
                                n += 1;
                            }
                        }
                    }
                }
                if n > 0 {
                    match serde_json::to_vec(&v).map_err(|e| e.to_string()).and_then(|b| write_json_like(&hist, &b).map_err(|e| e.to_string())) {
                        Ok(()) => rep.history = n,
                        Err(err) => warn(rep, "会话日志", err),
                    }
                }
            }
            Err(err) => warn(rep, "会话日志", err),
        }
    }
    // 置顶 pins.json：["/path", …]
    let pins = paths.state_dir().join("pins.json");
    if pins.is_file() {
        match read_json(&pins) {
            Ok(mut v) => {
                let n = reroot_value(&mut v, old, new);
                if n > 0 {
                    match serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()).and_then(|b| write_json_like(&pins, &b).map_err(|e| e.to_string())) {
                        Ok(()) => rep.pins = n,
                        Err(err) => warn(rep, "置顶", err),
                    }
                }
            }
            Err(err) => warn(rep, "置顶", err),
        }
    }
}

/// Claude Code 的会话存储：`~/.claude/projects/<slug(cwd)>/` 目录改名到新 slug，
/// 里面 jsonl 的 `"cwd":"<old>…"` 改成新根；`~/.claude.json` 的 `projects` 键同步。
/// `claude --resume <id>` 本身是全局按 id 找的（实测：换目录也能 resume，且写回原
/// 文件不复制），改这些是为了 daemon 按 cwd 找「这个目录最近的对话」、Claude 在新
/// 目录下 `/resume` 能列出老对话、memory 跟着项目走，以及 allowedTools 之类的
/// 每项目设置不丢。
fn rewrite_claude_store(paths: &Paths, old: &Path, new: &Path, rep: &mut Report) {
    let root = paths.claude_root();
    let (old_s, new_s) = (norm(old), norm(new));
    let old_slug = claude_slug(&old_s);
    let rd = match std::fs::read_dir(&root) {
        Ok(rd) => rd,
        Err(e) => {
            if root.exists() {
                warn(rep, "Claude 存储目录", e);
            }
            return;
        }
    };
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()).map(String::from) else { continue };
        // 这个目录对应哪个 cwd：先看里面 jsonl 的 cwd 字段（准确），没有 jsonl 的
        // （只剩 memory 的目录）退到名字前缀匹配（slug 有损，只能这样）
        let cwd = match first_cwd(&dir) {
            Ok(c) => c,
            Err(e) => {
                // 读不了就不动它（宁可漏改也别改错），但要说出来
                if name == old_slug || name.starts_with(&format!("{old_slug}-")) {
                    warn(rep, &format!("Claude 目录 {name} 读不了，未改名"), e);
                }
                continue;
            }
        };
        let target = match &cwd {
            Some(c) => {
                // 自检：我们的 slug 规则算出来必须等于 Claude 起的名，否则说明规则
                // 猜错了，不动它（改错名比不改更糟）。大小写不比：APFS 不分大小写，
                // 谁先建的目录用谁的大小写（实测 RapidLine淘气 的目录在磁盘上是 rapidline--）
                if !claude_slug(c).eq_ignore_ascii_case(&name) {
                    if reroot(c, old, new).is_some() {
                        warn(rep, &format!("Claude 目录 {name}"), format!("slug 规则对不上 cwd {c}，跳过"));
                    }
                    continue;
                }
                match reroot(c, old, new) {
                    Some(n) => n,
                    None => continue,
                }
            }
            None => {
                if name == old_slug {
                    new_s.clone()
                } else if let Some(rest) = name.strip_prefix(&format!("{old_slug}-")) {
                    // 名字前缀匹配到的：按「旧根 + 余下」拼回路径只为算新 slug
                    format!("{new_s}/{rest}")
                } else {
                    continue;
                }
            }
        };
        let new_name = claude_slug(&target);
        let dest = root.join(&new_name);
        // 不分大小写的卷上「已存在」可能就是自己（只差大小写）：那是改名不是合并
        let same_dir = dest.exists()
            && std::fs::canonicalize(&dest).ok() == std::fs::canonicalize(&dir).ok();
        if dest.exists() && !same_dir {
            // 撞名（中文目录本来就会撞）：把内容并进去，别覆盖
            if let Err(e) = merge_dir(&dir, &dest) {
                warn(rep, &format!("Claude 目录 {name} → {new_name} 合并"), e);
                continue;
            }
        } else if let Err(e) = std::fs::rename(&dir, &dest) {
            warn(rep, &format!("Claude 目录 {name} → {new_name}"), e);
            continue;
        }
        rep.claude_dirs += 1;
        rep.claude_files += rewrite_cwd_fields(&dest, &old_s, &new_s, rep);
    }
    // ~/.claude.json 的 projects 键：allowedTools / 信任 / 每项目历史都挂在这
    let cj = paths.home.join(".claude.json");
    if cj.is_file() {
        match read_json(&cj) {
            Ok(mut v) => {
                let mut n = 0;
                if let Some(projects) = v.get_mut("projects").and_then(|p| p.as_object_mut()) {
                    let keys: Vec<String> = projects.keys().cloned().collect();
                    for k in keys {
                        let Some(nk) = reroot(&k, old, new) else { continue };
                        if let Some(val) = projects.remove(&k) {
                            // 新键已存在（之前在新路径下跑过）就留新的，旧的丢
                            projects.entry(nk).or_insert(val);
                            n += 1;
                        }
                    }
                }
                if n > 0 {
                    match serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()).and_then(|b| write_json_like(&cj, &b).map_err(|e| e.to_string())) {
                        Ok(()) => rep.claude_json = n,
                        Err(err) => warn(rep, "~/.claude.json", err),
                    }
                }
            }
            Err(err) => warn(rep, "~/.claude.json", err),
        }
    }
}

/// 目录里第一条能读到的 `cwd`（顶层 jsonl 的头 120 行）。`Err` = 目录 / 文件读不了，
/// 与「读了但没有 cwd」（`Ok(None)`）分开——后者才允许退到名字前缀匹配。
fn first_cwd(dir: &Path) -> Result<Option<String>, String> {
    let rd = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    for f in files {
        std::fs::File::open(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        for o in stores::jsonl_head(&f, 120) {
            if let Some(c) = o.get("cwd").and_then(|c| c.as_str()) {
                if !c.is_empty() {
                    return Ok(Some(c.to_string()));
                }
            }
        }
    }
    Ok(None)
}

/// `src` 里的东西挪进已存在的 `dst`（撞名合并）。同名目录递归合并；同名文件
/// 保留 dst 的，src 的改名成 `<名>.migrated<后缀>` 放旁边——不丢数据（memory 的
/// MEMORY.md 就会撞），也不覆盖新目录里正在用的。
fn merge_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    for e in std::fs::read_dir(src)?.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        // symlink 当普通文件整个挪走，绝不顺着它递归到树外（gpt-6 审阅指出）
        let from_dir = std::fs::symlink_metadata(&from)?.is_dir();
        let to_dir = std::fs::symlink_metadata(&to).map(|m| m.is_dir()).unwrap_or(false);
        if std::fs::symlink_metadata(&to).is_err() {
            std::fs::rename(&from, &to)?;
            continue;
        }
        if from_dir && to_dir {
            merge_dir(&from, &to)?;
            continue;
        }
        let stem = from.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
        let ext = from.extension().and_then(|s| s.to_str()).map(|x| format!(".{x}")).unwrap_or_default();
        let mut alt = dst.join(format!("{stem}.migrated{ext}"));
        let mut n = 1;
        while alt.exists() {
            n += 1;
            alt = dst.join(format!("{stem}.migrated{n}{ext}"));
        }
        std::fs::rename(&from, &alt)?;
    }
    std::fs::remove_dir(src)
}

/// 目录下（含一层子目录里的）每个 jsonl：`"cwd":"<old>` 后面紧跟 `"` 或 `/` 的都换成
/// `"cwd":"<new>`。只认这个紧凑格式——Claude 写 transcript 就是这样；工具输出里被
/// 转义或带空格的副本不碰。返回改过的文件数。
fn rewrite_cwd_fields(dir: &Path, old_s: &str, new_s: &str, rep: &mut Report) -> usize {
    let mut n = 0;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth < 2 {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            match rewrite_cwd_in_file(&p, old_s, new_s) {
                Ok(true) => n += 1,
                Ok(false) => {}
                Err(err) => warn(rep, &format!("transcript {}", p.display()), err),
            }
        }
    }
    n
}

fn rewrite_cwd_in_file(p: &Path, old_s: &str, new_s: &str) -> std::io::Result<bool> {
    use std::io::{BufRead, Write};
    // 逐行流式改写，不把整个 transcript 读进内存（几百 MB 的对话真有）。
    // 先跑一遍只找不写：绝大多数文件根本不含旧路径（memory / 子代理），不必碰。
    let needle = format!("\"cwd\":\"{old_s}\"");
    let needle_slash = format!("\"cwd\":\"{old_s}/");
    let hit = |line: &[u8]| find(line, needle.as_bytes()).is_some() || find(line, needle_slash.as_bytes()).is_some();
    let f = std::fs::File::open(p)?;
    let perm = f.metadata()?.permissions();
    let mut any = false;
    {
        let mut r = std::io::BufReader::new(&f);
        let mut line = Vec::new();
        while r.read_until(b'\n', &mut line)? > 0 {
            if hit(&line) {
                any = true;
                break;
            }
            line.clear();
        }
    }
    if !any {
        return Ok(false);
    }
    let name = p.file_name().unwrap_or_default().to_string_lossy();
    let tmp = p.with_file_name(format!("{name}.tmp"));
    let out = std::fs::File::create(&tmp)?;
    std::fs::set_permissions(&tmp, perm)?;
    let mut w = std::io::BufWriter::new(out);
    let mut r = std::io::BufReader::new(std::fs::File::open(p)?);
    let mut line = Vec::new();
    let rep_plain = format!("\"cwd\":\"{new_s}\"");
    let rep_slash = format!("\"cwd\":\"{new_s}/");
    while r.read_until(b'\n', &mut line)? > 0 {
        let l1 = replace_all(&line, needle_slash.as_bytes(), rep_slash.as_bytes());
        let l2 = replace_all(&l1, needle.as_bytes(), rep_plain.as_bytes());
        w.write_all(&l2)?;
        line.clear();
    }
    w.flush()?;
    drop(w);
    std::fs::rename(&tmp, p)?;
    Ok(true)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

fn replace_all(hay: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    while i < hay.len() {
        match find(&hay[i..], needle) {
            Some(off) => {
                out.extend_from_slice(&hay[i..i + off]);
                out.extend_from_slice(with);
                i += off + needle.len();
            }
            None => {
                out.extend_from_slice(&hay[i..]);
                break;
            }
        }
    }
    out
}

/// `~/.cache/aaa-cwds.json`：键里 Claude 目录的旧 slug 换新（文件搬过了），值里旧根
/// 下的路径换新根。不重建——`cname2:` 里存着 haiku 起的名字，丢了要花钱重起。
fn rewrite_cwd_cache(paths: &Paths, old: &Path, new: &Path, rep: &mut Report) {
    let file = paths.cwd_cache();
    if !file.is_file() {
        return;
    }
    let Ok(v) = read_json(&file) else {
        warn(rep, "cwd 缓存", "不是合法 JSON，跳过（会自动重建）");
        return;
    };
    let Value::Object(map) = v else { return };
    let old_dir = paths.claude_root().join(claude_slug(&norm(old)));
    let new_dir = paths.claude_root().join(claude_slug(&norm(new)));
    let (od, nd) = (old_dir.to_string_lossy().into_owned(), new_dir.to_string_lossy().into_owned());
    let mut next = serde_json::Map::new();
    let mut n = 0;
    for (k, mut val) in map {
        // 键：`<kind>:<file>`，file 在 ~/.claude/projects/<slug>/… 下
        let nk = match k.split_once(':') {
            Some((kind, path)) => {
                let np = if path == od {
                    Some(nd.clone())
                } else {
                    path.strip_prefix(&format!("{od}-")).map(|rest| format!("{nd}-{rest}"))
                };
                match np {
                    Some(p) => {
                        n += 1;
                        format!("{kind}:{p}")
                    }
                    None => k,
                }
            }
            None => k,
        };
        n += reroot_value(&mut val, old, new);
        next.insert(nk, val);
    }
    if n == 0 {
        return;
    }
    match serde_json::to_vec(&Value::Object(next)).map_err(|e| e.to_string()).and_then(|b| write_json_like(&file, &b).map_err(|e| e.to_string())) {
        Ok(()) => rep.cwd_cache = n,
        Err(err) => warn(rep, "cwd 缓存", err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_matches_claude_code() {
        assert_eq!(claude_slug("/Volumes/SSD/project/aaa继续升级"), "-Volumes-SSD-project-aaa----");
        assert_eq!(claude_slug("/Volumes/SSD/project/https30million.love"), "-Volumes-SSD-project-https30million-love");
        assert_eq!(claude_slug("/Volumes/SSD/Agents/aaaproject"), "-Volumes-SSD-Agents-aaaproject");
    }

    #[test]
    fn reroot_is_prefix_exact() {
        let (o, n) = (Path::new("/a/old"), Path::new("/b/new"));
        assert_eq!(reroot("/a/old", o, n).as_deref(), Some("/b/new"));
        assert_eq!(reroot("/a/old/x/y", o, n).as_deref(), Some("/b/new/x/y"));
        assert_eq!(reroot("/a/older/x", o, n), None, "同前缀不同目录不算");
        assert_eq!(reroot("/elsewhere", o, n), None);
    }

    #[test]
    fn migrate_root_moves_everything_and_rewrites_all_stores() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let paths = Paths::new(&home);
        let old = base.path().join("proj-old");
        let new = base.path().join("proj-new");
        std::fs::create_dir_all(old.join("alpha")).unwrap();
        std::fs::write(old.join("alpha/file.txt"), "x").unwrap();
        let alpha_old = old.join("alpha").to_string_lossy().into_owned();
        let alpha_new = new.join("alpha").to_string_lossy().into_owned();
        std::fs::write(
            Registry::registry_path(&old),
            format!("{alpha_old}\tclaude\tid-42\n/Volumes/Other/x\tcodex\n"),
        )
        .unwrap();

        // Claude 存储：alpha 的 transcript（cwd 字段 + 子目录 + memory）、一个只剩
        // memory 的目录、一个根外的目录
        let slug_old = claude_slug(&alpha_old);
        let cdir = paths.claude_root().join(&slug_old);
        std::fs::create_dir_all(cdir.join("memory")).unwrap();
        std::fs::create_dir_all(cdir.join("sub")).unwrap();
        let line = format!("{{\"type\":\"user\",\"cwd\":\"{alpha_old}\",\"text\":\"path {alpha_old}/f\"}}\n{{\"cwd\":\"{alpha_old}/nested\"}}\n{{\"note\":\"\\\"cwd\\\": \\\"{alpha_old}\\\" escaped copy stays\"}}\n");
        std::fs::write(cdir.join("id-42.jsonl"), &line).unwrap();
        std::fs::write(cdir.join("sub/agent-1.jsonl"), &line).unwrap();
        std::fs::write(cdir.join("memory/MEMORY.md"), "- m").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(cdir.join("id-42.jsonl"), std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let memory_only = paths.claude_root().join(format!("{}-beta", claude_slug(&old.to_string_lossy())));
        std::fs::create_dir_all(memory_only.join("memory")).unwrap();
        let foreign = paths.claude_root().join("-Volumes-Other-x");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("f.jsonl"), "{\"cwd\":\"/Volumes/Other/x\"}\n").unwrap();
        std::fs::write(
            home.join(".claude.json"),
            format!("{{\"projects\":{{\"{alpha_old}\":{{\"allowedTools\":[\"Bash\"]}},\"/Volumes/Other/x\":{{}}}},\"numStartups\":3}}"),
        )
        .unwrap();

        // daemon 状态
        std::fs::create_dir_all(paths.sessions_dir()).unwrap();
        std::fs::write(
            paths.sessions_dir().join("s_1.json"),
            format!("{{\"title\":\"t\",\"project_path\":\"{alpha_old}\",\"project_name\":\"alpha\",\"agent\":\"claude\",\"extra_field\":1}}"),
        )
        .unwrap();
        std::fs::write(
            paths.state_dir().join("history.json"),
            format!("[{{\"id\":\"s_1\",\"project_path\":\"{alpha_old}\"}},{{\"id\":\"s_9\",\"project_path\":\"/Volumes/Other/x\"}}]"),
        )
        .unwrap();
        std::fs::write(paths.state_dir().join("pins.json"), format!("[\"{alpha_old}\",\"/Volumes/Other/x\"]")).unwrap();
        std::fs::create_dir_all(paths.cwd_cache().parent().unwrap()).unwrap();
        std::fs::write(
            paths.cwd_cache(),
            format!(
                "{{\"claude:{}\":\"{alpha_old}\",\"cname2:{}\":[1.0,\"名字\"],\"claude:/elsewhere/z.jsonl\":\"/z\"}}",
                cdir.join("id-42.jsonl").display(),
                cdir.join("id-42.jsonl").display()
            ),
        )
        .unwrap();

        let rep = migrate_root(&paths, &old, &new).unwrap();

        assert!(!old.exists(), "旧根整体移走");
        assert!(new.join("alpha/file.txt").is_file(), "内容随根移动");
        let reg = Registry::load(&new);
        assert_eq!(reg.get(&alpha_new), Some("claude"));
        assert_eq!(reg.get_id(&alpha_new), Some("id-42"), "对话 id 存活");
        assert_eq!(reg.get("/Volumes/Other/x"), Some("codex"), "外部条目不动");

        // Claude 目录改名、cwd 改写、memory 跟着走、根外目录不动、转义副本不碰、权限照旧
        let cnew = paths.claude_root().join(claude_slug(&alpha_new));
        assert!(cnew.is_dir() && !cdir.exists(), "slug 目录改名");
        assert!(cnew.join("memory/MEMORY.md").is_file());
        let body = std::fs::read_to_string(cnew.join("id-42.jsonl")).unwrap();
        assert!(body.contains(&format!("\"cwd\":\"{alpha_new}\"")));
        assert!(body.contains(&format!("\"cwd\":\"{alpha_new}/nested\"")));
        assert!(body.contains(&format!("path {alpha_old}/f")), "正文里的旧路径不碰");
        assert!(body.contains(&format!("\\\"cwd\\\": \\\"{alpha_old}\\\"")), "转义副本不碰");
        assert!(std::fs::read_to_string(cnew.join("sub/agent-1.jsonl")).unwrap().contains(&alpha_new), "子目录里的也改");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(cnew.join("id-42.jsonl")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(paths.claude_root().join(format!("{}-beta", claude_slug(&new.to_string_lossy()))).is_dir(), "只剩 memory 的目录按前缀改名");
        assert!(foreign.join("f.jsonl").is_file(), "根外目录不动");
        let cj: Value = read_json(&home.join(".claude.json")).unwrap();
        assert_eq!(cj["projects"][&alpha_new]["allowedTools"][0], "Bash");
        assert!(cj["projects"].get(&alpha_old).is_none());
        assert_eq!(cj["numStartups"], 3, "其它字段原样");

        // daemon 状态
        let meta: Value = read_json(&paths.sessions_dir().join("s_1.json")).unwrap();
        assert_eq!(meta["project_path"], alpha_new);
        assert_eq!(meta["extra_field"], 1, "不认识的字段保留");
        let hist: Value = read_json(&paths.state_dir().join("history.json")).unwrap();
        assert_eq!(hist[0]["project_path"], alpha_new);
        assert_eq!(hist[1]["project_path"], "/Volumes/Other/x");
        let pins: Value = read_json(&paths.state_dir().join("pins.json")).unwrap();
        assert_eq!(pins[0], alpha_new);
        let cache: Value = read_json(&paths.cwd_cache()).unwrap();
        let nk = format!("claude:{}", cnew.join("id-42.jsonl").display());
        assert_eq!(cache[&nk], alpha_new, "缓存键跟着目录改名，值换新根");
        assert_eq!(cache[&format!("cname2:{}", cnew.join("id-42.jsonl").display())][1], "名字", "haiku 起的名不丢");
        assert_eq!(cache["claude:/elsewhere/z.jsonl"], "/z");

        assert_eq!(
            (rep.session_metas, rep.history, rep.pins, rep.claude_dirs, rep.claude_files, rep.claude_json),
            (1, 1, 1, 2, 2, 1)
        );
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);

        // 防呆：相同 / 嵌套 / 非空目标 / 上级不存在都在 preflight 就拒绝（收会话之前）
        assert!(preflight(&new, &new).is_err());
        assert!(preflight(&new, &new.join("inner")).is_err());
        let occupied = base.path().join("occupied");
        std::fs::create_dir_all(occupied.join("stuff")).unwrap();
        assert!(preflight(&new, &occupied).is_err());
        assert!(preflight(&new, &base.path().join("no-such-parent/x")).is_err());
        assert!(preflight(&new, &base.path().join("fresh")).is_ok());
        assert!(migrate_root(&paths, &new, &occupied).is_err());
        assert!(new.exists(), "拒绝时不动原目录");
    }

    #[test]
    fn unreadable_meta_is_reported_not_swallowed() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        let paths = Paths::new(&home);
        let old = base.path().join("o");
        let new = base.path().join("n");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(paths.sessions_dir()).unwrap();
        std::fs::write(paths.sessions_dir().join("s_bad.json"), "{not json").unwrap();
        let rep = migrate_root(&paths, &old, &new).unwrap();
        assert_eq!(rep.session_metas, 0);
        assert!(rep.warnings.iter().any(|w| w.contains("s_bad.json")), "{:?}", rep.warnings);
    }

    #[cfg(unix)]
    #[test]
    fn merge_never_follows_symlinks_and_rewrite_streams_without_trailing_newline() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        let paths = Paths::new(&home);
        let old = base.path().join("o");
        let new = base.path().join("n");
        std::fs::create_dir_all(old.join("p")).unwrap();
        let p_old = old.join("p").to_string_lossy().into_owned();
        let p_new = new.join("p").to_string_lossy().into_owned();
        let src = paths.claude_root().join(claude_slug(&p_old));
        let dst = paths.claude_root().join(claude_slug(&p_new));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        // src 里一个指向树外目录的 symlink；dst 里同名的是真目录：不能顺着链接把外面的东西搬走
        let outside = base.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, src.join("memory")).unwrap();
        std::fs::create_dir_all(dst.join("memory")).unwrap();
        // 没有结尾换行的 transcript：流式改写后照样没有
        std::fs::write(src.join("a.jsonl"), format!("{{\"cwd\":\"{p_old}\"}}\n{{\"cwd\":\"{p_old}/x\",\"t\":\"{p_old}\"}}")).unwrap();
        let rep = migrate_root(&paths, &old, &new).unwrap();
        assert!(outside.join("secret.txt").is_file(), "树外的文件没被搬走");
        assert!(dst.join("memory").is_dir() && !dst.join("memory").join("secret.txt").exists());
        assert!(std::fs::symlink_metadata(dst.join("memory.migrated")).map(|m| m.file_type().is_symlink()).unwrap_or(false), "链接本身当文件挪到旁边");
        let body = std::fs::read_to_string(dst.join("a.jsonl")).unwrap();
        assert_eq!(body, format!("{{\"cwd\":\"{p_new}\"}}\n{{\"cwd\":\"{p_new}/x\",\"t\":\"{p_old}\"}}"));
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    }

    #[test]
    fn slug_check_ignores_case() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        let paths = Paths::new(&home);
        let old = base.path().join("o");
        let new = base.path().join("n");
        std::fs::create_dir_all(old.join("RapidLine")).unwrap();
        let p_old = old.join("RapidLine").to_string_lossy().into_owned();
        // 磁盘上的目录名是小写的（别的工具先建的，Claude 在不分大小写的卷上照用）
        let src = paths.claude_root().join(claude_slug(&p_old).to_lowercase());
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("a.jsonl"), format!("{{\"cwd\":\"{p_old}\"}}\n")).unwrap();
        let rep = migrate_root(&paths, &old, &new).unwrap();
        let dst = paths.claude_root().join(claude_slug(&new.join("RapidLine").to_string_lossy()));
        assert!(dst.join("a.jsonl").is_file(), "大小写不同也要认出来并改名: {:?}", rep.warnings);
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    }

    #[test]
    fn colliding_slug_dirs_merge_instead_of_clobber() {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        let paths = Paths::new(&home);
        let old = base.path().join("o");
        let new = base.path().join("n");
        std::fs::create_dir_all(old.join("p")).unwrap();
        let p_old = old.join("p").to_string_lossy().into_owned();
        let p_new = new.join("p").to_string_lossy().into_owned();
        let src = paths.claude_root().join(claude_slug(&p_old));
        let dst = paths.claude_root().join(claude_slug(&p_new));
        std::fs::create_dir_all(src.join("memory")).unwrap();
        std::fs::create_dir_all(dst.join("memory")).unwrap();
        std::fs::write(src.join("a.jsonl"), format!("{{\"cwd\":\"{p_old}\"}}\n")).unwrap();
        std::fs::write(dst.join("b.jsonl"), format!("{{\"cwd\":\"{p_new}\"}}\n")).unwrap();
        std::fs::write(dst.join("memory/MEMORY.md"), "keep").unwrap();
        std::fs::write(src.join("memory/MEMORY.md"), "old").unwrap();
        let rep = migrate_root(&paths, &old, &new).unwrap();
        assert!(dst.join("a.jsonl").is_file() && dst.join("b.jsonl").is_file());
        assert_eq!(std::fs::read_to_string(dst.join("memory/MEMORY.md")).unwrap(), "keep", "已有的不被覆盖");
        assert_eq!(std::fs::read_to_string(dst.join("memory/MEMORY.migrated.md")).unwrap(), "old", "撞名的旧文件放旁边，不丢");
        assert!(!src.exists());
        assert!(rep.warnings.is_empty(), "{:?}", rep.warnings);
    }
}
