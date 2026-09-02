package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import android.view.KeyEvent
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.termux.view.TerminalView
import com.termux.view.TerminalViewClient
import org.connectbot.terminal.ModifierManager
import org.connectbot.terminal.Terminal
import org.connectbot.terminal.VTermKey

// ============================================================
// 终端宿主：SessionScreen（agent 会话的终端视图）与 TerminalScreen（常驻终端面板）共用
// ============================================================

@Composable
internal fun TerminalHost(
    attachment: TerminalAttachment?,
    viewRef: MutableState<TerminalView?>,
    fontSize: Int,
    viewClientFactory: (TerminalView) -> TerminalViewClient,
) {
    val density = LocalDensity.current
    if (attachment == null) {
        Box(Modifier.fillMaxSize().background(Tok.TermBg), contentAlignment = Alignment.Center) {
            Text("未连接 daemon", color = Tok.Faint)
        }
        return
    }
    AndroidView(
        factory = { ctx ->
            TerminalView(ctx, null).apply {
                // 画布底色随主题；具体值由 update 按 Tok.current 维护（换主题时它会再跑一次）
                setBackgroundColor(Tok.TermBg.toArgb())
                // 代码里 new 出来的 View 默认不可聚焦（termux 原本靠布局 XML 里的
                // focusable / focusableInTouchMode）。不设这两项 requestFocus() 直接
                // 返回 false，软键盘弹不出来，IME 输入也就永远到不了 PTY。
                isFocusable = true
                isFocusableInTouchMode = true
                setTerminalViewClient(viewClientFactory(this))
                // setTextSize must come first: it constructs the renderer (and
                // is null-safe), while setTypeface reads the existing one and
                // would NPE on a freshly built view.
                setTextSize(with(density) { fontSize.sp.toPx() }.toInt())
                // 随包的 JetBrains Mono，比系统等宽字体窄一截（见 Fonts）。后续
                // update 里的 setTextSize 会沿用现有渲染器的字体，不用再设。
                setTypeface(Fonts.terminal(ctx))
                attachSession(attachment.session)
                keepScreenOn = true
                viewRef.value = this
            }
        },
        update = { view ->
            val px = with(density) { fontSize.sp.toPx() }.toInt()
            val palette = Tok.current // snapshot state：换主题这个 lambda 会被重新执行
            val was = view.tag as? HostState
            if (was?.px != px) view.setTextSize(px)
            if (was?.theme != palette.name) {
                // 全局配色表 AaaTheme 已经改好；这里让这个会话已存在的模拟器重读默认前景/背景，
                // 再把画布底色换掉——模拟器只画非默认底色的格子，其余露出来的就是它。
                applyTerminalPalette(palette, attachment.session)
                view.setBackgroundColor(palette.termBg.toArgb())
                view.onScreenUpdated()
            }
            if (was?.px != px || was?.theme != palette.name) view.tag = HostState(px, palette.name)
            if (view.currentSession !== attachment.session) view.attachSession(attachment.session)
        },
        modifier = Modifier.fillMaxSize().background(Tok.TermBg),
    )
}


/**
 * termlib 宿主：Compose 原生的 Terminal()，渲染、选区、缩放、链接点击都在 Compose 层。
 *
 * - 输出/输入/尺寸都接在 [TerminalAttachment.termlib] 上，这里只负责画与键盘。
 * - 字号跟设置走（Terminal 内部 LaunchedEffect(initialFontSize) 会重算格子）；双指缩放
 *   是它自己的临时状态，不写回设置——和 termux 那套「一捏改全局」刻意不同，先看哪种顺手。
 * - 粘性 Ctrl 通过 [ModifierManager] 注入：Terminal 每发出一个键就调 clearTransients()。
 * - 主题切换：Tok.current 变化时重喂 16 色 + 前景/背景。
 */
