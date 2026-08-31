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

/** 列表与会话详情的排布：单栏（直板机、外屏、半折）或左右两栏（内屏展开）。 */
enum class PaneLayout { Single, Dual }

/**
 * Medium（≥600dp）起进两栏。
 *
 * 门槛不是 Expanded：实测 OnePlus Open 内屏展开是 **sw698dp**（2268px ÷ 3.25），
 * 落在 Medium 里——按 Expanded 判，这台机器上双栏永远不会出现，等于这个功能对
 * 它要服务的设备无效。外屏 343dp、9R 直板都还在 Compact，行为不变。
 */
fun paneLayoutFor(width: WindowWidthSizeClass): PaneLayout =
    if (width == WindowWidthSizeClass.Compact) PaneLayout.Single else PaneLayout.Dual

/**
 * 两栏时左侧列表的固定宽度；右侧终端拿走剩下的，保证它总是最宽的那一栏。
 * 280 而不是更宽：698dp 的内屏减掉之后终端还剩 418dp，12sp 等宽字体下约 58 列，
 * 够看；列表这边放得下项目名 + 状态点 + 相对时间，本来也不需要更多。
 */
val ListPaneWidth = 280.dp

/** 「打开某个会话」的去向。单栏压栈到 session/{id}，两栏只换右栏选中项。 */
sealed class OpenTarget {
    data class Push(val id: String, val prefill: String) : OpenTarget()
    data class Select(val id: String, val prefill: String) : OpenTarget()
}

/**
 * 会话卡片、通知深链、项目页「继续会话」全都走这里算去向，折叠状态一变
 * 三处行为才不会各走各的。拼路由（要 Uri.encode）留给调用方，这里保持纯函数。
 */
fun openTargetFor(pane: PaneLayout, id: String, prefill: String = ""): OpenTarget = when (pane) {
    PaneLayout.Dual -> OpenTarget.Select(id, prefill)
    PaneLayout.Single -> OpenTarget.Push(id, prefill)
}

/**
 * 右栏选中项在会话列表刷新后的取舍：会话被删掉就回空态，否则原样保留。
 * 单栏↔两栏来回切时列表会重排，不能因为重排就丢掉当前会话。
 */
fun retainSelection(selected: String?, sessions: List<Session>): String? =
    selected?.takeIf { id -> sessions.any { it.id == id } }

/** 打开会话的统一入口，由 AaaApp 按当前 [PaneLayout] 提供。 */
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
