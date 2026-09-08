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
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
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
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

// ---------- 配色 ----------

/**
 * 一套完整配色。两套：黑暗（PROTOCOL.md 的原始 token，一个值都没改）与
 * Claude 橙（Anthropic 品牌的米白 + 赭橙，终端也是暖白底墨字）。纯数据，PaletteTest 直接跑。
 *
 * [isDark] 说的是**界面**底色深浅——决定系统栏图标颜色与 Material 基线。
 */
data class Palette(
    /** 设置里存的键：dark / claude。 */
    val name: String,
    /** 设置页显示的名字。 */
    val label: String,
    val isDark: Boolean,
    val bg: Color,
    val surface: Color,
    val raised: Color,
    val edge: Color,
    val edge2: Color,
    val ink: Color,
    val dim: Color,
    val faint: Color,
    /** 强调色（黑暗主题里是青色，Claude 橙里是赭橙），FAB / 链接 / 选中态都用它。 */
    val accent: Color,
    /** 压在强调色上的文字（FAB 的 ＋、键位条选中态）。 */
    val onAccent: Color,
    /** 配对页大标题用的第二强调色。 */
    val magenta: Color,
    val green: Color,
    val amber: Color,
    val red: Color,
    /**
     * 项目行首「在跑」那一点。三套主题里都要是**蓝**：green/amber/red 各有旧含义，
     * accent 又随主题从青变成橙，只有蓝在三套里都读作「它自己在动」。
     */
    val blue: Color,
    /** 代码块 / 思考行 / 工具输出这类「凹下去」的小面板底色——界面侧的，跟终端无关。 */
    val inset: Color,
    /** 终端画布底色与默认前景。 */
    val termBg: Color,
    val termFg: Color,
    /**
     * 终端 ANSI 16 色。null = 沿用 xterm 默认表（为黑底设计）；亮底主题必须
     * 自带一套——xterm 的亮黄 #FFFF54、亮青 #54FFFF 落在白纸上根本看不见，而 Claude Code
     * 的警告 / 提示恰恰爱用这几个。规则同 mac：浅底上 8–15 比 0–7 更沉而不是更亮。
     */
    val ansi: List<Color>? = null,
) {
    init { require(ansi == null || ansi.size == 16) { "ansi 必须是 16 色" } }

    companion object {
        val Dark = Palette(
            name = "dark", label = "黑暗", isDark = true,
            bg = Color(0xFF0E1216), surface = Color(0xFF1A222B), raised = Color(0xFF212B36),
            edge = Color(0xFF28323E), edge2 = Color(0xFF36434F),
            ink = Color(0xFFE3EBF3), dim = Color(0xFF8B99A8), faint = Color(0xFF5F6D7C),
            accent = Color(0xFF53C6DD), onAccent = Color(0xFF08252C), magenta = Color(0xFFC583E0),
            green = Color(0xFF5ECB8F), amber = Color(0xFFE3B45C), red = Color(0xFFE57373), blue = Color(0xFF6C9EF8),
            inset = Color(0xFF0A0E12), termBg = Color(0xFF0A0E12), termFg = Color(0xFFFFFFFF),
        )
        val Claude = Palette(
            name = "claude", label = "Claude 橙", isDark = false,
            bg = Color(0xFFFAF9F5), surface = Color(0xFFF0EEE6), raised = Color(0xFFE8E6DC),
            edge = Color(0xFFDAD8CE), edge2 = Color(0xFFC8C6BC),
            ink = Color(0xFF141413), dim = Color(0xFF5E5D59), faint = Color(0xFF91908A),
            accent = Color(0xFFD97757), onAccent = Color(0xFF141413), magenta = Color(0xFFD97757),
            green = Color(0xFF2F855A), amber = Color(0xFFB8860B), red = Color(0xFFC0392B), blue = Color(0xFF3F6EA8),
            inset = Color(0xFFE8E6DC), termBg = Color(0xFFFFFDF7), termFg = Color(0xFF141413),
            // gruvbox-light；与 mac 的 CLAUDE.ansi 逐色相同
            ansi = listOf(
                0xFF3C3836, 0xFFCC241D, 0xFF98971A, 0xFFD79921, 0xFF458588, 0xFFB16286, 0xFF689D6A, 0xFFA89984,
                0xFF7C6F64, 0xFF9D0006, 0xFF79740E, 0xFFB57614, 0xFF076678, 0xFF8F3F71, 0xFF427B58, 0xFF141413,
            ).map(::Color),
        )

        /** 设置页的排列顺序。 */
        val all: List<Palette> = listOf(Dark, Claude)

        /**
         * 设置里存的名字 → 配色；不认识的（含 null、旧版本没写过、以及 2026-09-08
         * 删掉的 "light"）一律黑暗。存量设置因此不需要迁移。
         */
        fun forName(name: String?): Palette = all.firstOrNull { it.name == name } ?: Dark
    }
}

