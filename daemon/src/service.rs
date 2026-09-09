//! Service management (`aaa-daemon service install|uninstall|status|restart`):
//! launchd on macOS, a systemd user unit on Linux.
//! Only ever executed explicitly from the CLI — never from tests or `run`.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::paths::Paths;

const LABEL: &str = "com.aaa.daemon";

/// 本机自签证书的名字，与 `install.sh` / `mac/packaging/bundle.sh` 用的是同一个。
pub const SIGN_IDENTITY: &str = "AAA Local Signing";
/// 签名里那个**固定的** identifier。cargo 产出的二进制自带一个随构建变的 ad-hoc
/// identifier（`aaa_daemon-<hash>`），不钉死就每次编译换一个身份。
pub const SIGN_IDENTIFIER: &str = "aaa-daemon";

// ---- 代码签名：TCC 授权能不能活过一次升级，全看这一段 ----
//
// macOS 的隐私授权（辅助功能 / 屏幕录制 / 输入监控 / 自动化）记的是**代码身份**，
// 不是路径：授权时记下这个二进制的 designated requirement，之后每次请求都拿它比对。
// 本机自签证书签出来的 DR 是
//     identifier "aaa-daemon" and certificate leaf = H"<证书指纹>"
// ——**与内容无关**，所以只要每次都用同一张证书、同一个 identifier 签，重新编译多少次
// 授权都还在。cargo 直接产出的二进制是 ad-hoc 签名，DR 里是随构建变的 identifier 加
// cdhash，换成它就等于换了一个 App，授权全部失效。
//
// 还有一条更隐蔽的：**不能就地覆盖**。`cp` / `install` 是把新内容写进同一个 inode，
// 正在跑的那个 daemon 的代码页当场失效，它**立刻**就通不过 TCC 校验了——不用等重启，
// 手里的授权当场没。所以安装和补签都走「写临时文件 → 签它 → rename 顶上去」：
// rename 只换目录项，跑着的进程还拿着原来那个 inode，一直有效到它自己退出。

/// 钥匙串里有没有那张自签证书
fn signing_identity_available() -> bool {
    std::process::Command::new("security")
        .args(["find-identity", "-v", "-p", "codesigning"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(SIGN_IDENTITY))
        .unwrap_or(false)
}

/// 这个二进制的 designated requirement（拿不到就是没签 / 签坏了）
fn designated_requirement(path: &Path) -> Option<String> {
    let out = std::process::Command::new("codesign")
        .args(["-d", "-r-"])
        .arg(path)
        .output()
        .ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    text.lines().find(|l| l.contains("designated =>")).map(str::to_string)
}

/// 它已经是我们要的那个身份了吗：identifier 钉死，而且是**证书**签的（不是 ad-hoc）
fn already_ours(path: &Path) -> bool {
    designated_requirement(path)
        .is_some_and(|r| r.contains(&format!("identifier \"{SIGN_IDENTIFIER}\"")) && r.contains("certificate leaf"))
}

/// 把 `path` 签成固定身份。返回「这次动手了没有」。
///
/// 没有那张证书（别人的机器、还没建过）就什么都不做——ad-hoc 也能跑，只是
/// 每次升级都要重新授权一次。
pub fn ensure_signed(path: &Path) -> io::Result<bool> {
    if !cfg!(target_os = "macos") || already_ours(path) || !signing_identity_available() {
        return Ok(false);
    }
    let tmp = path.with_extension("resign");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(path, &tmp)?;
    let st = std::process::Command::new("codesign")
        .args(["--force", "--sign", SIGN_IDENTITY, "-i", SIGN_IDENTIFIER])
        .arg(&tmp)
        .status()?;
    if !st.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io::Error::other(format!("codesign 失败（exit {st}）")));
    }
    let ok = std::process::Command::new("codesign")
        .args(["--verify", "--strict"])
        .arg(&tmp)
        .status()?;
    if !ok.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(io::Error::other("签完过不了 codesign --verify，没敢换上去"));
    }
    // rename 而不是覆盖：跑着的那个 daemon 还拿着老 inode，它的授权不受影响
    std::fs::rename(&tmp, path)?;
    println!("re-signed {} as {SIGN_IDENTITY} / {SIGN_IDENTIFIER}", path.display());
    Ok(true)
}

