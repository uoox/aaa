package cc.uoox.aaaui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.graphics.Typeface
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.graphics.compositeOver
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

// ---------- 配色 ----------

/**
 * 设计 token（PROTOCOL.md「设计令牌」）：Anthropic 象牙白 + 陶土橙，终端暖白底墨字。
 *
 * 2026-09-10 用户拍板「不需要黑暗模式，仅保留一个主题即可，精简代码」：以前是两套配色
 * 加一个 snapshot state，换主题时整棵树跟着重组。只剩一套之后这些全是常量——组合期读到的
 * 就是编译期的值，非组合代码（TerminalHost 的 View 回调、TerminalAttachment）也直接读。
 */
object Tok {
    val Bg = Color(0xFFFAF9F5)
    val Surface = Color(0xFFF0EEE6)
    val Raised = Color(0xFFE8E6DC)
    val Edge = Color(0xFFDAD8CE)
    val Edge2 = Color(0xFFC8C6BC)
    val Ink = Color(0xFF141413)
    val Dim = Color(0xFF5E5D59)
    val Faint = Color(0xFF91908A)
    /** 主操作 / 链接 / 选中那一行的标题与边框。 */
    val Accent = Color(0xFFD97757)
    /** 压在强调色上的文字（FAB 的 ＋、键位条选中态）。 */
    val OnAccent = Color(0xFF141413)
    /** 配对页大标题用的第二强调色（哑紫，不取橙的邻色，否则和 Accent 分不开）。 */
    val Magenta = Color(0xFF9B6B9E)
    val Green = Color(0xFF2F855A)
    val Amber = Color(0xFFB8860B)
    val Red = Color(0xFFC0392B)
    /** 看板卡片标题前「在跑」那根线。Green/Amber/Red 各有旧含义，Accent 是橙，只有蓝读作「它自己在动」。 */
    val Blue = Color(0xFF3F6EA8)
    /** 代码块 / 思考行 / 工具输出这类「凹下去」的小面板底色——界面侧的，跟终端无关。 */
    val Inset = Color(0xFFFFFEFA)

    /** 项目行「在跑」的淡蓝底（2026-09-10 起整行淡底代替行尾竖线）。 */
    val RowRunning = Color(0xFFDDE7F1)
    /** 项目行「未读」的淡黄底：跑完了 / 在等你，而这台设备还没看过。 */
    val RowUnread = Color(0xFFF6E7C9)

    /** 终端画布底色与默认前景。 */
    val TermBg = Color(0xFFFFFDF7)
    val TermFg = Color(0xFF141413)

    /**
     * 终端 ANSI 16 色（gruvbox-light），与 mac `theme.rs::ANSI` 逐色相同、共享向量
     * `fixtures/tokens.json` 钉着。**没有「不带就用 xterm 默认表」这条回退**——那是
     * 「同一段输出两端颜色不一样」的唯一来源，而 xterm 的亮黄 / 亮青落在白纸上根本看不见。
     * 浅底上 8–15 比 0–7 更沉而不是更亮，7 white 是暖灰，15 bright white = Ink。
     */
    val TerminalAnsi: IntArray = intArrayOf(
        0xFF3C3836.toInt(), 0xFFCC241D.toInt(), 0xFF98971A.toInt(), 0xFFD79921.toInt(),
        0xFF458588.toInt(), 0xFFB16286.toInt(), 0xFF689D6A.toInt(), 0xFFA89984.toInt(),
        0xFF7C6F64.toInt(), 0xFF9D0006.toInt(), 0xFF79740E.toInt(), 0xFFB57614.toInt(),
        0xFF076678.toInt(), 0xFF8F3F71.toInt(), 0xFF427B58.toInt(), 0xFF141413.toInt(),
    )

    fun stateColor(state: String): Color = when (state) {
        "running" -> Green
        "waiting" -> Amber
        "exited" -> Red
        else -> Dim
    }
}

/**
 * Material3 配色表：对话框、底部单、文本框、开关、分段按钮这些没有显式传色的控件从这里取。
 * selected 容器给强调色的淡底压在 Surface 上，而不是 M3 默认那套紫灰。
 */
