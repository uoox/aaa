//! 设计令牌（来源 PROTOCOL.md「设计令牌」表）。
//! 全部以 0xRRGGBB u32 存储，UI 层用 `gpui::rgb()` 转换。
//!
//! 2026-09-03 起有多套主题，进程内可切换；2026-09-08 用户砍到两套（黑暗 / Claude 橙）：
//! - 黑暗主题的原值直接写在 `DARK` 里；只有终端那张 16 色表（`ANSI` / `TERM_BG` /
//!   `TERM_FG`）还是 `pub const`，因为 `indexed_color` 这个自由函数在用；
//! - 其余 UI 一律走 `palette()` 或各令牌的小函数（`bg()`、`accent()`…），读的是
//!   `set_current` 选定的那一套；每帧调用，代价只是一次原子读；
//! - 选定的主题持久化在 `~/.config/aaa-ui/ui.toml` 的 `theme` 字段（model::UiState）。

use std::sync::atomic::{AtomicU8, Ordering};

/// 终端底（黑暗主题；`inset` 也用它）
pub const TERM_BG: u32 = 0x0a0e12;

// ── 主题 ────────────────────────────────────────────────────────────────────

/// 两套主题。`Dark` 是原始设计令牌；`Claude` 是 Anthropic 的象牙白 + 陶土橙，
/// 终端保留一块暖色深底。
///
/// 2026-09-08 用户拍板砍掉了中间那套「明亮」：一套暗、一套亮就够，三套的第三套
/// 从来只是「另一种亮」。存量 `ui.toml` 里写着 `theme = "light"` 的机器不需要迁移——
/// [`ThemeKind::from_str`] 认不出的名字一律回黑暗。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeKind {
    #[default]
    Dark,
    Claude,
}

impl ThemeKind {
    /// 设置页芯片的排列顺序
    pub const ALL: [ThemeKind; 2] = [ThemeKind::Dark, ThemeKind::Claude];

    /// ui.toml 里的值 → 主题；认不出来（旧文件、手改错）一律黑暗，不报错。
    // 刻意不实现 FromStr：这里要的是永不失败的兜底解析，Result 只会逼调用方再兜一次
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> ThemeKind {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude" => ThemeKind::Claude,
            // "light" 也走这条：那套主题 2026-09-08 删了，存量配置自动回黑暗
            _ => ThemeKind::Dark,
        }
    }

    /// 写进 ui.toml 的值
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::Claude => "claude",
        }
    }

    /// 设置页芯片文字
    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "黑暗",
            ThemeKind::Claude => "Claude 橙",
        }
    }

    fn from_u8(v: u8) -> ThemeKind {
        match v {
            1 => ThemeKind::Claude,
            _ => ThemeKind::Dark,
        }
    }
}

/// 一套主题的全部令牌。字段名沿用设计令牌表；`accent` 即原来的 CYAN 角色
/// （主操作 / 选中 / 链接），`inset` 是输入框、折叠面板这类「下沉底」——深色
/// 主题就是终端底，浅色主题另给一块近白，与终端底分开调。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub surface: u32,
    pub surface_raised: u32,
    pub edge: u32,
    pub edge_light: u32,
    pub ink: u32,
    pub dim: u32,
    pub faint: u32,
    pub term_bg: u32,
    pub term_fg: u32,
    pub accent: u32,
    pub magenta: u32,
    pub green: u32,
    pub amber: u32,
    pub red: u32,
    /// 项目行首「在跑」那一点。三套主题里都要是**蓝**：green/amber/red 各有旧含义，
    /// accent 又随主题从青变成橙，只有蓝在三套里都读作「它自己在动」。
    pub blue: u32,
    pub inset: u32,
    pub ansi: [u32; 16],
    pub is_dark: bool,
}

