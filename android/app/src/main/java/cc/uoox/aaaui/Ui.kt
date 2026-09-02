package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.windowsizeclass.WindowWidthSizeClass
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import android.content.Context
import android.graphics.Typeface
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

// ---------- design tokens (PROTOCOL.md · prototype.html) ----------
object Tok {
    val Bg = Color(0xFF0E1216)
    val Surface = Color(0xFF1A222B)
    val Raised = Color(0xFF212B36)
    val Edge = Color(0xFF28323E)
    val Edge2 = Color(0xFF36434F)
    val Ink = Color(0xFFE3EBF3)
    val Dim = Color(0xFF8B99A8)
    val Faint = Color(0xFF5F6D7C)
    val TermBg = Color(0xFF0A0E12)
    val Cyan = Color(0xFF53C6DD)
    val Magenta = Color(0xFFC583E0)
    val Green = Color(0xFF5ECB8F)
    val Amber = Color(0xFFE3B45C)
    val Red = Color(0xFFE57373)

    fun agentColor(agent: String): Color = when (agent) {
        "claude" -> Color(0xFFE8B46A)
        "codex" -> Color(0xFF8FD0FF)
        "pi" -> Color(0xFFB5E08F)
        "reasonix" -> Color(0xFFE08FB5)
        "agy" -> Color(0xFFC8A8F0)
        else -> Dim
    }

    fun stateColor(state: String): Color = when (state) {
        "running" -> Green
        "waiting" -> Amber
        "exited" -> Red
        else -> Dim
    }

    fun agentLabel(agent: String): String = when (agent) {
        "claude" -> "Claude"; "codex" -> "Codex"; "pi" -> "Pi"
        "reasonix" -> "Reasonix"; "agy" -> "Antigravity"; "shell" -> "终端"
        else -> agent
    }
}

@Composable
fun AaaTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Tok.Cyan,
            onPrimary = Color(0xFF08252C),
            background = Tok.Bg,
            surface = Tok.Surface,
            surfaceVariant = Tok.Raised,
            surfaceContainer = Tok.Surface,
            surfaceContainerHigh = Tok.Raised,
            surfaceContainerLow = Tok.Surface,
            onSurface = Tok.Ink,
            onBackground = Tok.Ink,
            onSurfaceVariant = Tok.Dim,
            outline = Tok.Edge2,
            outlineVariant = Tok.Edge,
            secondary = Tok.Dim,
            error = Tok.Red,
        ),
    ) {
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

// ---------- 折叠屏 / 大屏布局 ----------

/** 主导航放哪儿：窄屏底部标签栏，宽屏左侧 rail。 */
enum class NavPlacement { Bottom, Rail }

/**
 * 只有导航位置随宽度变，内容始终单栏。
 *
 * 展开后曾经试过列表 + 会话两栏，实机上信息太碎；宽屏真正的收益只是把横跨整个
 * 屏幕、只装三个 tab 的底栏收成左侧 rail，顺便把内容推高一整条。断点取 Medium
 * (600dp)：OnePlus Open 内屏实测 sw698dp（2268px ÷ 3.25），外屏 343dp 和直板机
 * 都留在底栏。
 *
 * 首页收成单页（项目列表就是首页，设置是压栈路由）之后没有 tab 可摆，首页不再
 * 按这个值切布局；断点本身保留给 PaneLayoutTest 与以后可能的宽屏两栏。
 */
fun navPlacementFor(width: WindowWidthSizeClass): NavPlacement =
    if (width == WindowWidthSizeClass.Compact) NavPlacement.Bottom else NavPlacement.Rail

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
fun AgentChip(agent: String) {
    val c = Tok.agentColor(agent)
    Text(
        if (agent == "shell") "终端" else agent,
        color = c,
        fontFamily = FontFamily.Monospace,
        fontSize = 10.sp,
        modifier = Modifier
            .border(1.dp, c.copy(alpha = 0.5f), RoundedCornerShape(5.dp))
            .padding(horizontal = 5.dp, vertical = 1.dp),
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
