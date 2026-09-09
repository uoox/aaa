//! 设计令牌（来源 PROTOCOL.md「设计令牌」表）。全部以 0xRRGGBB u32 存储，UI 层用 `gpui::rgb()` 转换。
//!
//! 只有一套主题（2026-09-10 用户拍板：「不需要黑暗模式，仅保留一个主题即可，精简代码」）：
//! Anthropic 的象牙白 + 陶土橙，终端是暖白纸面 + 墨字，ANSI 走 gruvbox-light。
//! 此前的 ThemeKind / 进程级 CURRENT / 每个令牌一个小函数全部拿掉——令牌就是 `pub const`，
//! `c(theme::INK)` 一眼看得出是常量，也没有每帧几百次的原子读和 `is_dark` 分支。
//! 共享向量 `fixtures/tokens.json` 钉着全部数值，Android 那边有一份对着同一个文件的测试。

/// 页面底（象牙白）
pub const BG: u32 = 0xfaf9f5;
/// 卡片 / 面板
pub const SURFACE: u32 = 0xf0eee6;
/// 浮起一层（悬停底、芯片底）
pub const SURFACE_RAISED: u32 = 0xe8e6dc;
/// 描边
pub const EDGE: u32 = 0xdad8ce;
pub const EDGE_LIGHT: u32 = 0xc8c6bc;
/// 文字三级
pub const INK: u32 = 0x141413;
pub const DIM: u32 = 0x5e5d59;
pub const FAINT: u32 = 0x91908a;
/// 终端底：暖白纸面，和界面一体
pub const TERM_BG: u32 = 0xfffdf7;
/// 终端字 = 墨
pub const TERM_FG: u32 = INK;
/// 主操作 / 选中 / 链接 / 选中项目标题的下划线（陶土橙；原「CYAN」角色）
pub const ACCENT: u32 = 0xd97757;
/// 品牌辅色：哑紫，不取橙的邻色，否则和 accent 分不开。mac 目前没有调用点
/// （Android 配对页大标题用），留着是因为令牌表两端要齐、共享向量钉着它
#[allow(dead_code)]
pub const MAGENTA: u32 = 0x9b6b9e;
/// 执行中 / 已激活（轮到你）/ 出错、已退出
pub const GREEN: u32 = 0x2f855a;
pub const AMBER: u32 = 0xb8860b;
pub const RED: u32 = 0xc0392b;
/// 看板卡片标题前「在跑」那根线。green/amber/red 各有旧含义，accent 是橙，
/// 只有蓝读作「它自己在动」
pub const BLUE: u32 = 0x3f6ea8;
/// 输入框 / 折叠面板的下沉底：比 surface 更亮一点点
pub const INSET: u32 = 0xfffefa;
/// 项目列表行的状态底色（2026-09-10 用整行淡底代替行尾竖线）：淡蓝 = 在跑
pub const ROW_RUNNING: u32 = 0xdde7f1;
/// 淡黄 = 跑完了 / 在等你回话而这台机器还没进去看
pub const ROW_UNREAD: u32 = 0xf6e7c9;
/// 实心主按钮上的字色 = 墨。以前按亮度算（`text_on(ACCENT)` = `#21120d`），Android 直接用
/// ink，两端差一丁点也是差——v1.25 起写死同一个值并进共享向量
pub const ON_ACCENT: u32 = INK;
/// 代码 span / 行内代码的文字色：ANSI 14（暗青），亮底上可读且与链接的 accent 分得开
pub const CODE_INK: u32 = ANSI[14];
/// 行内代码 / 等宽块的底：终端底跟纸面几乎同色，用 surface_raised 才看得出是块芯片
pub const CODE_BG: u32 = SURFACE_RAISED;

/// 终端 ANSI 16 色：gruvbox-light。浅底上 8–15 比 0–7 更沉而不是更亮，
/// 7 white 给成暖灰（亮底可见），15 bright white = ink。两端逐色相同（v1.23）。
pub const ANSI: [u32; 16] = [
    0x3c3836, // 0 black
    0xcc241d, // 1 red
    0x98971a, // 2 green
    0xd79921, // 3 yellow
    0x458588, // 4 blue
    0xb16286, // 5 magenta
    0x689d6a, // 6 cyan
    0xa89984, // 7 white（暖灰）
    0x7c6f64, // 8 bright black
    0x9d0006, // 9 bright red
    0x79740e, // 10 bright green
    0xb57614, // 11 bright yellow
    0x076678, // 12 bright blue
    0x8f3f71, // 13 bright magenta
    0x427b58, // 14 bright cyan
    INK,      // 15 bright white
];

/// xterm-256 调色板：0-15 用主题色，16-231 6×6×6 色立方，232-255 灰阶。
/// 终端渲染（`term.rs`、`ui/terminal_view.rs`）接的就是它。
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

// ── 颜色运算 ────────────────────────────────────────────────────────────────

fn channels(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    ]
}

/// WCAG 相对亮度（sRGB 线性化后加权）
fn luminance(hex: u32) -> f32 {
    let lin = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(hex);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// 线性插值：`t` 是 `b` 的权重（0 = 全 a，1 = 全 b）
pub fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ca = channels(a);
    let cb = channels(b);
    let ch = |i: usize| ((ca[i] + (cb[i] - ca[i]) * t) * 255.0).round() as u32;
    (ch(0) << 16) | (ch(1) << 8) | ch(2)
}