/// 黑暗 = PROTOCOL.md「设计令牌」表的原值。以前这些值先声明成 15 个 `pub const`
/// 再逐个填进来，另有一个测试断言两边相等——而结构体就是用那些常量初始化的，
/// 断言恒真。常量在 theme.rs 之外零引用，2026-09-08 一并内联掉。
static DARK: Palette = Palette {
    bg: 0x0e1216,          // 页面底
    surface: 0x1a222b,     // 卡片/面板
    surface_raised: 0x212b36,
    edge: 0x28323e,        // 描边
    edge_light: 0x36434f,
    ink: 0xe3ebf3,         // 文字一级
    dim: 0x8b99a8,         // 文字二级
    faint: 0x5f6d7c,       // 文字三级
    term_bg: TERM_BG,      // 终端底
    term_fg: TERM_FG,
    accent: 0x53c6dd,      // 主操作/选中（原 CYAN）
    magenta: 0xc583e0,     // 品牌辅色（= ANSI magenta）
    green: 0x5ecb8f,       // running
    amber: 0xe3b45c,       // waiting
    red: 0xe57373,         // exited
    blue: 0x6c9ef8,        // 「在跑」的行首点
    inset: TERM_BG,
    ansi: ANSI,
    is_dark: true,
};

/// Claude 橙：Anthropic 的象牙白 + 陶土橙；终端也是亮底暗字（暖白纸面，和界面一体），
/// ANSI 走 gruvbox-light 一路——同明亮主题的道理，浅底上 8–15 比 0–7 更沉而不是更亮，
/// 7 white 给成暖灰。品牌辅色不用橙的邻色，留一个哑紫（plum）做区分。
static CLAUDE: Palette = Palette {
    bg: 0xfaf9f5,
    surface: 0xf0eee6,
    surface_raised: 0xe8e6dc,
    edge: 0xdad8ce,
    edge_light: 0xc8c6bc,
    ink: 0x141413,
    dim: 0x5e5d59,
    faint: 0x91908a,
    term_bg: 0xfffdf7,
    term_fg: 0x141413,
    accent: 0xd97757,
    magenta: 0x9b6b9e,
    green: 0x2f855a,
    amber: 0xb8860b,
    red: 0xc0392b,
    blue: 0x3f6ea8,
    inset: 0xfffefa,
    ansi: [
        0x3c3836, // 0 black
        0xcc241d, // 1 red
        0x98971a, // 2 green
        0xd79921, // 3 yellow
        0x458588, // 4 blue
        0xb16286, // 5 magenta
        0x689d6a, // 6 cyan
        0xa89984, // 7 white（暖灰，亮底可见）
        0x7c6f64, // 8 bright black
        0x9d0006, // 9 bright red
        0x79740e, // 10 bright green
        0xb57614, // 11 bright yellow
        0x076678, // 12 bright blue
        0x8f3f71, // 13 bright magenta
        0x427b58, // 14 bright cyan
        0x141413, // 15 bright white（= ink）
    ],
    is_dark: false,
};

impl Palette {
    pub const fn for_kind(kind: ThemeKind) -> &'static Palette {
        match kind {
            ThemeKind::Dark => &DARK,
            ThemeKind::Claude => &CLAUDE,
        }
    }

    /// xterm-256 调色板（本主题版）：0-15 主题色，16-231 色立方，232-255 灰阶。
    /// 终端渲染（`term.rs`、`ui/terminal_view.rs`）接的就是它。
    /// （`#[allow(dead_code)]` 于 v1.22 拿掉：接线早就做完了，标记一直留着。）
    pub fn indexed_color(&self, idx: u8) -> u32 {
        match idx {
            0..=15 => self.ansi[idx as usize],
            _ => indexed_color(idx),
        }
    }

    /// 代码 span / 行内代码的文字色：亮青（ANSI 14），在 `term_bg` 上可读，
    /// 且与链接的 accent 区分开
    pub fn code_ink(&self) -> u32 {
        self.ansi[14]
    }
}

/// 当前主题（进程级；`ThemeKind as u8`）。启动时由 RootView 按 ui.toml 设定，
/// 之后只在设置页切换时改。
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_current(kind: ThemeKind) {
    CURRENT.store(kind as u8, Ordering::Relaxed);
}

