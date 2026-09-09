package cc.uoox.aaaui

import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.connectbot.terminal.TerminalEmulator
import org.connectbot.terminal.TerminalEmulatorFactory
import java.io.ByteArrayOutputStream

/**
 * 远端 TUI 声明的终端模式，从输出字节流里盯 DECSET/DECRST 得来——termlib 0.1.0 不把
 * libvterm 的 termprop 往外报，而这几项决定触摸手势的含义。daemon 在 attach 的 replay 里
 * 会用 state_formatted 把当前模式重发一遍，所以晚接入也能拿到。
 */
data class TermModes(
    /** 鼠标上报：0 关；1000 点击；1002 拖动；1003 任意移动。Claude Code 开的是 1003。 */
    val mouse: Int = 0,
    /** DECSET 1006：SGR 坐标编码（不限 223 列）。 */
    val sgrMouse: Boolean = false,
    /** DECSET 1049 / 47 / 1047：备用屏。备用屏没有回滚，滑动要变成滚轮事件发给 TUI。 */
) {
    val mouseOn: Boolean get() = mouse != 0
}

/** 增量扫描输出字节里的私有模式开关；跨帧被截断的 `ESC [ ?…` 前缀留到下一帧接着解析。 */
class ModeTracker {
    private val _modes = MutableStateFlow(TermModes())
    val modes: StateFlow<TermModes> = _modes.asStateFlow()
    private var carry = ByteArray(0)

    fun feed(bytes: ByteArray) {
        val buf = if (carry.isEmpty()) bytes else carry + bytes
        val (next, rest) = scan(_modes.value, buf)
        carry = rest
        if (next != _modes.value) _modes.value = next
    }

    /** 重连前清零：replay 会把模式重发一遍。 */
    fun reset() {
        carry = ByteArray(0)
        _modes.value = TermModes()
    }

    companion object {
        private const val ESC = 0x1b.toByte()
        private const val MAX_PARAMS = 64

        /** 纯函数：叠加 [buf] 里所有完整的 `CSI ? Pm h/l`，返回新模式与末尾未完结的序列前缀。 */
        fun scan(start: TermModes, buf: ByteArray): Pair<TermModes, ByteArray> {
            var m = start
            var i = 0
            while (i < buf.size) {
                if (buf[i] != ESC) { i++; continue }
                var j = i + 1
                if (j >= buf.size) return m to buf.copyOfRange(i, buf.size)
                if (buf[j] != '['.code.toByte()) { i++; continue }
                j++
                if (j >= buf.size) return m to buf.copyOfRange(i, buf.size)
                if (buf[j] != '?'.code.toByte()) { i++; continue }
                j++
                val params = StringBuilder()
                while (j < buf.size && params.length <= MAX_PARAMS) {
                    val c = buf[j].toInt().toChar()
                    if (c.isDigit() || c == ';') { params.append(c); j++ } else break
                }
                if (j >= buf.size) return m to buf.copyOfRange(i, buf.size) // 截断，等下一帧
                val final = buf[j].toInt().toChar()
                if (final == 'h' || final == 'l') {
                    val on = final == 'h'
                    for (p in params.split(';')) when (val n = p.toIntOrNull()) {
                        1000, 1002, 1003 -> m = m.copy(mouse = if (on) n else 0)
                        1006 -> m = m.copy(sgrMouse = on)
                    }
                }
                i = j + 1
            }
            return m to ByteArray(0)
        }
    }
}

/** xterm 鼠标上报编码。[col]/[row] 1-based；滚轮按键 64（上）/ 65（下）只有按下没有松开。 */
object Mouse {
    const val LEFT = 0
    const val WHEEL_UP = 64
    const val WHEEL_DOWN = 65

    fun report(button: Int, col: Int, row: Int, press: Boolean, sgr: Boolean): ByteArray =
        if (sgr) {
            "\u001b[<$button;$col;$row${if (press) 'M' else 'm'}".toByteArray()
        } else {
            // X10 编码：松开一律是按钮 3；坐标 +32 后必须是单字节
            val b = if (press) button else 3
            byteArrayOf(
                0x1b, '['.code.toByte(), 'M'.code.toByte(),
                (32 + b).toByte(), (32 + col.coerceIn(1, 223)).toByte(), (32 + row.coerceIn(1, 223)).toByte(),
            )
        }
}

/**
 * Owns the attach WebSocket for one session and glues it to the termlib emulator:
 * hello text frame → session metadata; binary frames → emulator（同时过一遍 [ModeTracker]）;
 * emulator 编好的键盘字节 → binary frames; Compose 侧算出的格子数 → `{"t":"resize"}` text frames。
 * Reconnects with exponential backoff — the daemon replays the full screen on re-attach.
 *
 * Lifetime is owned by [AppStore], not by the composable that shows it — see
 * `AppStore.attachmentFor`. 模拟器随之常驻，所以回滚历史跨 composable 重建保留。
 */