val AaaColorScheme: ColorScheme = lightColorScheme(
    primary = Tok.Accent, onPrimary = Tok.OnAccent,
    primaryContainer = Tok.Accent.copy(alpha = 0.18f).compositeOver(Tok.Surface), onPrimaryContainer = Tok.Ink,
    secondary = Tok.Dim, onSecondary = Tok.Surface,
    secondaryContainer = Tok.Accent.copy(alpha = 0.18f).compositeOver(Tok.Surface), onSecondaryContainer = Tok.Ink,
    background = Tok.Bg, onBackground = Tok.Ink,
    surface = Tok.Surface, onSurface = Tok.Ink,
    surfaceVariant = Tok.Raised, onSurfaceVariant = Tok.Dim,
    surfaceContainerLowest = Tok.Surface, surfaceContainerLow = Tok.Surface, surfaceContainer = Tok.Surface,
    surfaceContainerHigh = Tok.Raised, surfaceContainerHighest = Tok.Raised,
    surfaceTint = Tok.Accent,
    outline = Tok.Edge2, outlineVariant = Tok.Edge,
    error = Tok.Red, onError = Tok.Surface,
)

/** 配色喂给 libvterm：16 色 + 默认前景/背景。 */
fun applyTerminalPalette(emulator: org.connectbot.terminal.TerminalEmulator) {
    emulator.applyColorScheme(Tok.TerminalAnsi, Tok.TermFg.toArgb(), Tok.TermBg.toArgb())
}

/** 全局主题。两个 Activity（主界面、分享目标）都走这里。 */
@Composable
fun AaaTheme(content: @Composable () -> Unit) {
    val view = LocalView.current
    SideEffect {
        // 亮界面 → 系统栏黑图标。API 35 起 setStatusBarColor 是空操作（强制 edge-to-edge，
        // 底色由下面那个 Box 透上去），老系统上仍要它把栏染成 Bg。
        val window = view.context.findActivity()?.window
        if (window != null && !view.isInEditMode) {
            @Suppress("DEPRECATION")
            window.statusBarColor = Tok.Bg.toArgb()
            @Suppress("DEPRECATION")
            window.navigationBarColor = Tok.Bg.toArgb()
            WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = true
                isAppearanceLightNavigationBars = true
            }
        }
    }
    MaterialTheme(colorScheme = AaaColorScheme) {
        // Keep every screen clear of the status and navigation bars. Screen
        // heights differ enough between devices (a foldable's cover display
        // has a taller status bar than a plain phone) that a layout which
        // merely looks right on one of them will slide its header under the
        // clock on another.
        Box(
            Modifier
                .fillMaxSize()
                .background(Tok.Bg)
                .systemBarsPadding(),
        ) { content() }
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** 打开会话的统一入口，由 AaaApp 提供（压栈到 session/{id}）。 */
val LocalOpenSession = staticCompositionLocalOf<(String, String) -> Unit> {
    error("LocalOpenSession 未提供")
}

@Composable
fun StateDot(color: Color, size: Int = 8) {
    Spacer(
        Modifier.size(size.dp).background(color, CircleShape)
    )
}

@Composable
fun DotWithText(color: Color, text: String, textColor: Color = Tok.Dim) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        StateDot(color)
        Spacer(Modifier.width(6.dp))
        Text(text, color = textColor, fontSize = 12.sp)
    }
}

// ---------- fonts ----------

/**
 * 终端字体：随包带的 JetBrains Mono（OFL 1.1）。系统 Typeface.MONOSPACE 在两台
 * 测试机上都是 Droid Sans Mono，字宽偏大，同一屏能放下的列数明显少。termux 渲染器
 * 用 measureText("X") 定格宽，换字体本身就是把格子变窄的修法。
 *
 * 用的是官方 **NL（No Ligatures）** 变体：字形与度量完全一样，只去掉了连字表。
 * 带连字的那版在 9R 上实测把 `|-` 画成 ⊢、`->` 画成 →——终端里 `!=` `||` `//`
 * 这些都会被吞掉，看着像另一个字符；渲染器一段同色文字一次 drawText，Paint 会
 * 默认套 liga/calt，又没有暴露 setFontFeatureSettings，所以从字体源头去掉。
 * Typeface.createFromAsset 走磁盘，进程内缓存一份——折叠/展开重建 SessionScreen 不重读。
 */
object Fonts {
    const val TERMINAL_ASSET = "fonts/JetBrainsMonoNL-Regular.ttf"

