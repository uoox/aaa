package cc.uoox.aaaui

import android.annotation.SuppressLint
import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.selects.select
import kotlinx.coroutines.withTimeout
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

sealed class ConnState {
    data object NoServer : ConnState()
    data class Connecting(val host: String?) : ConnState()
    data class Connected(val host: String, val latencyMs: Long) : ConnState()
    data class Failed(val message: String) : ConnState()
}

sealed class NotifyEvent {
    data class Waiting(val session: Session) : NotifyEvent()
    data class Exited(val session: Session) : NotifyEvent()
    data class Stalled(val session: Session?, val id: String, val quietS: Long) : NotifyEvent()
}

/** Process-wide repository: settings, connection loop, /events WS → StateFlows. */
class AppStore private constructor(context: Context) {
    companion object {
        @SuppressLint("StaticFieldLeak")
        @Volatile private var instance: AppStore? = null
        fun get(context: Context): AppStore =
            instance ?: synchronized(this) { instance ?: AppStore(context.applicationContext).also { instance = it } }
    }

    val settings = SettingsStore(context)
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val _connState = MutableStateFlow<ConnState>(ConnState.NoServer)
    val connState: StateFlow<ConnState> = _connState.asStateFlow()
    private val _sessions = MutableStateFlow<List<Session>>(emptyList())
    val sessions: StateFlow<List<Session>> = _sessions.asStateFlow()
    private val _projects = MutableStateFlow<List<Project>>(emptyList())
    val projects: StateFlow<List<Project>> = _projects.asStateFlow()
    private val _health = MutableStateFlow<Health?>(null)
    val health: StateFlow<Health?> = _health.asStateFlow()

    /** Session-state transition notifications (waiting / exited / stalled). */
    private val _notifyEvents = MutableSharedFlow<NotifyEvent>(extraBufferCapacity = 32)
    val notifyEvents = _notifyEvents.asSharedFlow()
    /** Raw v1.1 frames screens care about (messages_changed / inbox_changed). */
    private val _frames = MutableSharedFlow<EventFrame>(extraBufferCapacity = 64)
    val frames = _frames.asSharedFlow()

    @Volatile var client: DaemonClient? = null
        private set

    private var loopJob: Job? = null
    private val reconnectKick = Channel<Unit>(Channel.CONFLATED)
    private val prevStates = HashMap<String, String>()

    fun ensureStarted() {
        synchronized(this) {
            if (loopJob == null) loopJob = scope.launch { connectionLoop() }
        }
    }

    /** Save server config (from pairing / manual entry) and reconnect immediately. */
    suspend fun applyServer(server: ServerConfig) {
        settings.setServer(server)
        client = null
        _connState.value = ConnState.Connecting(server.preferredHost)
        ensureStarted()
        reconnectKick.trySend(Unit)
    }

    suspend fun clearServer() {
        settings.setServer(null)
        client = null
        _connState.value = ConnState.NoServer
        reconnectKick.trySend(Unit)
    }

    fun kickReconnect() { reconnectKick.trySend(Unit) }

    // ---------- connection loop ----------

    private suspend fun connectionLoop() {
        var backoffMs = 1000L
        while (true) {
            val server = settings.current().server
            if (server == null) {
                _connState.value = ConnState.NoServer
                reconnectKick.receive()
                continue
            }
            _connState.value = ConnState.Connecting(server.preferredHost)
            val probed = probeHosts(server)
            if (probed == null) {
                _connState.value = ConnState.Failed("所有 host 均无法连接")
                waitOrKick(backoffMs)
                backoffMs = (backoffMs * 2).coerceAtMost(30_000)
                continue
            }
            val (api, host, latency) = probed
            client = api
            _connState.value = ConnState.Connected(host, latency)
            settings.setPreferredHost(host)
            backoffMs = 1000L
            scope.launch { runCatching { _health.value = api.health() } }
            scope.launch { refreshProjects() }
            runEventsUntilClosed(api) // suspends while WS is healthy
            if (settings.current().server == null) continue
            _connState.value = ConnState.Connecting(host)
            waitOrKick(backoffMs)
            backoffMs = (backoffMs * 2).coerceAtMost(30_000)
        }
    }

