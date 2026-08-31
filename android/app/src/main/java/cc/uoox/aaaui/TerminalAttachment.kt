package cc.uoox.aaaui

import android.os.Handler
import android.os.Looper
import com.termux.terminal.TerminalSessionClient
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
 */
class TerminalAttachment(
    val api: DaemonClient,
    private val sessionId: String,
    client: TerminalSessionClient,
    private val onHello: (Session) -> Unit = {},
    private val onConnectionChange: (Boolean) -> Unit = {},
) {
    val session = RemoteTerminalSession(::sendBytes, ::sendControl, client)

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
    }

    private fun connect() {
        if (stopped) return
        ws = api.attachSocket(sessionId, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                open = true
                attempt = 0
                onConnectionChange(true)
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
                        handler.post { onHello(sess) }
                    }
                } catch (_: Exception) { }
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                session.pushBytes(bytes.toByteArray())
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                open = false
                onConnectionChange(false)
                scheduleReconnect()
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                open = false
                onConnectionChange(false)
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
