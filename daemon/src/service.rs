//! launchd service management (`aaa-daemon service install|uninstall|status`).
//! Only ever executed explicitly from the CLI — never from tests or `run`.

use std::io;
use std::path::Path;

use crate::paths::Paths;

const LABEL: &str = "com.aaa.daemon";

fn plist_body(exe: &Path, state_dir: &Path) -> String {
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