/// launchd/systemd 实际会拉起哪个文件——补签要补的是**那一个**，不是此刻在跑的这个
/// 命令行工具（用户可能是在仓库里直接跑 `target/release/aaa-daemon`）。
/// plist 是我们自己写的，取 `ProgramArguments` 的第一个 `<string>`。
fn managed_program(paths: &Paths) -> io::Result<std::path::PathBuf> {
    let from_plist = std::fs::read_to_string(paths.launchd_plist())
        .ok()
        .and_then(|text| program_in_plist(&text));
    match from_plist.filter(|p| p.is_file()) {
        Some(p) => Ok(p),
        None => std::env::current_exe(),
    }
}

/// plist 里 `ProgramArguments` 的第一个 `<string>`。这份 plist 是 `install` 自己写的，
/// 所以不引一个 plist 解析库，认这一处形状就够。
fn program_in_plist(text: &str) -> Option<std::path::PathBuf> {
    let after = text.split("ProgramArguments").nth(1)?;
    let open = after.find("<string>")? + "<string>".len();
    let rest = &after[open..];
    let end = rest.find("</string>")?;
    Some(std::path::PathBuf::from(rest[..end].trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 补签要补的是 launchd 真正会拉起的那个文件——不是此刻在跑的这个命令行工具
    /// （用户可能是在仓库里跑 `target/release/aaa-daemon`）。
    #[test]
    fn reads_the_program_path_out_of_our_own_plist() {
        let plist = macos::plist_body(Path::new("/Users/x/.local/bin/aaa-daemon"), Path::new("/Users/x/.state"));
        assert_eq!(
            program_in_plist(&plist),
            Some(std::path::PathBuf::from("/Users/x/.local/bin/aaa-daemon")),
        );
        assert_eq!(program_in_plist("<plist></plist>"), None, "不是我们那份 plist 就说不知道");
    }
}

#[cfg(not(target_os = "macos"))]
pub use linux::{install, kick, uninstall, status};
#[cfg(target_os = "macos")]
pub use macos::{install, kick, uninstall, status};

/// `aaa-daemon service restart`：**不管 daemon 此刻是死是活都能用**。
///
/// 客户端此前只有 `POST /api/v1/restart` 一条路，而那条路的前提正是「daemon 还活着」——
/// 它关掉之后按钮就再也点不动了（2026-09-10 用户报的就是这个）。所以顺序是：
/// ① 它还答话 → 走 REST 的 force 重启，**活着的会话会被记下来、起来之后自动 resume**；
/// ② 它不答话 → 交给服务管理器拉起（launchd / systemd），没装服务就直接把自己 spawn 起来。
///
/// 只有第一条能保住会话，所以顺序不能反。
pub fn restart(paths: &Paths) -> io::Result<()> {
    // 先把磁盘上那个二进制补成固定身份，**再**重启：新进程一起来就是被授权的那个身份，
    // 用户不用在系统设置里重新勾一遍。补签走 rename，正在跑的这个不受影响（见上）
    if let Ok(prog) = managed_program(paths) {
        if let Err(e) = ensure_signed(&prog) {
            eprintln!("补签失败（{e}）；照常重启，但隐私授权可能要重新给一次");
        }
    }
    match graceful(paths) {
        Ok(()) => {
            println!("restarting (会话已记下，起来后自动 resume)");
            wait_healthy(paths, Duration::from_secs(20));
            Ok(())
        }
        Err(why) => {
            println!("daemon 没有答话（{why}），改从服务管理器拉起");
            kick(paths)?;
            wait_healthy(paths, Duration::from_secs(20));
            Ok(())
        }
    }
}

/// 让还活着的 daemon 自己重启（REST）。返回 Err = 它没答话，调用方改走拉起那条路。
fn graceful(paths: &Paths) -> Result<(), String> {
    let cfg = crate::config::load_or_create(&paths.config_path()).map_err(|e| e.to_string())?;
    let port = running_port(paths).unwrap_or(cfg.port);
    ureq::post(&format!("http://127.0.0.1:{port}/api/v1/restart"))
        .set("Authorization", &format!("Bearer {}", cfg.token))
        .timeout(Duration::from_secs(10))
        .send_json(ureq::json!({ "force": true }))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// daemon 实际监听的端口：它启动时把端口写在 `daemon.port` 里（config 里可能是 0 或已改过）。
fn running_port(paths: &Paths) -> Option<u16> {
    std::fs::read_to_string(paths.port_file()).ok()?.trim().parse().ok()
}

/// 服务没装时的兜底：把自己 `run` 起来，脱离当前终端。
///
/// **必须把 `XPC_SERVICE_NAME` / `INVOCATION_ID` 摘掉**：daemon 靠这两个变量判断
/// 「我是被 launchd / systemd 管着的吗」，管着就用 `exit(0)` 让服务管理器拉起干净的
/// 实例。而这里的调用方常常本身就在被管的进程树里（mac App 是 Finder 拉起来的、
/// CLI 可能跑在 daemon 自己开的终端里），变量原样传下去，这个**没人管**的实例
/// 就会在下一次重启时 `exit(0)` 然后再也回不来。
fn spawn_detached() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(&exe)
        .arg("run")
        .env_remove("XPC_SERVICE_NAME")
        .env_remove("INVOCATION_ID")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    println!("spawned: {} run", exe.display());
    Ok(())
}

/// 等它回来：`/health` 可达且不在 `restarting`。等不到不算失败——服务管理器可能
/// 慢一点，这里只是想在命令返回前给出一句确定的话。
fn wait_healthy(paths: &Paths, budget: Duration) {
    let cfg = crate::config::load_or_create(&paths.config_path()).ok();
    let deadline = Instant::now() + budget;
    let mut port;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(400));
        // **每轮重读 `daemon.port`**：config 里写 0 时端口是启动那一刻才定的，
        // 重启前后可以不一样；只读一次就会对着一个没人听的端口白等到超时
        port = running_port(paths).or_else(|| cfg.as_ref().map(|c| c.port)).unwrap_or(2730);
        // `/health` 也在鉴权后面：不带 token 只会拿到 401，看起来就像「一直没起来」
        let ok = ureq::get(&format!("http://127.0.0.1:{port}/api/v1/health"))
            .set("Authorization", &format!("Bearer {}", cfg.as_ref().map(|c| c.token.as_str()).unwrap_or("")))
            .timeout(Duration::from_secs(2))
            .call()
            .ok()
            .and_then(|r| r.into_json::<serde_json::Value>().ok())
            .filter(|v| v.get("restarting").and_then(serde_json::Value::as_bool) != Some(true))
            .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(String::from));
        if let Some(v) = ok {
            println!("daemon v{v} 已就绪 (127.0.0.1:{port})");
            return;
        }
    }
    println!("等了 {}s 还没等到 /health，去看 ~/.local/state/aaa-daemon/daemon.err.log", budget.as_secs());
}

