package cc.uoox.aaaui

import android.os.Handler
import android.os.Looper
import com.termux.terminal.TerminalSessionClient
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

    /** 换宿主 UI 时把回调对象接过去（emulator 也要跟着换，见 vendored 实现）。 */
    fun rebind(client: TerminalSessionClient) = session.updateTerminalSessionClient(client)

    private fun connect() {
        if (stopped) return
        ws = api.attachSocket(sessionId, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                open = true
                attempt = 0
                _connected.value = true
                session.resendSize()
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
                session.pushBytes(bytes.toByteArray())
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                open = false
                _connected.value = false
                scheduleReconnect()
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
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