pub fn current() -> ThemeKind {
    ThemeKind::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// 当前调色板。每帧会被调几百次：一次原子读 + 一个 match，不用缓存。
pub fn palette() -> &'static Palette {
    Palette::for_kind(current())
}

// 各令牌的小函数：`c(theme::ink())` 与旧写法 `c(theme::INK)` 形状一致，好替换好读

pub fn bg() -> u32 {
    palette().bg
}
pub fn surface() -> u32 {
    palette().surface
}
pub fn surface_raised() -> u32 {
    palette().surface_raised
}
pub fn edge() -> u32 {
    palette().edge
}
pub fn edge_light() -> u32 {
    palette().edge_light
}
pub fn ink() -> u32 {
    palette().ink
}
pub fn dim() -> u32 {
    palette().dim
}
pub fn faint() -> u32 {
    palette().faint
}
pub fn term_bg() -> u32 {
    palette().term_bg
}
pub fn term_fg() -> u32 {
    palette().term_fg
}
/// 主操作 / 选中 / 链接（黑暗主题里就是 CYAN）
pub fn accent() -> u32 {
    palette().accent
}
pub fn green() -> u32 {
    palette().green
}
pub fn amber() -> u32 {
    palette().amber
}
pub fn red() -> u32 {
    palette().red
}

