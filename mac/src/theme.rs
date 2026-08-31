//! 设计令牌（来源 PROTOCOL.md「设计令牌」表，与 prototype.html 一致）。
//! 全部以 0xRRGGBB u32 存储，UI 层用 `gpui::rgb()` 转换。

pub const BG: u32 = 0x0e1216; // 页面底
pub const SURFACE: u32 = 0x1a222b; // 卡片/面板
pub const SURFACE_RAISED: u32 = 0x212b36;
pub const EDGE: u32 = 0x28323e; // 描边
pub const EDGE_LIGHT: u32 = 0x36434f;
pub const INK: u32 = 0xe3ebf3; // 文字一级
pub const DIM: u32 = 0x8b99a8; // 文字二级
pub const FAINT: u32 = 0x5f6d7c; // 文字三级
pub const TERM_BG: u32 = 0x0a0e12; // 终端底
pub const CYAN: u32 = 0x53c6dd; // 主操作/选中
pub const MAGENTA: u32 = 0xc583e0; // 品牌辅色（= ANSI magenta）
pub const GREEN: u32 = 0x5ecb8f; // running
pub const AMBER: u32 = 0xe3b45c; // waiting
pub const RED: u32 = 0xe57373; // exited

/// agent 标签色
pub fn agent_color(agent: &str) -> u32 {
    match agent {
        "claude" => 0xe8b46a,
        "codex" => 0x8fd0ff,
        "pi" => 0xb5e08f,
        "reasonix" => 0xe08fb5,
        "agy" => 0xc8a8f0,
        _ => DIM, // shell / 未知
    }
}

/// 会话状态点颜色：绿=运行中、黄=等待输入、灰=空闲、红=已退出
pub fn state_color(state: &str) -> u32 {
    match state {
        "running" => GREEN,
        "waiting" => AMBER,
        "idle" => FAINT,
        "exited" => RED,
        _ => FAINT,
    }
}

pub fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "运行中",
        "waiting" => "等待输入",
        "idle" => "空闲",
        "exited" => "已退出",
        _ => "未知",
    }
}

// ── 终端 ANSI 16 色（深色主题，与设计令牌协调） ──────────────────────────────

pub const ANSI: [u32; 16] = [
    0x1c242e, // 0 black（略亮于 term-bg，保证可见）
    RED,      // 1 red
    GREEN,    // 2 green
    AMBER,    // 3 yellow
    0x6fa8dc, // 4 blue
    MAGENTA,  // 5 magenta
    CYAN,     // 6 cyan
    TERM_FG,  // 7 white
    FAINT,    // 8 bright black
    0xef9a9a, // 9 bright red
    0x81e2ac, // 10 bright green
    0xf0c987, // 11 bright yellow
    0x8fc3f0, // 12 bright blue
    0xd9a8ef, // 13 bright magenta
    0x7fdbef, // 14 bright cyan
    INK,      // 15 bright white
];

pub const TERM_FG: u32 = 0xc9d4de;

/// xterm-256 调色板：0-15 用主题色，16-231 6×6×6 色立方，232-255 灰阶
pub fn indexed_color(idx: u8) -> u32 {
    match idx {
        0..=15 => ANSI[idx as usize],
        16..=231 => {
            let i = idx as u32 - 16;
            let (r, g, b) = (i / 36, (i / 6) % 6, i % 6);
            let c = |v: u32| if v == 0 { 0 } else { v * 40 + 55 };
            (c(r) << 16) | (c(g) << 8) | c(b)
        }
        232..=255 => {
            let v = (idx as u32 - 232) * 10 + 8;
            (v << 16) | (v << 8) | v
        }
    }
}

/// 人性化字节数（客户端渲染约定：daemon 只给原始字节数）
pub fn human_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let n = n as f64;
    if n < K {
        format!("{}B", n as u64)
    } else if n < K * K {
        format!("{:.1}K", n / K)
    } else if n < K * K * K {
        format!("{:.1}M", n / K / K)
    } else {
        format!("{:.1}G", n / K / K / K)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_colors_match_tokens() {
        assert_eq!(agent_color("claude"), 0xe8b46a);
        assert_eq!(agent_color("codex"), 0x8fd0ff);
        assert_eq!(agent_color("pi"), 0xb5e08f);
        assert_eq!(agent_color("reasonix"), 0xe08fb5);
        assert_eq!(agent_color("agy"), 0xc8a8f0);
        assert_eq!(agent_color("shell"), DIM);
    }

    #[test]
    fn state_colors() {
        assert_eq!(state_color("running"), GREEN);
        assert_eq!(state_color("waiting"), AMBER);
        assert_eq!(state_color("idle"), FAINT);
        assert_eq!(state_color("exited"), RED);
    }

    #[test]
    fn cube_and_gray() {
        assert_eq!(indexed_color(16), 0x000000);
        assert_eq!(indexed_color(231), 0xffffff);
        assert_eq!(indexed_color(232), 0x080808);
        assert_eq!(indexed_color(255), 0xeeeeee);
        // 21 = 16 + 0*36 + 0*6 + 5 → 纯蓝
        assert_eq!(indexed_color(21), 0x0000ff);
    }

    #[test]
    fn bytes_humanized() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(4198), "4.1K");
        assert_eq!(human_bytes(134217728), "128.0M");
        assert_eq!(human_bytes(2469606195), "2.3G");
    }
}