@Composable
internal fun TermlibHost(
    attachment: TerminalAttachment?,
    fontSize: Int,
    ctrlSticky: MutableState<Boolean>,
    onHyperlinkClick: (String) -> Unit,
    onPasteRequest: () -> Unit,
) {
    if (attachment == null) {
        Box(Modifier.fillMaxSize().background(Tok.TermBg), contentAlignment = Alignment.Center) {
            Text("未连接 daemon", color = Tok.Faint)
        }
        return
    }
    val ctx = androidx.compose.ui.platform.LocalContext.current
    val emulator = attachment.termlib
    val palette = Tok.current
    LaunchedEffect(emulator, palette.name) { applyTermlibPalette(palette, emulator) }
    DisposableEffect(attachment) {
        attachment.onClipboardCopy = { copyToClipboard(ctx, it) }
        onDispose { if (attachment.onClipboardCopy != null) attachment.onClipboardCopy = null }
    }
    val modifiers = remember(ctrlSticky) {
        object : ModifierManager {
            override fun isCtrlActive() = ctrlSticky.value
            override fun isAltActive() = false
            override fun isShiftActive() = false
            override fun clearTransients() { ctrlSticky.value = false }
        }
    }
    Terminal(
        terminalEmulator = emulator,
        modifier = Modifier.fillMaxSize().background(palette.termBg),
        typeface = Fonts.terminal(ctx),
        initialFontSize = fontSize.sp,
        minFontSize = 10.sp,
        maxFontSize = 22.sp,
        backgroundColor = palette.termBg,
        foregroundColor = palette.termFg,
        selectionBackgroundColor = palette.accent,
        selectionForegroundColor = palette.onAccent,
        keyboardEnabled = true,
        modifierManager = modifiers,
        onHyperlinkClick = onHyperlinkClick,
        onPasteRequest = onPasteRequest,
    )
}

/**
 * 键位条的 Android 键码 → libvterm 键。只覆盖键位条上有的几个；不认识的返回 null，
 * 调用方就不发。libvterm 会按当前 keypad / cursor 模式给出正确转义序列，与 termux 的
 * KeyHandler 同一职责。
 */
internal fun vtermKeyFor(keyCode: Int): Int? = when (keyCode) {
    KeyEvent.KEYCODE_ESCAPE -> VTermKey.ESCAPE
    KeyEvent.KEYCODE_TAB -> VTermKey.TAB
    KeyEvent.KEYCODE_ENTER -> VTermKey.ENTER
    KeyEvent.KEYCODE_DPAD_UP -> VTermKey.UP
    KeyEvent.KEYCODE_DPAD_DOWN -> VTermKey.DOWN
    KeyEvent.KEYCODE_DPAD_LEFT -> VTermKey.LEFT
    KeyEvent.KEYCODE_DPAD_RIGHT -> VTermKey.RIGHT
    KeyEvent.KEYCODE_MOVE_HOME -> VTermKey.HOME
    KeyEvent.KEYCODE_MOVE_END -> VTermKey.END
    KeyEvent.KEYCODE_DEL -> VTermKey.BACKSPACE
    KeyEvent.KEYCODE_FORWARD_DEL -> VTermKey.DEL
    KeyEvent.KEYCODE_PAGE_UP -> VTermKey.PAGEUP
    KeyEvent.KEYCODE_PAGE_DOWN -> VTermKey.PAGEDOWN
    else -> null
}

/** libvterm 的修饰键掩码：bit0 Shift、bit1 Alt、bit2 Ctrl（见 termlib KeyboardHandler.getModifierMask）。 */
internal const val VTERM_MOD_CTRL = 4

/** 会话页顶栏那颗视图切换按钮的循环顺序：消息流 → Termux → Termlib → 消息流；消息流不可用时在两套终端间来回。 */
internal fun nextUiMode(current: String, messagesSupported: Boolean): String = when (current) {
    "messages" -> "termux"
    "termux" -> "termlib"
    else -> if (messagesSupported) "messages" else "termux"
}

/** update 用来判「字号 / 主题变了没有」的标记，挂在 view.tag 上。 */
private data class HostState(val px: Int, val theme: String)

@Composable
internal fun KeyChip(label: String, active: Boolean = false, onClick: () -> Unit) {
    Text(
        label,
        color = if (active) Tok.OnAccent else Tok.Ink,
        fontSize = 13.sp,
        fontFamily = FontFamily.Monospace,
        modifier = Modifier
            .background(if (active) Tok.Accent else Tok.Raised, RoundedCornerShape(7.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
    )
}