/**
 * 设计 token（PROTOCOL.md「设计令牌」）。调用点仍旧读 `Tok.Bg`、`Tok.Ink`……，
 * 值来自 [current]。current 是 snapshot state：组合期读到的每一个 Tok.X 都被 Compose
 * 追踪，换主题时凡是画过颜色的地方自动重组，不必整棵树 key 重建；非组合代码
 * （TerminalHost 里的 View 回调、AndroidView.update）读到的就是当下的值。
 */
object Tok {
    var current: Palette by mutableStateOf(Palette.Dark)

    val Bg: Color get() = current.bg
    val Surface: Color get() = current.surface
    val Raised: Color get() = current.raised
    val Edge: Color get() = current.edge
    val Edge2: Color get() = current.edge2
    val Ink: Color get() = current.ink
    val Dim: Color get() = current.dim
    val Faint: Color get() = current.faint
    val Accent: Color get() = current.accent
    val OnAccent: Color get() = current.onAccent
    val Magenta: Color get() = current.magenta
    val Green: Color get() = current.green
    val Amber: Color get() = current.amber
    val Red: Color get() = current.red
    val Blue: Color get() = current.blue
    val Inset: Color get() = current.inset
    val TermBg: Color get() = current.termBg
    val TermFg: Color get() = current.termFg

    fun stateColor(state: String): Color = when (state) {
        "running" -> Green
        "waiting" -> Amber
        "exited" -> Red
        else -> Dim
    }
}

/**
 * Material3 配色表跟着 Palette 走：对话框、底部单、文本框、开关、分段按钮这些没有
 * 显式传色的控件从这里取。selected 容器给强调色的淡底压在 surface 上，而不是 M3
 * 默认那套紫灰。
 */
fun Palette.materialScheme(): ColorScheme {
    val accentTint = accent.copy(alpha = 0.18f).compositeOver(surface)
    return if (isDark) darkColorScheme(
        primary = accent, onPrimary = onAccent,
        primaryContainer = accentTint, onPrimaryContainer = ink,
        secondary = dim, onSecondary = bg,
        secondaryContainer = accentTint, onSecondaryContainer = ink,
        background = bg, onBackground = ink,
        surface = surface, onSurface = ink,
        surfaceVariant = raised, onSurfaceVariant = dim,
        surfaceContainerLowest = bg, surfaceContainerLow = surface, surfaceContainer = surface,
        surfaceContainerHigh = raised, surfaceContainerHighest = raised,
        surfaceTint = accent,
        outline = edge2, outlineVariant = edge,
        error = red, onError = bg,
    ) else lightColorScheme(
        primary = accent, onPrimary = onAccent,
        primaryContainer = accentTint, onPrimaryContainer = ink,
        secondary = dim, onSecondary = surface,
        secondaryContainer = accentTint, onSecondaryContainer = ink,
        background = bg, onBackground = ink,
        surface = surface, onSurface = ink,
        surfaceVariant = raised, onSurfaceVariant = dim,
        surfaceContainerLowest = surface, surfaceContainerLow = surface, surfaceContainer = surface,
        surfaceContainerHigh = raised, surfaceContainerHighest = raised,
        surfaceTint = accent,
        outline = edge2, outlineVariant = edge,
        error = red, onError = surface,
    )
}

