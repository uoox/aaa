package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.connectbot.terminal.Terminal
import org.connectbot.terminal.TerminalEmulator
import kotlin.math.abs
import kotlin.math.roundToInt

// ============================================================
// 终端宿主：SessionScreen（agent 会话的终端视图）与 TerminalScreen（常驻终端面板）共用。
//
// 画面：termlib 的 Terminal()（libvterm 解析、Compose Canvas 渲染、长按选区、链接点击）。
// 键盘：自己的 TermInputView（termlib 自带的那个中文输入法不能组词，见 TermInput.kt），
//       所以 Terminal 的 keyboardEnabled 关掉，点终端 → 我们弹键盘。
// 触摸：TUI 开了鼠标上报（Claude Code：1003+1006，且在备用屏里没有回滚）时，盖一层
//       MouseOverlay 把滑动变成滚轮事件、点按变成点击发给远端，双指捏合改字号；
//       没开鼠标的普通 shell 则让 termlib 自己处理（回滚滚动、选区、放大镜）。
// ============================================================

@Composable
internal fun TerminalHost(
    attachment: TerminalAttachment?,
    fontSize: Int,
    ctrlSticky: MutableState<Boolean>,
    onHyperlinkClick: (String) -> Unit,
    onPasteRequest: () -> Unit,
    /** 双指捏合落定后的新字号（10..22）；由调用方写回设置并提示。 */
    onFontSize: (Int) -> Unit,
    /** 暴露给键位条：⌨ 键把软键盘叫出来。 */
    inputRef: MutableState<TermInputView?>? = null,
) {
    if (attachment == null) {
        Box(Modifier.fillMaxSize().background(Tok.TermBg), contentAlignment = Alignment.Center) {
            Text("未连接 daemon", color = Tok.Faint)
        }
        return
    }
    val ctx = LocalContext.current
    val emulator = attachment.emulator
    val palette = Tok.current
    LaunchedEffect(emulator, palette.name) { applyTerminalPalette(palette, emulator) }
    DisposableEffect(attachment) {
        attachment.onClipboardCopy = { copyToClipboard(ctx, it) }
        onDispose { attachment.onClipboardCopy = null }
    }
    // 终端在前台就别熄屏（跟 termux 那版一致）
    val rootView = LocalView.current
    DisposableEffect(rootView) {
        rootView.keepScreenOn = true
        onDispose { rootView.keepScreenOn = false }
    }
    val sink = remember(attachment, ctrlSticky) { TermInputSink(attachment, ctrlSticky) }
    val sinkState = rememberUpdatedState(sink)
    var input by remember { mutableStateOf<TermInputView?>(null) }
    LaunchedEffect(input) { inputRef?.value = input }
    val modes by attachment.modes.collectAsState()

    Box(Modifier.fillMaxSize().background(palette.termBg)) {
        Terminal(
            terminalEmulator = emulator,
            modifier = Modifier.fillMaxSize(),
            typeface = Fonts.terminal(ctx),
            initialFontSize = fontSize.sp,
            minFontSize = 10.sp,
            maxFontSize = 22.sp,
            backgroundColor = palette.termBg,
            foregroundColor = palette.termFg,
            selectionBackgroundColor = palette.accent,
            selectionForegroundColor = palette.onAccent,
            keyboardEnabled = false,
            onTerminalTap = { input?.showKeyboard() },
            onHyperlinkClick = onHyperlinkClick,
            onPasteRequest = onPasteRequest,
        )
        if (modes.mouseOn) {
            MouseOverlay(
                emulator = emulator,
                modes = modes,
                fontSize = fontSize,
                send = attachment::sendRaw,
                onTap = { input?.showKeyboard() },
                onFontSize = onFontSize,
            )
        }
        AndroidView(
            factory = { c -> TermInputView(c) { sinkState.value }.also { input = it } },
            modifier = Modifier.size(1.dp).align(Alignment.TopStart),
        )
    }
}

