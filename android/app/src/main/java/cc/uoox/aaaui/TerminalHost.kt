package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.findViewTreeLifecycleOwner
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
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
//       长按进入「选择模式」——盖层撤下，termlib 原生的长按选区 / 点链接接管，
//       点一下空处退出。没开鼠标的普通 shell 一直是 termlib 原生手势。
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
    /** daemon 侧的屏幕文本（鼠标模式下点到链接要靠它定位；termlib 不暴露格子内容）。 */
    screenText: suspend () -> String?,
    /** 选择模式开关：键位条的「选择」键和长按都切它；退出由点空处触发。 */
    selectMode: MutableState<Boolean>,
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
    val scope = rememberCoroutineScope()
    val emulator = attachment.emulator
    val palette = Tok.current
    LaunchedEffect(emulator, palette.name) { applyTerminalPalette(palette, emulator) }
    DisposableEffect(attachment) {
        attachment.onClipboardCopy = { copyToClipboard(ctx, it) }
        onDispose { attachment.onClipboardCopy = null }
    }
    // 终端在前台就别熄屏；回到前台时把自己的行列重新宣告给 daemon——
    // 「谁在看谁说了算」：mac 那边在这期间改过尺寸，手机一回来就夺回
    val rootView = LocalView.current
    DisposableEffect(rootView, attachment) {
        rootView.keepScreenOn = true
        val owner = rootView.findViewTreeLifecycleOwner()
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_RESUME) attachment.resendSize()
        }
        owner?.lifecycle?.addObserver(observer)
        onDispose {
            rootView.keepScreenOn = false
            owner?.lifecycle?.removeObserver(observer)
        }
    }
    val sink = remember(attachment, ctrlSticky) { TermInputSink(attachment, ctrlSticky) }
    val sinkState = rememberUpdatedState(sink)
    var input by remember { mutableStateOf<TermInputView?>(null) }
    LaunchedEffect(input) { inputRef?.value = input }
    val modes by attachment.modes.collectAsState()
    var selecting by selectMode
    // 鼠标模式一关（TUI 退出），选择模式也没意义了
    LaunchedEffect(modes.mouseOn) { if (!modes.mouseOn) selecting = false }
    val currentScreenText = rememberUpdatedState(screenText)

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
            // 点空处：选择模式下是「退出」，平时是「叫键盘」
            onTerminalTap = { if (selecting) selecting = false else input?.showKeyboard() },
            onHyperlinkClick = onHyperlinkClick,
            onPasteRequest = onPasteRequest,
        )
        if (modes.mouseOn && !selecting) {
            MouseOverlay(
                emulator = emulator,
                modes = modes,
                fontSize = fontSize,
                send = attachment::sendRaw,
                onTap = { col, row ->
                    // 先看点的是不是链接（问 daemon 那份屏幕文本，几十毫秒），是就打开；
                    // 不是才发鼠标点击并叫键盘
                    scope.launch {
                        val url = runCatching { currentScreenText.value() }.getOrNull()?.let { urlAtCell(it, row, col) }
                        if (url != null) onHyperlinkClick(url)
                        else {
                            val sgr = modes.sgrMouse
                            attachment.sendRaw(Mouse.report(Mouse.LEFT, col, row, press = true, sgr = sgr))
                            attachment.sendRaw(Mouse.report(Mouse.LEFT, col, row, press = false, sgr = sgr))
                            input?.showKeyboard()
                        }
                    }
                },
                onLongPress = { selecting = true },
                onFontSize = onFontSize,
            )
        }
        if (selecting) {
            Text(
                "选择模式 · 长按选字，点链接打开，点空处退出",
                color = Tok.OnAccent, fontSize = 11.sp, textAlign = TextAlign.Center,
                modifier = Modifier.align(Alignment.TopCenter).fillMaxWidth().background(Tok.Accent.copy(alpha = 0.9f)).padding(vertical = 3.dp),
            )
        }
        AndroidView(
            factory = { c -> TermInputView(c) { sinkState.value }.also { input = it } },
            modifier = Modifier.size(1.dp).align(Alignment.TopStart),
        )
    }
}

/**
 * 滚轮事件节流：手指每过一格记一行，这里按 40ms 一批、每批最多 3 行发出去。
 * 一次一帧地发，Claude Code 每个滚轮事件都整屏重画，手机这头解析 + 渲染跟不上就卡。
 */
private class WheelPump(private val scope: CoroutineScope, private val send: (ByteArray) -> Unit) {
    private var pending = 0
    private var job: Job? = null
    var col = 1
    var row = 1
    var sgr = false

    fun add(lines: Int) {
        pending += lines
        if (job?.isActive != true) job = scope.launch {
            while (isActive && pending != 0) {
                val k = pending.coerceIn(-MAX_PER_TICK, MAX_PER_TICK)
                pending -= k
                val button = if (k > 0) Mouse.WHEEL_UP else Mouse.WHEEL_DOWN
                var batch = ByteArray(0)
                repeat(abs(k)) { batch += Mouse.report(button, col, row, press = true, sgr = sgr) }
                send(batch)
                delay(TICK_MS)
            }
        }
    }