    @Volatile private var terminal: Typeface? = null

    fun terminal(context: Context): Typeface =
        terminal ?: synchronized(this) {
            terminal ?: runCatching { Typeface.createFromAsset(context.assets, TERMINAL_ASSET) }
                .getOrDefault(Typeface.MONOSPACE)
                .also { terminal = it }
        }
}

// ---------- formatting ----------

fun humanBytes(bytes: Long): String {
    // 规则与 mac `theme.rs::human_bytes` 逐字相同：一律一位小数。以前这边 K 不带小数、
    // M 还按大小分两档，同一个文件在两端显示成不同的大小（v1.22 对齐）。
    if (bytes < 1024) return "${bytes}B"
    val kb = bytes / 1024.0
    if (kb < 1024) return "%.1fK".format(kb)
    val mb = kb / 1024.0
    if (mb < 1024) return "%.1fM".format(mb)
    return "%.1fG".format(mb / 1024.0)
}

fun relativeTime(iso: String): String {
    if (iso.isBlank()) return ""
    return try {
        val t = Instant.parse(iso)
        val now = Instant.now()
        val s = java.time.Duration.between(t, now).seconds
        when {
            s < 60 -> "刚刚"
            s < 3600 -> "${s / 60} 分钟前"
            s < 86400 -> "${s / 3600} 小时前"
            else -> {
                val local = t.atZone(ZoneId.systemDefault())
                local.format(DateTimeFormatter.ofPattern("MM-dd HH:mm"))
            }
        }
    } catch (_: Exception) { iso }
}

/**
 * ☰ 左侧栏（项目面板）占屏幕宽度的比例：**小屏铺满，大屏半屏**（2026-09-08 用户拍板：
 * 「整个左侧栏的宽度：小屏情况下，直接铺满，大屏情况下，半屏」）。
 *
 * 手机竖屏（343dp）本来就只装得下一栏，抽屉再留一条边等于白扔；折叠机内屏 / 平板 /
 * 横屏上，右半屏是**正在看的那个对话**，抽屉盖住一半、露出一半，切之前先看得见要切去
 * 哪儿。先前那个固定上限（400dp）两头都不对——小屏偏窄，大屏也偏窄，且和屏幕多宽没关系。
 *
 * 门槛取 Material 的 compact / medium 分界 600dp：折叠机内屏（≈674dp）落在大屏一侧，
 * 外屏和普通手机落在小屏一侧。横屏的手机也算大屏——那时右边确实有半屏内容可看。
 *
 * `ModalDrawerSheet` 内部有一句 `sizeIn(maxWidth = 360dp)`，但外层给的是**定宽**约束，
 * `sizeIn` 会被夹回定宽，所以这里的比例说了算（装机实测过 400dp 那版确实生效）。
 */
@Composable
fun sidebarFraction(): Float = sidebarFraction(LocalConfiguration.current.screenWidthDp)

/** 纯函数那一半，好测：屏幕宽 [screenWidthDp] → 左侧栏占屏比例 */
fun sidebarFraction(screenWidthDp: Int): Float = if (screenWidthDp < SIDEBAR_WIDE_DP) 1f else 0.5f

/** 「大屏」的门槛，dp。Material 的 compact / medium 分界 */
const val SIDEBAR_WIDE_DP = 600

/** ISO 时间戳 → 本地 HH:mm（消息流用户块上方的小时间）；解析不了给空串。 */
fun clockTime(iso: String): String = try {
    Instant.parse(iso).atZone(ZoneId.systemDefault()).format(DateTimeFormatter.ofPattern("HH:mm"))
} catch (_: Exception) { "" }

/** 用量段落拼成一行：百分比段按级别着色，其它段用 [base]；分隔符是 ` · ` */
fun segmentsAnnotated(segs: List<UsageSegment>, base: Color): AnnotatedString = buildAnnotatedString {
    segs.forEachIndexed { i, seg ->
        if (i > 0) withStyle(SpanStyle(color = base)) { append(" · ") }
        withStyle(SpanStyle(color = pctColor(seg.level, base))) { append(seg.text) }
    }
}

fun pctColor(level: PctLevel, base: Color): Color = when (level) {
    PctLevel.CRIT -> Tok.Red
    PctLevel.WARN -> Tok.Amber
    PctLevel.NORMAL -> base
}
