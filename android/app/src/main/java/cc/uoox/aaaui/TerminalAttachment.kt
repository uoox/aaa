package cc.uoox.aaaui

import android.os.Handler
import android.os.Looper
import com.termux.terminal.TerminalSessionClient
import org.connectbot.terminal.TerminalEmulator
import org.connectbot.terminal.TerminalEmulatorFactory
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
import java.io.ByteArrayOutputStream

/** 手机端终端用哪套模拟器画。设置里可切，也可以在会话页临时切。 */
enum class TerminalEngine(val key: String, val label: String) {
    /** vendored termux：Java 解析 + 自绘 View，久经考验但和 Compose 隔着一层。 */
    Termux("termux", "Termux"),
    /** ConnectBot termlib：libvterm 走 JNI 解析，Compose Canvas 渲染，选区/缩放/链接都是原生 Compose。 */
    Termlib("termlib", "Termlib");

    companion object {
        fun forName(name: String?): TerminalEngine = entries.firstOrNull { it.key == name } ?: Termux
    }
}

/**
 * Owns the attach WebSocket for one session and glues it to a [RemoteTerminalSession]:
 * hello text frame → session metadata; binary frames → emulator; user input bytes →
 * binary frames; resize → `{"t":"resize"}` text frames. Reconnects with exponential
 * backoff — the daemon replays the full screen on re-attach.
 *
 * Lifetime is owned by [AppStore], not by the composable that shows it — see
 * `AppStore.attachmentFor`.
 */
class TerminalAttachment(
    val api: DaemonClient,
    private val sessionId: String,
    client: TerminalSessionClient,
) {
    val session = RemoteTerminalSession(::sendBytes, ::sendControl, client)

    /**
     * 当前把远端字节喂给哪套模拟器。两套都建着太费（每个字节解析两遍），所以只喂一套；
     * 切换时断开重连，daemon 在 hello 后整屏 replay，新的那套就有完整画面。
     */
    @Volatile var engine: TerminalEngine = TerminalEngine.Termux
        private set

    /** OSC 52（程序往剪贴板写）到达时的去处，由持有 Context 的 UI 挂上。 */
    @Volatile var onClipboardCopy: ((String) -> Unit)? = null

    /**
     * 第二套模拟器（termlib / libvterm）。按需建，和 attach 同生共死，所以滚回历史
     * 跨 composable 重建保留。键盘输入经 [onKeyboardInput] 原样进 WS；尺寸由 Compose
     * 侧的 Terminal() 按可用像素算出来后回调，再走同一条 resize 控制帧。
     */
    val termlib: TerminalEmulator by lazy {
        TerminalEmulatorFactory.create(
            defaultForeground = Tok.TermFg,
            defaultBackground = Tok.TermBg,
            onKeyboardInput = ::sendBytes,
            onResize = { d -> sendResize(d.columns, d.rows) },
            onClipboardCopy = { text -> onClipboardCopy?.invoke(text) },
            autoDetectUrls = true,
        ).also { applyTermlibPalette(Tok.current, it) }
    }

    /** 切换模拟器：清掉目标那套的旧画面，断开重连拿 replay。同一套则什么都不做。 */
    fun switchEngine(target: TerminalEngine) {
        if (engine == target) return
        engine = target
        // 还没连上（首连在路上 / 重连已排队）就不用动：feed() 是按当下的 engine 分发的，
        // 即将到来的 replay 自然进新的那套。已经连着才需要断开换一份 replay。
        if (open && !stopped) reconnectNow()
    }

    /** 直接写字面文本进 PTY（键位条上的 - / | ~ 与「换行」）。 */
    fun write(text: String) = sendBytes(text.toByteArray())

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
    private var attempt = 0
    private val pendingInput = ByteArrayOutputStream()
    /** 每次 connect 加一；旧 socket 被主动 cancel 后的 onFailure 不许再排重连，否则会连出两条。 */
    @Volatile private var generation = 0

    fun start() = connect()

    fun stop() {
        stopped = true
        handler.removeCallbacksAndMessages(null)
        ws?.cancel()
        ws = null
        open = false
        _connected.value = false
    }

    /** 换宿主 UI 时把回调对象接过去（emulator 也要跟着换，见 vendored 实现）。 */
    fun rebind(client: TerminalSessionClient) = session.updateTerminalSessionClient(client)

    /** 主动断开并立刻重连（切模拟器用）。旧 socket 的收尾回调按代数丢弃。 */
    private fun reconnectNow() {
        handler.removeCallbacksAndMessages(null)
        ws?.cancel()
        ws = null
        open = false
        _connected.value = false
        attempt = 0
        connect()
    }

    private fun feed(bytes: ByteArray) {
        when (engine) {
            TerminalEngine.Termux -> session.pushBytes(bytes)
            TerminalEngine.Termlib -> termlib.writeInput(bytes)
        }
    }

    private fun resendSize() {
        when (engine) {
            TerminalEngine.Termux -> session.resendSize()
            TerminalEngine.Termlib -> {
                val d = termlib.dimensions
                if (d.columns > 0 && d.rows > 0) sendResize(d.columns, d.rows)
            }
        }
    }

    private fun sendResize(cols: Int, rows: Int) = sendControl("""{"t":"resize","cols":$cols,"rows":$rows}""")

    private fun connect() {
        if (stopped) return
        val gen = ++generation
        ws = api.attachSocket(sessionId, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                if (gen != generation) return
                val isReconnect = everConnected
                everConnected = true
                open = true
                attempt = 0
                _connected.value = true
                if (isReconnect) {
                    // hello 后 daemon 必发整屏 replay（含数百行历史）：重连前本地
                    // 清掉回滚缓冲，否则每次断线重连都叠一份重复历史（审查 P1）。
                    // 3J = 清 transcript（termux 与 libvterm 都认），2J+H = 清屏归位。
                    feed("\u001b[3J\u001b[2J\u001b[H".toByteArray())
                }
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
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                if (gen != generation) return
                open = false
                _connected.value = false
                scheduleReconnect()
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                if (gen != generation) return
                open = false
                _connected.value = false
                scheduleReconnect()
            }
        })
    }

    private fun scheduleReconnect() {
        if (stopped) return
        val delay = (1000L shl attempt.coerceAtMost(4)) // 1s..16s
        attempt++
        handler.postDelayed({ connect() }, delay)
    }

    private fun sendBytes(bytes: ByteArray) {
        val socket = ws
        if (open && socket != null) {
            socket.send(bytes.toByteString())
        } else {
            synchronized(pendingInput) {
                if (pendingInput.size() < 16 * 1024) pendingInput.write(bytes)
            }
        }
    }

    private fun sendControl(text: String) {
        if (open) ws?.send(text)
    }
}
