package cc.uoox.aaaui

import android.annotation.SuppressLint
import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
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

/** 系统通知只有三种（2026-09-07 用户拍板）：待回复 / 运行结束 / 出错 */
sealed class NotifyEvent {
    data class Done(val session: Session, val exited: Boolean) : NotifyEvent()
    data class Asking(val session: Session) : NotifyEvent()
    data class Error(val session: Session, val error: String) : NotifyEvent()
}

/**
 * 一次会话更新该响哪一声，**最多一声**（PROTOCOL「通知策略」：待回复 / 运行结束 / 出错）。
 *
 * v1.22 改成阶梯，先中先出。以前是四个平铺的 `if`：一次更新里 `asking` 翻 true、状态又从
 * running 落到 waiting（同一拍里常有的事），会一口气弹两条说同一件事。mac 侧一直是 return
 * 式的阶梯，两端说的话对不上。
 *
 * [prev] 是这个会话上一次的 `state`，[killedHere] = 这台设备自己按的「结束」（自己动的手不用报告）。
 * 纯函数，好测；「正盯着看就不响」和「静音只关通知不关黄点」在调用方（[AppStore]）。
 */
fun notifyEventFor(s: Session, old: Session?, prev: String?, killedHere: Boolean): NotifyEvent? = when {
    s.agent == "shell" -> null // 终端没有「一轮跑完了」这回事
    s.asking && old?.asking != true && s.state != "exited" -> NotifyEvent.Asking(s)
    !s.error.isNullOrBlank() && old?.error != s.error -> NotifyEvent.Error(s, s.error)
    prev == "running" && s.state == "waiting" -> NotifyEvent.Done(s, exited = false)
    // 退出：只有非 0 退出码算「出错」；正常退出不弹；本机手动终止的不弹
    prev == "running" && s.state == "exited" && !killedHere && (s.exit_code ?: 0) != 0 ->
        NotifyEvent.Done(s, exited = true)
    else -> null
}

/** Process-wide repository: settings, connection loop, /events WS → StateFlows. */
class AppStore private constructor(context: Context) {
    companion object {
        @SuppressLint("StaticFieldLeak")
        @Volatile private var instance: AppStore? = null
        fun get(context: Context): AppStore =
            instance ?: synchronized(this) { instance ?: AppStore(context.applicationContext).also { instance = it } }

        /** 折叠屏重建 SessionScreen 只要几十毫秒，留够宽限就不会误伤 attach。 */
        private const val RELEASE_GRACE_MS = 5_000L
    }

    val settings = SettingsStore(context)
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    private val _connState = MutableStateFlow<ConnState>(ConnState.NoServer)
    val connState: StateFlow<ConnState> = _connState.asStateFlow()
    private val _sessions = MutableStateFlow<List<Session>>(emptyList())
    val sessions: StateFlow<List<Session>> = _sessions.asStateFlow()
    private val _projects = MutableStateFlow<List<Project>>(emptyList())
    val projects: StateFlow<List<Project>> = _projects.asStateFlow()
    private val _agents = MutableStateFlow<List<AgentInfo>>(emptyList())
    /** agent 表（GET /agents）。空 = 老 daemon 或只有一个可用：不画任何切换入口 */
    val agents: StateFlow<List<AgentInfo>> = _agents.asStateFlow()
    private val _health = MutableStateFlow<Health?>(null)
    val health: StateFlow<Health?> = _health.asStateFlow()
    /** 套餐用量（5h / 7d / 按模型）；null = 没数据，首页不显示 */
    private val _planUsage = MutableStateFlow<PlanUsage?>(null)
    val planUsage: StateFlow<PlanUsage?> = _planUsage.asStateFlow()