/**
 * 鼠标上报模式下接管全部触摸（盖在 Terminal 上面的兄弟节点，Compose 命中测试只给最上面那个）：
 * - 单指滑动：每过一个格高发一个滚轮事件（手指向下 = 内容下来 = 滚轮向上），松手带一点惯性。
 * - 点按：在那个格子发一次按下+松开，然后叫键盘——TUI 的输入框点一下就能打字。
 * - 双指：捏合比例落定后改全局字号。
 * 格子尺寸用 emulator 的行列数除画布像素估算，termlib 没把字宽字高暴露出来。
 */
@Composable
private fun MouseOverlay(
    emulator: TerminalEmulator,
    modes: TermModes,
    fontSize: Int,
    send: (ByteArray) -> Unit,
    onTap: () -> Unit,
    onFontSize: (Int) -> Unit,
) {
    val cur = rememberUpdatedState(modes)
    val fs = rememberUpdatedState(fontSize)
    val scope = rememberCoroutineScope()
    var fling by remember { mutableStateOf<Job?>(null) }
    Box(
        Modifier.fillMaxSize().pointerInput(emulator) {
            awaitEachGesture {
                val down = awaitFirstDown()
                fling?.cancel()
                val dims = emulator.dimensions
                val cellW = size.width.toFloat() / dims.columns.coerceAtLeast(1)
                val cellH = size.height.toFloat() / dims.rows.coerceAtLeast(1)
                val slop = viewConfiguration.touchSlop
                fun cell(p: Offset): Pair<Int, Int> =
                    ((p.x / cellW).toInt() + 1).coerceIn(1, dims.columns.coerceAtLeast(1)) to
                        ((p.y / cellH).toInt() + 1).coerceIn(1, dims.rows.coerceAtLeast(1))
                fun wheel(up: Boolean, at: Offset) {
                    val (c, r) = cell(at)
                    send(Mouse.report(if (up) Mouse.WHEEL_UP else Mouse.WHEEL_DOWN, c, r, press = true, sgr = cur.value.sgrMouse))
                }
                var acc = 0f
                var moved = false
                var zoomed = false
                var zoom = 1f
                var total = Offset.Zero
                var last = down.position
                val velocity = VelocityTracker()
                velocity.addPosition(down.uptimeMillis, down.position)
                while (true) {
                    val ev = awaitPointerEvent()
                    val pressed = ev.changes.filter { it.pressed }
                    if (pressed.isEmpty()) break
                    if (pressed.size >= 2) {
                        zoomed = true
                        zoom *= ev.calculateZoom()
                        ev.changes.forEach { it.consume() }
                        continue
                    }
                    if (zoomed) { ev.changes.forEach { it.consume() }; continue }
                    val ch = ev.changes.firstOrNull { it.id == down.id } ?: pressed.first()
                    velocity.addPosition(ch.uptimeMillis, ch.position)
                    val delta = ch.position - last
                    last = ch.position
                    total += delta
                    if (!moved && total.getDistance() > slop) moved = true
                    if (moved) {
                        acc += delta.y
                        while (acc >= cellH) { acc -= cellH; wheel(up = true, at = ch.position) }
                        while (acc <= -cellH) { acc += cellH; wheel(up = false, at = ch.position) }
                    }
                    ch.consume()
                }
                when {
                    zoomed -> {
                        val target = (fs.value * zoom).roundToInt().coerceIn(10, 22)
                        if (target != fs.value) onFontSize(target)
                    }
                    moved -> {
                        // 惯性：按松手速度再补几行，每 16ms 一行，最多 40 行
                        val vy = velocity.calculateVelocity().y
                        val lines = (abs(vy) / cellH * 0.25f).toInt().coerceAtMost(40)
                        if (lines > 0) fling = scope.launch {
                            repeat(lines) { wheel(up = vy > 0, at = last); delay(16) }
                        }
                    }
                    else -> {
                        val (c, r) = cell(down.position)
                        val sgr = cur.value.sgrMouse
                        send(Mouse.report(Mouse.LEFT, c, r, press = true, sgr = sgr))
                        send(Mouse.report(Mouse.LEFT, c, r, press = false, sgr = sgr))
                        onTap()
                    }
                }
            }
        },
    )
}

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