/// 实心色块上该用的字色：底够亮就用带底色调的深字（比纯黑柔和），否则白字。
/// 阈值 0.179 是黑字 / 白字对比度的交叉点（WCAG）。
pub fn text_on(bg: u32) -> u32 {
    if luminance(bg) > 0.179 {
        mix(bg, 0x000000, 0.85)
    } else {
        0xffffff
    }
}

// ── 状态 ────────────────────────────────────────────────────────────────────

/// 会话状态点颜色：绿=运行中、黄=这轮干完了（waiting）、红=已退出
pub fn state_color(state: &str) -> u32 {
    match state {
        "running" => GREEN,
        "waiting" => AMBER,
        "exited" => RED,
        _ => FAINT,
    }
}

/// 状态字，按三态口径：waiting 就是「已完成」（这轮说完了，轮到你）；
/// 「待回复」不是状态而是 `asking`，由调用方优先盖上去
pub fn state_label(state: &str) -> &'static str {
    match state {
        "running" => "运行中",
        "waiting" => "已完成",
        "exited" => "已退出",
        _ => "未知",
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

    /// 设计令牌的三端共享向量 `fixtures/tokens.json`（Android 那边有一份对着同一个文件的
    /// 测试）。「两端 UI 必须一致」这句话只有被一份共同的向量盯着才成立——此前两端各测
    /// 各的一套，终端前景一边 `#c9d4de` 一边纯白、magenta 一边哑紫一边直接等于 accent，
    /// 谁都没发现。
    #[test]
    fn tokens_match_the_shared_fixture() {
        let fx: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/tokens.json")).unwrap();
        let hex = |v: u32| format!("#{v:06x}");
        let roles = &fx["roles"];
        let want: Vec<(&str, u32)> = vec![
            ("bg", BG),
            ("surface", SURFACE),
            ("surface_raised", SURFACE_RAISED),
            ("edge", EDGE),
            ("edge_light", EDGE_LIGHT),
            ("ink", INK),
            ("dim", DIM),
            ("faint", FAINT),
            ("term_bg", TERM_BG),
            ("term_fg", TERM_FG),
            ("accent", ACCENT),
            ("on_accent", ON_ACCENT),
            ("magenta", MAGENTA),
            ("green", GREEN),
            ("amber", AMBER),
            ("red", RED),
            ("blue", BLUE),
            ("inset", INSET),
            ("row_running", ROW_RUNNING),
            ("row_unread", ROW_UNREAD),
        ];
        assert_eq!(roles.as_object().unwrap().len(), want.len(), "向量里的角色数要和这里对上");
        for (role, got) in want {
            assert_eq!(roles[role].as_str().unwrap(), hex(got), "{role}");
        }
        let ansi: Vec<String> = fx["ansi"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(ansi, ANSI.iter().map(|c| hex(*c)).collect::<Vec<_>>(), "ansi");
    }

    /// 亮底主题的几条硬约束：终端是亮底暗字、16 色没有一个比底还亮、下沉底和行底都是亮的
    #[test]
    fn light_palette_stays_readable() {
        assert!(luminance(TERM_BG) > 0.9, "终端底要是亮的");
        assert!(luminance(TERM_FG) < 0.2, "终端字要是暗的");
        assert!(luminance(CODE_INK) < 0.4, "代码字色要在亮底上可读");
        for (i, &a) in ANSI.iter().enumerate() {
            assert!(luminance(a) < 0.6, "{i}: ansi 色 {a:06x} 在亮底上看不见");
        }
        assert!(luminance(INSET) > 0.5, "INK 字要落在下沉底上");
        // 项目行的两种状态底：都是淡的（字照样黑），彼此分得开，也和侧栏底分得开
        for bg in [ROW_RUNNING, ROW_UNREAD] {
            assert!(luminance(bg) > 0.7, "行底 {bg:06x} 该是淡的");
            assert_ne!(bg, SURFACE);
        }
        assert_ne!(ROW_RUNNING, ROW_UNREAD);
        assert_ne!(BLUE, AMBER);
        assert!((luminance(BLUE) - luminance(BG)).abs() > 0.1, "看板的蓝线在底色上看不见");
    }

    #[test]
    fn text_on_picks_readable_ink() {
        // 亮底 → 带底色调的深字；深底 → 白字
        assert_eq!(text_on(0xffffff), mix(0xffffff, 0, 0.85));
        assert_eq!(text_on(0x000000), 0xffffff);
        // accent 够亮，主按钮字是深色（用户要求）：算出来的也是深字，写死的常量更是
        assert_ne!(text_on(ACCENT), 0xffffff);
        assert!(luminance(ON_ACCENT) < 0.2);
        // 红是深红，危险按钮用白字
        assert_eq!(text_on(RED), 0xffffff);
        // mix 端点与中点
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }

    #[test]
    fn state_colors() {
        assert_eq!(state_color("running"), GREEN);
        assert_eq!(state_color("waiting"), AMBER);
        assert_eq!(state_color("exited"), RED);
        assert_eq!(state_color("idle"), FAINT, "未知状态一律灰");
    }

    #[test]
    fn cube_and_gray() {
        assert_eq!(indexed_color(1), ANSI[1]);
        assert_eq!(indexed_color(15), INK);
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