    private suspend fun waitOrKick(ms: Long) {
        kotlinx.coroutines.withTimeoutOrNull(ms) { reconnectKick.receive() }
    }

    /** Try preferred host first, then the rest; remember whichever answers /health. */
    private suspend fun probeHosts(server: ServerConfig): Triple<DaemonClient, String, Long>? {
        val ordered = buildList {
            server.preferredHost?.let { add(it) }
            server.hosts.forEach { if (it != server.preferredHost) add(it) }
        }
        for (host in ordered) {
            val api = DaemonClient(DaemonClient.normalizeBase(host), server.token)
            val t0 = System.currentTimeMillis()
            val ok = runCatching { withTimeout(4000) { api.health() } }.isSuccess
            if (ok) return Triple(api, host, System.currentTimeMillis() - t0)
        }
        return null
    }

    private suspend fun runEventsUntilClosed(api: DaemonClient) {
        val closed = Channel<Unit>(Channel.CONFLATED)
        val ws = api.eventsSocket(object : WebSocketListener() {
            override fun onMessage(webSocket: WebSocket, text: String) { handleFrame(EventFrame.parse(text)) }
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) { closed.trySend(Unit) }
            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) { closed.trySend(Unit) }
        })
        select<Unit> {
            closed.onReceive { }
            reconnectKick.onReceive { }
        }
        ws.cancel()
    }

    private fun handleFrame(frame: EventFrame) {
        when (frame) {
            is EventFrame.Snapshot -> {
                synchronized(prevStates) {
                    prevStates.clear()
                    frame.sessions.forEach { prevStates[it.id] = it.state }
                }
                _sessions.value = frame.sessions
            }
            is EventFrame.SessionUpdate -> {
                val s = frame.session
                val prev = synchronized(prevStates) { val p = prevStates[s.id]; prevStates[s.id] = s.state; p }
                _sessions.value = _sessions.value.filter { it.id != s.id } + s
                if (s.state == "waiting" && prev != "waiting") _notifyEvents.tryEmit(NotifyEvent.Waiting(s))
                if (s.state == "exited" && prev == "running") _notifyEvents.tryEmit(NotifyEvent.Exited(s))
            }
            is EventFrame.SessionRemoved -> {
                synchronized(prevStates) { prevStates.remove(frame.id) }
                _sessions.value = _sessions.value.filter { it.id != frame.id }
            }
            is EventFrame.ProjectsChanged -> scope.launch { refreshProjects() }
            is EventFrame.HealthUpdate -> {
                _health.value = (_health.value ?: Health()).copy(ssd_mounted = frame.ssdMounted)
            }
            is EventFrame.SessionStalled -> {
                val session = _sessions.value.find { it.id == frame.id }
                _notifyEvents.tryEmit(NotifyEvent.Stalled(session, frame.id, frame.quietS))
                _frames.tryEmit(frame)
            }
            is EventFrame.MessagesChanged, is EventFrame.InboxChanged -> _frames.tryEmit(frame)
            is EventFrame.Unknown -> { }
        }
    }

    // ---------- manual refresh ----------

    suspend fun refreshSessions() {
        val api = client ?: return
        runCatching { api.sessions() }.onSuccess { list ->
            synchronized(prevStates) { prevStates.clear(); list.forEach { prevStates[it.id] = it.state } }
            _sessions.value = list
        }
    }

    suspend fun refreshProjects() {
        val api = client ?: return
        runCatching { api.projects() }.onSuccess { _projects.value = it }
    }

    suspend fun refreshHealth() {
        val api = client ?: return
        runCatching { api.health() }.onSuccess { _health.value = it }
    }
}
