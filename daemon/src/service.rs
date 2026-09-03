//! Service management (`aaa-daemon service install|uninstall|status`):
//! launchd on macOS, a systemd user unit on Linux.
//! Only ever executed explicitly from the CLI — never from tests or `run`.

use std::io;
use std::path::Path;

use crate::paths::Paths;

const LABEL: &str = "com.aaa.daemon";

#[cfg(not(target_os = "macos"))]
pub use linux::{install, uninstall, status};
#[cfg(target_os = "macos")]
pub use macos::{install, uninstall, status};

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
}