    /** Session-state transition notifications (running → waiting / exited). */
    private val _notifyEvents = MutableSharedFlow<NotifyEvent>(extraBufferCapacity = 32)
    val notifyEvents = _notifyEvents.asSharedFlow()
    /** 屏幕要的原始帧（messages_changed）。 */
    private val _frames = MutableSharedFlow<EventFrame>(extraBufferCapacity = 64)
    val frames = _frames.asSharedFlow()

    @Volatile var client: DaemonClient? = null
        private set

    private var loopJob: Job? = null
    private val reconnectKick = Channel<Unit>(Channel.CONFLATED)
    private val prevStates = HashMap<String, String>()
    private val shellDeletes = java.util.Collections.synchronizedSet(mutableSetOf<String>())
    private val attachments = HashMap<String, TerminalAttachment>()
    private val pendingRelease = HashMap<String, Job>()

    fun ensureStarted() {
        synchronized(this) {
            if (loopJob == null) loopJob = scope.launch { connectionLoop() }
        }
    }

    // ---------- 输入框草稿 ----------

    /**
     * 会话 id → 没发出去的输入。SessionScreen 离开 composition（返回首页、切去别的 app
     * 被系统回收）rememberSaveable 都保不住，所以放这里，并落盘到 DataStore：进程被杀
     * 再回来字也还在。内存里这份是权威，磁盘写入去抖，多敲几个字不多写几次。
     */
    private val drafts = HashMap<String, String>()
    private val draftsLoaded = kotlinx.coroutines.CompletableDeferred<Unit>()
    private var draftFlush: Job? = null

    init {
        scope.launch {
            val saved = runCatching { settings.drafts() }.getOrDefault(emptyMap())
            synchronized(drafts) { saved.forEach { (k, v) -> drafts.putIfAbsent(k, v) } }
            draftsLoaded.complete(Unit)
        }
    }

    /** 等磁盘那份读完再给（冷启动直接深链进会话时会用到）；已加载就立刻返回。 */
    suspend fun awaitDraft(sessionId: String): String {
        draftsLoaded.await()
        return synchronized(drafts) { drafts[sessionId].orEmpty() }
    }

    fun draft(sessionId: String): String = synchronized(drafts) { drafts[sessionId].orEmpty() }