class TerminalAttachment(
    val api: DaemonClient,
    private val sessionId: String,
) {
    private val tracker = ModeTracker()
    val modes: StateFlow<TermModes> get() = tracker.modes

    /** OSC 52（程序往剪贴板写）到达时的去处，由持有 Context 的 UI 挂上。 */
    @Volatile var onClipboardCopy: ((String) -> Unit)? = null

    /**
     * libvterm 模拟器（termlib）。键盘输入经 onKeyboardInput 原样进 WS；尺寸由 Compose 侧的
     * Terminal() 按可用像素算出来后回调，再走 resize 控制帧。在主线程建（attachmentFor 在组合期调）。
     */
    val emulator: TerminalEmulator = TerminalEmulatorFactory.create(
        defaultForeground = Tok.TermFg,
        defaultBackground = Tok.TermBg,
        onKeyboardInput = ::sendBytes,
        onResize = { d -> sendResize(d.columns, d.rows) },
        onClipboardCopy = { text -> onClipboardCopy?.invoke(text) },
        autoDetectUrls = true,
    ).also { applyTerminalPalette(it) }

    /**
     * 连接状态与 hello 帧带回的会话元数据做成 StateFlow，而不是构造期传进来的
     * 回调：attach 现在跨 composable 存活（折叠/展开、单栏↔两栏都会重建
     * SessionScreen），构造期捕获的 lambda 会指着已经销毁的那一份 UI。
     */
    private val _connected = MutableStateFlow(false)
    val connected: StateFlow<Boolean> = _connected.asStateFlow()
    private val _hello = MutableStateFlow<Session?>(null)
    val hello: StateFlow<Session?> = _hello.asStateFlow()

    private val handler = Handler(Looper.getMainLooper())
    private var ws: WebSocket? = null
    @Volatile private var open = false
    @Volatile private var stopped = false
    @Volatile private var everConnected = false
    @Volatile private var awaitingReplay = false
    @Volatile private var reconnectScheduled = false
    private var attempt = 0
    private val pendingInput = ByteArrayOutputStream()

    fun start() = connect()

    fun stop() {
        stopped = true
        handler.removeCallbacksAndMessages(null)
        ws?.cancel()
        ws = null
        open = false
        _connected.value = false
    }

    /** 直接写字面文本进 PTY（键位条上的 - / | ~ 与「换行」，输入法提交的整段文本）。 */
    fun write(text: String) = sendBytes(text.toByteArray())

    /** 原样字节（鼠标上报序列）。 */
    fun sendRaw(bytes: ByteArray) = sendBytes(bytes)

    private fun feed(bytes: ByteArray) {
        tracker.feed(bytes)
        emulator.writeInput(bytes)
    }

    /** 把本地行列重新宣告给 daemon：重连后、以及回到前台时（谁在看谁说了算）。 */
    fun resendSize() {
        val d = emulator.dimensions
        if (d.columns > 0 && d.rows > 0) sendResize(d.columns, d.rows)
    }

    private fun sendResize(cols: Int, rows: Int) = sendControl("""{"t":"resize","cols":$cols,"rows":$rows}""")

    private fun connect() {
        if (stopped) return
        ws = api.attachSocket(sessionId, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                val isReconnect = everConnected
                everConnected = true
                open = true
                awaitingReplay = true
                reconnectScheduled = false
                attempt = 0
                _connected.value = true
                // hello 后 daemon 必发整屏 replay（含数百行历史 + 当前终端模式）：
                // 模式从零开始跟，重连还要先清掉本地回滚，否则每次断线重连都叠一份重复历史。
                // 3J = 清回滚（libvterm 认），2J+H = 清屏归位。
                tracker.reset()
                if (isReconnect) feed("\u001b[3J\u001b[2J\u001b[H".toByteArray())
                resendSize()
                val queued = synchronized(pendingInput) {
                    val b = pendingInput.toByteArray(); pendingInput.reset(); b
                }
                if (queued.isNotEmpty()) webSocket.send(queued.toByteString())
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                try {
                    val obj = ProtocolJson.instance.parseToJsonElement(text) as? JsonObject ?: return
                    if ((obj["t"] as? JsonPrimitive)?.content == "hello") {
                        val sess = obj["session"]?.let {
                            ProtocolJson.instance.decodeFromJsonElement(Session.serializer(), it)
                        } ?: return
                        _hello.value = sess
                    }
                } catch (_: Exception) { }
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                feed(bytes.toByteArray())
                if (awaitingReplay && webSocket === ws) {
                    awaitingReplay = false
                    resendSize()
                    val queued = synchronized(pendingInput) {
                        pendingInput.toByteArray().also { pendingInput.reset() }
                    }
                    if (queued.isNotEmpty() && !webSocket.send(queued.toByteString())) {
                        synchronized(pendingInput) { pendingInput.write(queued) }
                    }
                }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                if (webSocket !== ws) return
                open = false
                _connected.value = false
                scheduleReconnect()
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                if (webSocket !== ws) return
                open = false
                _connected.value = false
                scheduleReconnect()
            }
        })
    }

    private fun scheduleReconnect() {
        if (stopped || reconnectScheduled) return
        reconnectScheduled = true
        val delay = (1000L shl attempt.coerceAtMost(4)) // 1s..16s
        attempt++
        handler.postDelayed({
            reconnectScheduled = false
            connect()
        }, delay)
    }

    private fun sendBytes(bytes: ByteArray) {
        val socket = ws
        if (open && socket != null && socket.send(bytes.toByteString())) return
        synchronized(pendingInput) {
            if (pendingInput.size() + bytes.size <= 16 * 1024) pendingInput.write(bytes)
        }
    }

    private fun sendControl(text: String) {
        if (open) ws?.send(text)
    }
}