/// 项目行首「在跑」那一点
pub fn blue() -> u32 {
    palette().blue
}
/// 输入框 / 折叠面板的下沉底（见 `Palette::inset`）
pub fn inset() -> u32 {
    palette().inset
}
pub fn code_ink() -> u32 {
    palette().code_ink()
}
/// 行内代码的底：黑暗主题是终端底（比正文底更深，一眼分出来）；浅色主题的终端底
/// 跟纸面几乎同色，改用 surface_raised 才看得出是块芯片
pub fn code_bg() -> u32 {
    let p = palette();
    if p.is_dark { p.term_bg } else { p.surface_raised }
}
/// 实心主按钮上的字色
pub fn on_accent() -> u32 {
    text_on(accent())
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
    let p = palette();
    match state {
        "running" => p.green,
        "waiting" => p.amber,
        "exited" => p.red,
        _ => p.faint,
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

// ── 终端 ANSI 16 色（深色主题，与设计令牌协调） ──────────────────────────────

/// 1/2/3/5/6/8/15 号色刻意与界面令牌同值（红 / 绿 / 琥珀 / 品牌辅色 / 主色 /
/// 文字三级 / 文字一级），终端里的这几个字和界面上的同一个意思长一个样。
pub const ANSI: [u32; 16] = [
    0x1c242e, // 0 black（略亮于 term-bg，保证可见）
    0xe57373, // 1 red（= DARK.red）
    0x5ecb8f, // 2 green（= DARK.green）
    0xe3b45c, // 3 yellow（= DARK.amber）
    0x6fa8dc, // 4 blue
    0xc583e0, // 5 magenta（= DARK.magenta）
    0x53c6dd, // 6 cyan（= DARK.accent）
    TERM_FG,  // 7 white
    0x5f6d7c, // 8 bright black（= DARK.faint）
    0xef9a9a, // 9 bright red
    0x81e2ac, // 10 bright green
    0xf0c987, // 11 bright yellow
    0x8fc3f0, // 12 bright blue
    0xd9a8ef, // 13 bright magenta
    0x7fdbef, // 14 bright cyan
    0xe3ebf3, // 15 bright white（= DARK.ink）
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
    use std::sync::Mutex;

    /// 设计令牌的三端共享向量 `fixtures/tokens.json`（Android 那边有一份对着同一个文件的
    /// 测试）。「两端 UI 必须一致」这句话此前只写在 PROTOCOL 里，没有任何东西盯着——
    /// 深色终端前景一边 `#c9d4de` 一边纯白、Claude 主题的 magenta 一边哑紫一边直接等于
    /// accent，两端各测各的一套，所以谁都没发现。
    #[test]
    fn tokens_match_the_shared_fixture() {
        let fx: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/tokens.json")).unwrap();
        let hex = |v: u32| format!("#{v:06x}");
        for (name, p) in [("dark", &DARK), ("claude", &CLAUDE)] {
            let roles = &fx["themes"][name]["roles"];
            for (role, got) in [
                ("bg", p.bg),
                ("surface", p.surface),
                ("surface_raised", p.surface_raised),
                ("edge", p.edge),
                ("edge_light", p.edge_light),
                ("ink", p.ink),
                ("dim", p.dim),
                ("faint", p.faint),
                ("term_bg", p.term_bg),
                ("term_fg", p.term_fg),
                ("accent", p.accent),
                ("magenta", p.magenta),
                ("green", p.green),
                ("amber", p.amber),
                ("red", p.red),
                ("blue", p.blue),
                ("inset", p.inset),
            ] {
                assert_eq!(roles[role].as_str().unwrap(), hex(got), "{name}.{role}");
            }
        }
        // v1.23：**两套**主题的终端 16 色都两端逐色相同（深色那套 Android 以前用的是
        // xterm / termux 出厂表，同一段输出在两端颜色完全不一样）。
        for (name, ansi) in [("dark", &ANSI), ("claude", &CLAUDE.ansi)] {
            let want: Vec<String> = fx["themes"][name]["ansi"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            assert_eq!(want, ansi.iter().map(|c| hex(*c)).collect::<Vec<_>>(), "{name}.ansi");
        }
    }

    /// 动到进程级 CURRENT 的测试串行跑，别让并行的兄弟测试读到半路换掉的主题
    static CURRENT_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn theme_kind_round_trip_and_fallback() {
        for k in ThemeKind::ALL {
            assert_eq!(ThemeKind::from_str(k.as_str()), k);
            assert_eq!(ThemeKind::from_u8(k as u8), k);
        }
        // 大小写 / 空白宽容；认不出的一律黑暗
        assert_eq!(ThemeKind::from_str(" Claude "), ThemeKind::Claude);
        assert_eq!(ThemeKind::from_str("CLAUDE"), ThemeKind::Claude);
        // 2026-09-08 删掉的那套：存量 ui.toml 里的 "light" 静静回黑暗，不报错
        assert_eq!(ThemeKind::from_str("light"), ThemeKind::Dark);
        assert_eq!(ThemeKind::from_str(""), ThemeKind::Dark);
        assert_eq!(ThemeKind::from_str("solarized"), ThemeKind::Dark);
        assert_eq!(ThemeKind::default(), ThemeKind::Dark);
        assert_eq!(ThemeKind::from_u8(200), ThemeKind::Dark);
    }

    #[test]
    fn dark_palette_indexing_and_code_ink() {
        let p = Palette::for_kind(ThemeKind::Dark);
        assert_eq!(p.code_ink(), 0x7fdbef, "代码字色 = 原 CODE_INK");
        // 调色板版的 256 色索引必须与自由函数逐个一致
        for i in 0..=255u8 {
            assert_eq!(p.indexed_color(i), indexed_color(i));
        }
    }

    #[test]
    fn palettes_are_distinct_and_flagged() {
        let [d, c] = ThemeKind::ALL.map(Palette::for_kind);
        assert_ne!(d.bg, c.bg);
        assert_ne!(d.accent, c.accent);
        assert!(d.is_dark);
        assert!(!c.is_dark, "Claude 橙是象牙白纸面");
        // 用户拍板的关键色
        assert_eq!(c.accent, 0xd97757);
        // 两套里黑暗是暗底终端；Claude 橙是亮底暗字
        assert!(luminance(d.term_bg) < 0.2);
        for p in [c] {
            assert!(luminance(p.term_bg) > 0.9, "浅色主题终端底要是亮的");
            assert!(luminance(p.term_fg) < 0.2, "浅色主题终端字要是暗的");
            assert!(luminance(p.code_ink()) < 0.4, "代码字色要在亮底上可读");
            // 亮底上 16 色都得有足够对比：没有一个 ANSI 色比底还亮
            for (i, &a) in p.ansi.iter().enumerate() {
                assert!(luminance(a) < 0.6, "{i}: ansi 色 {a:06x} 在亮底上看不见");
            }
        }
        // 项目行首三点（蓝 = 在跑 / 黄 = 未读 / 灰 = 已读）：两套主题上都得看得见，且互不撞色
        for p in [d, c] {
            assert_ne!(p.blue, p.amber);
            assert_ne!(p.blue, p.dim);
            assert!((luminance(p.blue) - luminance(p.bg)).abs() > 0.1, "行首蓝点在底色上看不见");
        }
        // 浅色主题的下沉底不能是深色终端底：INK 字要落在上面
        assert!(luminance(c.inset) > 0.5);
    }

    #[test]
    fn text_on_picks_readable_ink() {
        // 亮底 → 带底色调的深字；深底 → 白字
        assert_eq!(text_on(0xffffff), mix(0xffffff, 0, 0.85));
        assert_eq!(text_on(0x000000), 0xffffff);
        for k in ThemeKind::ALL {
            let p = Palette::for_kind(k);
            // 三套 accent 都够亮，按钮字都是深色（用户要求）
            assert_ne!(text_on(p.accent), 0xffffff, "{k:?} accent 上应是深字");
        }
        // Claude 的红是深红，危险按钮用白字；黑暗的红偏粉，仍是深字
        assert_eq!(text_on(Palette::for_kind(ThemeKind::Claude).red), 0xffffff);
        assert_ne!(text_on(Palette::for_kind(ThemeKind::Dark).red), 0xffffff);
        // mix 端点与中点
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xffffff, 0.5), 0x808080);
    }

    #[test]
    fn set_current_switches_palette() {
        let _g = CURRENT_LOCK.lock().unwrap();
        for k in ThemeKind::ALL {
            set_current(k);
            assert_eq!(current(), k);
            assert_eq!(palette(), Palette::for_kind(k));
            assert_eq!(bg(), Palette::for_kind(k).bg);
            assert_eq!(accent(), Palette::for_kind(k).accent);
            assert_eq!(on_accent(), text_on(Palette::for_kind(k).accent));
        }
        set_current(ThemeKind::Dark);
    }

    #[test]
    fn state_colors() {
        let _g = CURRENT_LOCK.lock().unwrap();
        for k in ThemeKind::ALL {
            set_current(k);
            let p = Palette::for_kind(k);
            assert_eq!(state_color("running"), p.green);
            assert_eq!(state_color("waiting"), p.amber);
            assert_eq!(state_color("exited"), p.red);
            assert_eq!(state_color("idle"), p.faint, "未知状态一律灰");
        }
        set_current(ThemeKind::Dark);
    }

    #[test]
    fn cube_and_gray() {
        assert_eq!(indexed_color(16), 0x000000);
        assert_eq!(indexed_color(231), 0xffffff);
        assert_eq!(indexed_color(232), 0x080808);
        assert_eq!(indexed_color(255), 0xeeeeee);
        // 21 = 16 + 0*36 + 0*6 + 5 → 纯蓝
        assert_eq!(indexed_color(21), 0x0000ff);
        // 调色板版只替换 0-15，其余同一张表
        let cl = Palette::for_kind(ThemeKind::Claude);
        assert_eq!(cl.indexed_color(1), cl.ansi[1]);
        assert_eq!(cl.indexed_color(21), 0x0000ff);
    }

    #[test]
    fn bytes_humanized() {
        assert_eq!(human_bytes(512), "512B");
        assert_eq!(human_bytes(4198), "4.1K");
        assert_eq!(human_bytes(134217728), "128.0M");
        assert_eq!(human_bytes(2469606195), "2.3G");
    }
}