#[cfg(not(target_os = "macos"))]
mod linux {
    use super::*;

    fn unit_body(exe: &Path, state_dir: &Path) -> String {
        format!(
            "[Unit]\nDescription=AAA daemon (Claude Code sessions)\nAfter=network.target\n\n[Service]\nExecStart={exe} run\nRestart=always\nRestartSec=2\nStandardOutput=append:{out}\nStandardError=append:{err}\n\n[Install]\nWantedBy=default.target\n",
            exe = exe.display(),
            out = state_dir.join("daemon.out.log").display(),
            err = state_dir.join("daemon.err.log").display(),
        )
    }

    pub fn install(paths: &Paths) -> io::Result<()> {
        let exe = std::env::current_exe()?;
        let state_dir = paths.state_dir();
        std::fs::create_dir_all(&state_dir)?;
        let unit = paths.systemd_unit();
        if let Some(dir) = unit.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&unit, unit_body(&exe, &state_dir))?;
        println!("written: {}", unit.display());
        let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
        let st = std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "aaa-daemon.service"])
            .status()?;
        if st.success() {
            println!("enabled: aaa-daemon.service (systemd --user)");
        } else {
            eprintln!("systemctl --user enable failed (exit {st}); the unit was written");
        }
        Ok(())
    }

    pub fn uninstall(paths: &Paths) -> io::Result<()> {
        let unit = paths.systemd_unit();
        if unit.exists() {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "disable", "--now", "aaa-daemon.service"])
                .status();
            std::fs::remove_file(&unit)?;
            let _ = std::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status();
            println!("removed: {}", unit.display());
        } else {
            println!("not installed ({} missing)", unit.display());
        }
        Ok(())
    }

    /// 交给 systemd 拉起（没装单元就直接把自己 spawn 起来）。
    pub fn kick(paths: &Paths) -> io::Result<()> {
        if paths.systemd_unit().exists() {
            let st = std::process::Command::new("systemctl")
                .args(["--user", "restart", "aaa-daemon.service"])
                .status()?;
            if st.success() {
                println!("systemctl --user restart aaa-daemon.service");
                return Ok(());
            }
            eprintln!("systemctl restart failed (exit {st}); 直接 spawn");
        }
        super::spawn_detached()
    }

    pub fn status(paths: &Paths) -> io::Result<()> {
        let unit = paths.systemd_unit();
        println!("unit: {} ({})", unit.display(), if unit.exists() { "present" } else { "missing" });
        let out = std::process::Command::new("systemctl")
            .args(["--user", "is-active", "aaa-daemon.service"])
            .output()?;
        println!("systemd: {}", String::from_utf8_lossy(&out.stdout).trim());
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    pub(super) fn plist_body(exe: &Path, state_dir: &Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>Label</key>
        <string>{LABEL}</string>
        <key>ProgramArguments</key>
        <array>
            <string>{exe}</string>
            <string>run</string>
        </array>
        <key>RunAtLoad</key>
        <true/>
        <key>KeepAlive</key>
        <true/>
        <key>StandardOutPath</key>
        <string>{out}</string>
        <key>StandardErrorPath</key>
        <string>{err}</string>
    </dict>
    </plist>
    "#,
            exe = exe.display(),
            out = state_dir.join("daemon.out.log").display(),
            err = state_dir.join("daemon.err.log").display(),
        )
    }

    pub fn install(paths: &Paths) -> io::Result<()> {
        let exe = std::env::current_exe()?;
        // plist 指向哪个文件，就把哪个文件签成固定身份——授权是给身份的，不是给路径的
        if let Err(e) = super::ensure_signed(&exe) {
            eprintln!("补签失败（{e}）；服务照装，但每次升级都要重新授权一次");
        }
        let state_dir = paths.state_dir();
        std::fs::create_dir_all(&state_dir)?;
        let plist = paths.launchd_plist();
        if let Some(dir) = plist.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = plist.with_extension("plist.tmp");
        std::fs::write(&tmp, plist_body(&exe, &state_dir))?;
        std::fs::rename(&tmp, &plist)?;
        println!("written: {}", plist.display());
        let st = std::process::Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&plist)
            .status()?;
        if st.success() {
            println!("loaded: {LABEL}");
        } else {
            eprintln!("launchctl load failed (exit {st}); the plist was written");
        }
        Ok(())
    }

    pub fn uninstall(paths: &Paths) -> io::Result<()> {
        let plist = paths.launchd_plist();
        if plist.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload", "-w"])
                .arg(&plist)
                .status();
            std::fs::remove_file(&plist)?;
            println!("removed: {}", plist.display());
        } else {
            println!("not installed ({} missing)", plist.display());
        }
        Ok(())
    }

    /// 交给 launchd 拉起：`kickstart -k` 对「跑着的」是重启、对「装了没跑的」是启动。
    /// 服务没装（plist 不在）就直接把自己 spawn 起来——用户可能一直是手动跑的。
    pub fn kick(paths: &Paths) -> io::Result<()> {
        if paths.launchd_plist().exists() {
            let target = format!("gui/{}/{LABEL}", unsafe { libc::getuid() });
            let st = std::process::Command::new("launchctl")
                .args(["kickstart", "-k", &target])
                .status()?;
            if st.success() {
                println!("launchctl kickstart -k {target}");
                return Ok(());
            }
            eprintln!("launchctl kickstart failed (exit {st}); 直接 spawn");
        }
        super::spawn_detached()
    }

    pub fn status(paths: &Paths) -> io::Result<()> {
        let plist = paths.launchd_plist();
        println!(
            "plist: {} ({})",
            plist.display(),
            if plist.exists() { "present" } else { "missing" }
        );
        let out = std::process::Command::new("launchctl").arg("list").output()?;
        let listed = String::from_utf8_lossy(&out.stdout)
            .lines()
            .find(|l| l.contains(LABEL))
            .map(String::from);
        match listed {
            Some(line) => println!("launchctl: {line}"),
            None => println!("launchctl: not loaded"),
        }
        Ok(())
    }
}