    fun setDraft(sessionId: String, text: String) {
        synchronized(drafts) { if (text.isEmpty()) drafts.remove(sessionId) else drafts[sessionId] = text }
        draftFlush?.cancel()
        draftFlush = scope.launch {
            delay(400)
            val snapshot = synchronized(drafts) { drafts.toMap() }
            runCatching { settings.setDrafts(snapshot) }
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
            scope.launch { runCatching { api.health() }.onSuccess { h -> _health.value = h; settings.noteProjectRoot(h.project_root) } }
            scope.launch { refreshProjects() }
            scope.launch { refreshUsage() }
            scope.launch { runCatching { api.agents() }.onSuccess { _agents.value = it } }
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

    /** 用户在本机点了终止的会话 id：随后的 exited 事件不推本地通知 */
    private val userKilled = java.util.Collections.synchronizedSet(mutableSetOf<String>())

    fun markUserKilled(id: String) { userKilled.add(id) }

    /**
     * 黄点（2026-09-08）：这一轮跑完了、或者在等你回话，而这台设备还没进去看。
     * 打点的时机和三种通知完全一样——通知响一声、列表上留一个点，是同一件事的两种说法。
     * 清点在 [seenProject]（进会话就清）。
     */
    private fun markUnread(s: Session) {
        val path = s.project_path
        if (path.isBlank()) return
        scope.launch { settings.setProjectUnread(path, true) }
    }

    /**
     * 正盯着看的那个会话（会话屏在最上面、且 app 在前台）。眼皮底下跑完的东西再弹一条
     * 只是噪音——mac 侧一直有这条抑制（`watching`），Android 以前没有：手机开着会话屏，
     * 同一件事照样响一声、还留个黄点。
     */
    @Volatile private var watching: String? = null

    /** 会话屏进入前台时登记；[clearWatching] 离开时销掉（比对 id，防切会话时后销的把先登记的清了）。 */
    fun setWatching(id: String) { watching = id }

    fun clearWatching(id: String) { if (watching == id) watching = null }

    /** 进了这个项目的会话：黄点消失。 */
    fun seenProject(path: String?) {
        if (path.isNullOrBlank()) return
        scope.launch { settings.setProjectUnread(path, false) }
    }

    /**
     * 一次会话更新最多响**一声**（PROTOCOL「通知策略」：待回复 / 运行结束 / 出错，三种）。
     * v1.22 改成阶梯，先中先出：以前是四个平铺的 `if`，一次更新里 `asking` 翻 true 而状态
     * 又从 running 落到 waiting（这是同一拍里常有的事），会一口气弹两条说同一件事；mac 侧
     * 一直是 return 式的阶梯，两端说的话对不上。
     *
     * 「标记」（黄点）与「响一声」是同一件事的两种说法，所以判定共用这一处；区别只有：
     * 静音只关通知、不关黄点（静音是「别吵我」不是「别记着」，在 NotificationService 里滤），
     * 而**正盯着这个会话看**的时候两样都不做——已经看见了。
     */
    private fun notifyForUpdate(s: Session, old: Session?, prev: String?) {
        // 标记要无条件消耗掉（哪怕这次不响）
        val killedHere = userKilled.remove(s.id)
        val ev = notifyEventFor(s, old, prev, killedHere) ?: return
        if (watching == s.id) return
        _notifyEvents.tryEmit(ev)
        markUnread(s)
    }

    private fun handleFrame(frame: EventFrame) {
        when (frame) {
            is EventFrame.Snapshot -> {
                synchronized(prevStates) {
                    prevStates.clear()
                    frame.sessions.forEach { prevStates[it.id] = it.state }
                }
                _sessions.value = frame.sessions
                frame.sessions.filter { it.agent == "shell" && it.state == "exited" }.forEach(::cleanupExitedShell)
            }
            is EventFrame.SessionUpdate -> {
                val s = frame.session
                val prev = synchronized(prevStates) { val p = prevStates[s.id]; prevStates[s.id] = s.state; p }
                val old = _sessions.value.find { it.id == s.id }
                _sessions.value = _sessions.value.filter { it.id != s.id } + s
                if (s.agent == "shell" && s.state == "exited") cleanupExitedShell(s)
                notifyForUpdate(s, old, prev)
            }
            is EventFrame.SessionRemoved -> {
                synchronized(prevStates) { prevStates.remove(frame.id) }
                _sessions.value = _sessions.value.filter { it.id != frame.id }
                releaseAttachmentNow(frame.id) // 会话没了，attach 再重连也只会一直失败
            }
            is EventFrame.ProjectsChanged -> scope.launch { refreshProjects() }
            is EventFrame.HealthUpdate -> {
                _health.value = (_health.value ?: Health()).copy(ssd_mounted = frame.ssdMounted)
            }
            is EventFrame.MessagesChanged -> _frames.tryEmit(frame)
            // 详情屏的收件箱那一节自己订这条（屏幕没开就没人收，白广播一次也无所谓）
            is EventFrame.InboxChanged -> _frames.tryEmit(frame)
            is EventFrame.UsageUpdate -> _planUsage.value = frame.plan
            is EventFrame.Unknown -> { }
        }
    }

    /** 终端退出后没有回放价值，尽快从 daemon 与本地 attach 注册表清掉。 */
    private fun cleanupExitedShell(s: Session) {
        releaseAttachmentNow(s.id)
        if (shellDeletes.add(s.id)) scope.launch { runCatching { client?.deleteSession(s.id) } }
    }

    /**
     * 关一个终端：**先从列表里拿掉**，kill + DELETE 在后台跑（2026-09-08）。
     * daemon 的 `DELETE /sessions/:id` 对还没退出的会话最多要等 3 秒（SIGTERM → 2s → SIGKILL），
     * 以前 UI 串着 await 这两个请求，手指点下去到行消失中间就是这三秒的空白——「关终端很卡」
     * 的大头。会话没了 daemon 会广播 session_removed，列表本来就会收敛到同一个结果。
     */
    fun closeTerminal(sessionId: String) {
        _sessions.value = _sessions.value.filter { it.id != sessionId }
        releaseAttachmentNow(sessionId)
        shellDeletes.add(sessionId)
        scope.launch {
            runCatching { client?.kill(sessionId) }
            runCatching { client?.deleteSession(sessionId) }
            refreshSessions()
        }
    }

    /**
     * 提前把 attach 拉起来：点终端那一行的瞬间就开 WS，等屏幕组合完 replay 往往已经到了。
     * 复用 [attachmentFor] 的注册表，所以随后屏幕里再要一次拿到的是同一个（不会开两条）。
     */
    fun prewarmAttachment(sessionId: String) {
        attachmentFor(sessionId)
    }

    // ---------- 终端 attach 注册表 ----------

    /**
     * 拿到某个会话的 attach，没有就建一个。attach 由 store 持有而不是由
     * SessionScreen 持有：折叠/展开会重建 SessionScreen（单栏挂在 nav 的
     * session/{id}，宽屏时两栏并排），attach 若跟着 composable 生死，
     * 每折一次屏就断线重连一次——PTY 在 daemon 上不会丢，但整屏 replay 肉眼可见。
     *
     */
    /** 已经建好的 attach（有就直接给，没有返回 null，不新建）：首帧就能画上，不闪一下「未连接」。 */
    fun peekAttachment(sessionId: String): TerminalAttachment? = synchronized(attachments) { attachments[sessionId] }

    fun attachmentFor(sessionId: String): TerminalAttachment? {
        val api = client ?: return null
        return synchronized(attachments) {
            pendingRelease.remove(sessionId)?.cancel()
            val cur = attachments[sessionId]
            // daemon 重连后换了 DaemonClient，旧 socket 指向的 base 可能已经不对了
            if (cur != null && cur.api === api) {
                cur
            } else {
                cur?.stop()
                TerminalAttachment(api, sessionId).also { attachments[sessionId] = it; it.start() }
            }
        }
    }

    /**
     * 预约关闭：SessionScreen 被销毁时叫，但宽限 [RELEASE_GRACE_MS] 再真的收——
     * 折叠屏配置变化里重建只隔几十毫秒，宽限期内重新出现就当无事发生；真的离开
     * 会话才会把 socket 收掉，不留着无限重连。
     */
    fun releaseAttachmentSoon(sessionId: String) {
        synchronized(attachments) {
            if (!attachments.containsKey(sessionId)) return
            pendingRelease.remove(sessionId)?.cancel()
            pendingRelease[sessionId] = scope.launch {
                delay(RELEASE_GRACE_MS)
                releaseAttachmentNow(sessionId)
            }
        }
    }

    /** 会话被删/被结束时立刻收，否则 attach 会对着不存在的会话一直重连。 */
    fun releaseAttachmentNow(sessionId: String) {
        synchronized(attachments) {
            pendingRelease.remove(sessionId)?.cancel()
            attachments.remove(sessionId)?.stop()
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

    /** 404（旧 daemon）或失败都不动现值；成功才覆盖，包括覆盖成 null */
    suspend fun refreshUsage() {
        val api = client ?: return
        runCatching { api.usage() }.onSuccess { _planUsage.value = it }
    }

    suspend fun refreshHealth() {
        val api = client ?: return
        runCatching { api.health() }.onSuccess { _health.value = it; settings.noteProjectRoot(it.project_root) }
    }
}

fun terminalSessions(sessions: List<Session>): List<Session> =
    sessions.filter { it.agent == "shell" && it.state != "exited" }.sortedBy { it.created_at }