/** 暗色主题的 ANSI 16 色：xterm 默认表（termux 出厂同一套），为黑底设计。 */
internal val XTERM_ANSI: IntArray = intArrayOf(
    0xFF000000.toInt(), 0xFFCD0000.toInt(), 0xFF00CD00.toInt(), 0xFFCDCD00.toInt(),
    0xFF0000EE.toInt(), 0xFFCD00CD.toInt(), 0xFF00CDCD.toInt(), 0xFFE5E5E5.toInt(),
    0xFF7F7F7F.toInt(), 0xFFFF0000.toInt(), 0xFF00FF00.toInt(), 0xFFFFFF00.toInt(),
    0xFF5C5CFF.toInt(), 0xFFFF00FF.toInt(), 0xFF00FFFF.toInt(), 0xFFFFFFFF.toInt(),
)

/** 主题里 libvterm 实际会用的 16 色：主题没自带 ANSI 表的用 xterm 默认。 */
fun terminalAnsi(p: Palette): IntArray = IntArray(16) { i -> p.ansi?.get(i)?.toArgb() ?: XTERM_ANSI[i] }

/** 配色喂给 libvterm：16 色 + 默认前景/背景。 */
fun applyTerminalPalette(p: Palette, emulator: org.connectbot.terminal.TerminalEmulator) {
    emulator.applyColorScheme(terminalAnsi(p), p.termFg.toArgb(), p.termBg.toArgb())
}

/** 读设置里的主题，交给 [AaaTheme]。两个 Activity（主界面、分享目标）都走这里。 */
@Composable
fun AaaTheme(store: AppStore, content: @Composable () -> Unit) {
    val settings by store.settings.flow.collectAsState(initial = null)
    AaaTheme(theme = settings?.theme, content = content)
}

@Composable
fun AaaTheme(theme: String?, content: @Composable () -> Unit) {
    val palette = remember(theme) { Palette.forName(theme) }
    val view = LocalView.current
    SideEffect {
        // 写在 SideEffect 里而不是组合期：Tok.current 是被追踪的 state，组合期改它
        // 会被判成反向写入。这一帧 MaterialTheme 已经用新 palette 画，Tok 读者下一帧跟上。
        if (Tok.current != palette) Tok.current = palette
        // 系统栏：亮主题黑图标，暗主题白图标。API 35 起 setStatusBarColor 是空操作
        // （强制 edge-to-edge，底色由下面那个 Box 透上去），老系统上仍要它把栏染成 Bg。
        val window = view.context.findActivity()?.window
        if (window != null && !view.isInEditMode) {
            @Suppress("DEPRECATION")
            window.statusBarColor = palette.bg.toArgb()
            @Suppress("DEPRECATION")
            window.navigationBarColor = palette.bg.toArgb()
            WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = !palette.isDark
                isAppearanceLightNavigationBars = !palette.isDark
            }
        }
    }
    MaterialTheme(colorScheme = palette.materialScheme()) {
        // Keep every screen clear of the status and navigation bars. Screen
        // heights differ enough between devices (a foldable's cover display
        // has a taller status bar than a plain phone) that a layout which
        // merely looks right on one of them will slide its header under the
        // clock on another.
        Box(
            Modifier
                .fillMaxSize()
                .background(palette.bg)
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
    if (bytes < 1024) return "${bytes}B"
    val kb = bytes / 1024.0
    if (kb < 1024) return "%.0fK".format(kb)
    val mb = kb / 1024.0
    if (mb < 1024) return if (mb < 10) "%.1fM".format(mb) else "%.0fM".format(mb)
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
