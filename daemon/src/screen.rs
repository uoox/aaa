//! 屏幕上的纯文本判定。**只剩一个**：answer 驱动按完键之后确认对话框真的没了。
//!
//! 这里从不解释屏幕内容——daemon 早就不再读屏猜「TUI 在问什么」（PROTOCOL.md，
//! 2026-09-02），对话框一律走 transcript 里的结构化信号。2026-09-08 又把最后一点
//! 读屏产物 `preview` 删了：v1.17 拿掉状态字之后没有任何客户端读它，而它每次
//! 构 `/sessions`、每个 session WS 帧都要拿一次解析器锁、把整屏逐行剥框线。

/// 可见区任意一行含 `needle`？
pub fn screen_contains(screen: &vt100::Screen, needle: &str) -> bool {
    let (_, cols) = screen.size();
    screen.rows(0, cols).any(|r| r.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_scans_visible_rows() {
        let mut p = vt100::Parser::new(24, 80, 100);
        p.process("Pick a color\r\n\u{276f} 1. Red\r\nEnter to select \u{b7} Esc to cancel\r\n".as_bytes());
        assert!(screen_contains(p.screen(), "Enter to select"));
        assert!(!screen_contains(p.screen(), "Ready to submit"));
    }
}
