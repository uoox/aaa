//! macOS 系统通知：shell out `osascript -e 'display notification …'`。
//! （gpui 无通知 API；对本机自用场景足够。）

/// 生成 AppleScript 片段（纯函数，便于测试转义）
pub fn build_script(title: &str, body: &str) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "display notification \"{}\" with title \"{}\"",
        esc(body),
        esc(title)
    )
}

/// 发送通知（异步 spawn，不阻塞 UI；失败静默）
pub fn send(title: &str, body: &str) {
    let script = build_script(title, body);
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_shape() {
        assert_eq!(
            build_script("会话 A", "等待输入"),
            r#"display notification "等待输入" with title "会话 A""#
        );
    }

    #[test]
    fn escapes_quotes_and_backslashes() {
        let s = build_script(r#"a"b"#, r#"c\d "e""#);
        assert_eq!(
            s,
            r#"display notification "c\\d \"e\"" with title "a\"b""#
        );
    }
}