    fun cancel() { pending = 0; job?.cancel() }

    private companion object {
        const val TICK_MS = 40L
        const val MAX_PER_TICK = 3
    }
}

/**
 * 鼠标上报模式下接管全部触摸（盖在 Terminal 上面的兄弟节点，Compose 命中测试只给最上面那个）：
 * - 单指滑动：每过一个格高记一行滚轮（手指向下 = 内容下来 = 滚轮向上），经 [WheelPump] 节流发出，松手带惯性。
 * - 点按：回调格子坐标，由宿主决定是开链接还是发点击 + 叫键盘。
 * - 长按不动：进入选择模式。
 * - 双指：捏合比例落定后改全局字号。
 * 格子尺寸用 emulator 的行列数除画布像素估算，termlib 没把字宽字高暴露出来。
 */
@Composable
private fun MouseOverlay(
    emulator: TerminalEmulator,
    modes: TermModes,
    fontSize: Int,
    send: (ByteArray) -> Unit,
    onTap: (col: Int, row: Int) -> Unit,
    onLongPress: () -> Unit,
    onFontSize: (Int) -> Unit,
) {
    val cur = rememberUpdatedState(modes)
    val fs = rememberUpdatedState(fontSize)
    val scope = rememberCoroutineScope()
    val haptic = LocalHapticFeedback.current
    val pump = remember(emulator) { WheelPump(scope, send) }
    DisposableEffect(pump) { onDispose { pump.cancel() } }
    Box(
        Modifier.fillMaxSize().pointerInput(emulator) {
            awaitEachGesture {
                val down = awaitFirstDown()
                pump.cancel()
                val dims = emulator.dimensions
                val cols = dims.columns.coerceAtLeast(1)
                val rows = dims.rows.coerceAtLeast(1)
                val cellW = size.width.toFloat() / cols
                val cellH = size.height.toFloat() / rows
                val slop = viewConfiguration.touchSlop
                val longPressMs = viewConfiguration.longPressTimeoutMillis
                fun cell(p: Offset): Pair<Int, Int> =
                    ((p.x / cellW).toInt() + 1).coerceIn(1, cols) to ((p.y / cellH).toInt() + 1).coerceIn(1, rows)
                pump.sgr = cur.value.sgrMouse
                cell(down.position).let { (c, r) -> pump.col = c; pump.row = r }
                var acc = 0f
                var moved = false
                var zoomed = false
                var longPressed = false
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
                    if (zoomed || longPressed) { ev.changes.forEach { it.consume() }; continue }
                    val ch = ev.changes.firstOrNull { it.id == down.id } ?: pressed.first()
                    velocity.addPosition(ch.uptimeMillis, ch.position)
                    val delta = ch.position - last
                    last = ch.position
                    total += delta
                    if (!moved && total.getDistance() > slop) moved = true
                    if (!moved && ch.uptimeMillis - down.uptimeMillis >= longPressMs) {
                        longPressed = true
                        haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                        onLongPress()
                    }
                    if (moved) {
                        acc += delta.y
                        cell(ch.position).let { (c, r) -> pump.col = c; pump.row = r }
                        var lines = 0
                        while (acc >= cellH) { acc -= cellH; lines++ }
                        while (acc <= -cellH) { acc += cellH; lines-- }
                        if (lines != 0) pump.add(lines)
                    }
                    ch.consume()
                }
                when {
                    zoomed -> {
                        val target = (fs.value * zoom).roundToInt().coerceIn(10, 22)
                        if (target != fs.value) onFontSize(target)
                    }
                    longPressed -> {}
                    moved -> {
                        // 惯性：按松手速度再补几行，最多 40 行，同样经 pump 节流
                        val vy = velocity.calculateVelocity().y
                        val lines = (abs(vy) / cellH * 0.25f).toInt().coerceAtMost(40)
                        if (lines > 0) pump.add(if (vy > 0) lines else -lines)
                    }
                    else -> {
                        val (c, r) = cell(down.position)
                        onTap(c, r)
                    }
                }
            }
        },
    )
}

/**
 * 屏幕文本第 [row] 行（1-based）里覆盖第 [col] 列（1-based）的链接。列按显示宽度算：
 * 东亚全角占两格。URL 本身是 ASCII，宽字符只影响它前面的偏移。
 */
internal fun urlAtCell(screen: String, row: Int, col: Int): String? {
    val line = screen.split('\n').getOrNull(row - 1) ?: return null
    // 每个字符的起始列（1-based）
    val starts = IntArray(line.length + 1)
    var c = 1
    for (i in line.indices) { starts[i] = c; c += displayWidth(line[i]) }
    starts[line.length] = c
    return findUrls(line).firstOrNull { span -> col >= starts[span.start] && col < starts[span.end] }?.url
}

internal fun displayWidth(ch: Char): Int {
    val code = ch.code
    return if (
        code in 0x1100..0x115F || code in 0x2E80..0xA4CF || code in 0xAC00..0xD7A3 ||
        code in 0xF900..0xFAFF || code in 0xFE30..0xFE4F || code in 0xFF00..0xFF60 || code in 0xFFE0..0xFFE6
    ) 2 else 1
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
