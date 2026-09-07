package cc.uoox.aaaui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.inputmethod.InputMethodManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.rememberDrawerState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.zIndex
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.navigation.NavHostController
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

// ============================================================
// 会话屏：消息流 ⇄ 终端 双视图 + 共用 composer
// ============================================================

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(
    store: AppStore,
    nav: NavHostController,
    sessionId: String,
    prefill: String,
    onClose: () -> Unit = { nav.popBackStack() },
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val sessions by store.sessions.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val session = sessions.find { it.id == sessionId }

    // 消息流支持探测：null=未知，true/false=已知
    var messagesSupported by remember { mutableStateOf<Boolean?>(null) }
    var uiMode by rememberSaveable { mutableStateOf("") } // "" = 未定，跟随默认设置
    val effectiveMode = when {
        uiMode.isNotEmpty() -> uiMode
        messagesSupported == false -> "terminal"
        else -> settings.defaultUi
    }
    val showMessages = effectiveMode == "messages" && messagesSupported != false

    // 输入框：内容跟着会话存在 AppStore（并落盘），返回首页 / 切去别的 app 再回来字还在；
    // 通知带来的 prefill 优先，它本身也成为新草稿
    var composer by remember(sessionId) { mutableStateOf(prefill.ifEmpty { store.draft(sessionId) }) }
    LaunchedEffect(sessionId) {
        val saved = store.awaitDraft(sessionId)
        if (composer.isEmpty() && saved.isNotEmpty()) composer = saved
    }
    LaunchedEffect(sessionId) { snapshotFlow { composer }.collect { store.setDraft(sessionId, it) } }
    val ctrlStickyState = remember { mutableStateOf(false) }
    var ctrlSticky by ctrlStickyState
    // 终端视图的键位条 + 输入框：默认收起，右下角 ⌨ 放出来；不持久化，每次进来都是收起的
    var keysOpen by rememberSaveable { mutableStateOf(false) }

    // 终端 attach：实例归 AppStore 管（折叠/展开会重建本 composable），跟着 daemon 重连重新取一次。
    val conn by store.connState.collectAsState()
    val inputRef = remember { mutableStateOf<TermInputView?>(null) }
    val selectMode = remember { mutableStateOf(false) }
    var attachment by remember(sessionId) { mutableStateOf<TerminalAttachment?>(null) }
    LaunchedEffect(conn, sessionId) { attachment = store.attachmentFor(sessionId) }
    DisposableEffect(sessionId) {
        // 只是预约关闭：折叠屏重建在宽限期内会把它取消掉，见 AppStore
        onDispose { store.releaseAttachmentSoon(sessionId) }
    }
    // attach 还没建好时的占位流（remember 不能写在 elvis 右边——那是条件式 remember）
    val noAttachConnected = remember { MutableStateFlow(false) }
    val noAttachHello = remember { MutableStateFlow<Session?>(null) }
    val wsConnected by (attachment?.connected ?: noAttachConnected).collectAsState()
    val helloSession by (attachment?.hello ?: noAttachHello).collectAsState()
    val s = session ?: helloSession

    LaunchedEffect(sessionId) { if (session == null) store.refreshSessions() }
    // 黄点：进来就清掉，人在这一屏时又跑完一轮也当场清（2026-09-08）——
    // 黄点说的是「这台设备还没看」，正看着就不算没看
    LaunchedEffect(s?.project_path, s?.updated_at, s?.asking) { store.seenProject(s?.project_path) }

    // 消息流状态
    val messages = remember(sessionId) { mutableStateOf<List<ChatMessage>>(emptyList()) }
    var lastSeq by remember(sessionId) { mutableStateOf(0L) }
    suspend fun fetchMessages() {
        val api = store.client ?: return
        try {
            var pages = 0
            while (pages < 40) {
                val resp = api.messages(sessionId, after = lastSeq)
                messagesSupported = resp.supported
                if (!resp.supported) return
                if (resp.messages.isNotEmpty()) {
                    messages.value = (messages.value + resp.messages).distinctBy { it.seq }.sortedBy { it.seq }.takeLast(1500)
                    lastSeq = resp.messages.maxOf { it.seq }
                }
                if (resp.messages.isEmpty() || lastSeq >= resp.last_seq) break
                pages++
            }
        } catch (e: DaemonHttpException) {
            if (e.code == 404) messagesSupported = false // v1 daemon：无此端点，回落终端
        } catch (_: Exception) { }
    }
    LaunchedEffect(sessionId) { fetchMessages() }
    LaunchedEffect(sessionId) {
        store.frames.collectLatest { f ->
            if (f is EventFrame.MessagesChanged && f.id == sessionId) fetchMessages()
        }
    }

    // 待发送：项目收件箱里的条目，就画在消息流末尾。agent 还在跑时点「发送」进这里，
    // daemon 在它这轮跑完（或此刻已空着）时自动写进去——和终端里先敲好等它一样
    val projectPath = s?.project_path
    val pending = remember(sessionId) { mutableStateOf<List<InboxItem>>(emptyList()) }
    suspend fun fetchPending() {
        val api = store.client ?: return
        val path = projectPath ?: return
        try { pending.value = api.inbox(path) } catch (_: Exception) { }
    }
    LaunchedEffect(projectPath) { fetchPending() }
    LaunchedEffect(projectPath) {
        store.frames.collectLatest { f ->
            if (f is EventFrame.InboxChanged && f.path == projectPath) fetchPending()
        }
    }

    fun sendInput(text: String, enter: Boolean) {
        scope.launch {
            try { store.client?.input(sessionId, text, enter) }
            catch (e: Exception) { Toast.makeText(context, "发送失败：${e.message}", Toast.LENGTH_SHORT).show() }
        }
    }
    /** 输入框「发送」：agent 空着就直接写进去；还在跑（或弹着问题）就排成待发送。 */
    fun submitComposer() {
        val text = composer
        if (text.isBlank()) return
        composer = ""
        val path = projectPath
        if (queueInsteadOfSend(s?.state, s?.asking == true) && path != null) {
            scope.launch {
                try { store.client?.inboxAdd(path, text); fetchPending() }
                catch (e: Exception) {
                    composer = text // 没排上：字还给输入框
                    Toast.makeText(context, "排队失败：${e.message}", Toast.LENGTH_SHORT).show()
                }
            }
        } else {
            sendInput(text, enter = true)
        }
    }
    fun deletePending(item: InboxItem) {
        pending.value = pending.value.filter { it.id != item.id }
        scope.launch {
            runCatching { store.client?.inboxDelete(item.id) }
                .onFailure { Toast.makeText(context, "撤回失败：${it.message}", Toast.LENGTH_SHORT).show() }
            fetchPending()
        }
    }
    /**
     * 粘贴：剪贴板文本走 daemon 的 /input——它在 TUI 开了 DECSET 2004 时会补 bracketed-paste
     * 包裹，Claude Code 靠它区分「粘进来的多行」和「一行行敲的回车」。
     */
    fun pasteViaDaemon() {
        val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
        if (!text.isNullOrEmpty()) sendInput(text, enter = false)
    }

    // 附件上传（composer 📎）
    val filePicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        val proj = s?.project_path
        if (uri != null && proj != null) {
            scope.launch {
                try {
                    val (name, bytes) = readUri(context, uri)
                    val saved = store.client?.upload(proj, name, bytes)
                    if (saved != null) {
                        // Claude Code 的 @路径 引用：图片直接看、文件直接读
                        composer = (composer.trim() + " @" + saved.saved_path + " ").trimStart()
                        Toast.makeText(context, "已上传：${saved.saved_path}", Toast.LENGTH_SHORT).show()
                    }
                } catch (e: Exception) { Toast.makeText(context, "上传失败：${e.message}", Toast.LENGTH_LONG).show() }
            }
        }
    }

    // 左上角 ☰：拉出项目面板（= 以前的首页整块：连接状态 / 终端 / 看板 / 设置 / 用量 / 新建 /
    // 项目列表与长按操作），点一行直接切会话（2026-09-07 用户拍板：抽屉有首页全部功能，首页去掉）
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            ModalDrawerSheet(
                modifier = Modifier.fillMaxWidth(0.92f),
                drawerContainerColor = Tok.Surface, drawerContentColor = Tok.Ink,
            ) {
                ProjectSwitcher(store, nav, currentPath = s?.project_path, onOpened = { scope.launch { drawerState.close() } })
            }
        },
    ) {
    Box(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding().imePadding()) {
    Column(Modifier.fillMaxSize()) {
        // 顶栏
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "☰", color = Tok.Dim, fontSize = 20.sp,
                modifier = Modifier.clickable { scope.launch { drawerState.open() } }.padding(horizontal = 8.dp, vertical = 2.dp),
            )
            // 顶栏只有一行（2026-09-08 用户拍板）：标题 + 模型 + 上下文占比。项目名、resume id、
            // 缓存命中率、花费都进详情屏——手机顶栏就这么宽，三行叠起来只是把标题挤扁
            Text(
                s?.title?.ifBlank { s.project_name } ?: sessionId,
                color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            val usageSegs = usageHeaderSegments(s?.usage)
            if (usageSegs.isNotEmpty()) {
                Spacer(Modifier.width(8.dp))
                Text(
                    segmentsAnnotated(usageSegs, Tok.Dim),
                    fontSize = 10.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            Spacer(Modifier.width(8.dp))
            // 视图切换：显示当前视图名，点一下换另一种
            if (messagesSupported != false) {
                Text(
                    if (showMessages) "消息流" else "终端",
                    color = Tok.Accent, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
                    modifier = Modifier
                        .clickable { uiMode = if (showMessages) "terminal" else "messages" }
                        .border(1.dp, Tok.Edge2, RoundedCornerShape(8.dp))
                        .padding(horizontal = 8.dp, vertical = 4.dp),
                )
                Spacer(Modifier.width(8.dp))
            }
            StateDot(Tok.stateColor(s?.state ?: ""))
            // 2026-09-08 用户拍板：⋮ 整个换成详情按钮——里面九项大半一年用一次，而
            // 子代理 / 后台任务 / 已上传 / 产物 / 技能这些「发生过但翻不出来」的才该占这个位置
            Text(
                "ⓘ", color = Tok.Dim, fontSize = 20.sp,
                modifier = Modifier.clickable { nav.openDetail(sessionId) }.padding(horizontal = 10.dp),
            )
        }
        if (!wsConnected && !showMessages) {
            Text("连接中…", color = Tok.Amber, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
        }

        // 快捷键条（仅终端视图，且要先用 ⌨ 放出来）：放在终端上方，软键盘弹起时不会被顶到看不见
        if (!showMessages && keysOpen) {
            Row(
                Modifier.fillMaxWidth().background(Tok.Surface).horizontalScroll(rememberScrollState())
                    .padding(horizontal = 8.dp, vertical = 5.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                // 键码交给 libvterm 的 dispatchKey，它按当前 keypad / cursor 模式给出正确的
                // 转义序列；字面符号直接写进 PTY。粘性 Ctrl 用一次就松开。
                fun key(code: Int) = {
                    vtermKeyFor(code)?.let { k ->
                        attachment?.emulator?.dispatchKey(if (ctrlSticky) VTERM_MOD_CTRL else 0, k)
                        ctrlSticky = false
                    }
                    Unit
                }
                fun lit(ch: String) = { attachment?.write(ch); Unit }
                KeyChip("⌨", active = true) { keysOpen = false }
                KeyChip("键盘") { inputRef.value?.showKeyboard() }
                // 回车放最前面：条会横向滚，排后面在手机上根本看不见
                KeyChip("⏎", onClick = key(KeyEvent.KEYCODE_ENTER))
                KeyChip("选择", active = selectMode.value) { selectMode.value = !selectMode.value }
                KeyChip("Esc", onClick = key(KeyEvent.KEYCODE_ESCAPE))
                KeyChip("Ctrl", active = ctrlSticky) { ctrlSticky = !ctrlSticky }
                KeyChip("↑", onClick = key(KeyEvent.KEYCODE_DPAD_UP))
                KeyChip("↓", onClick = key(KeyEvent.KEYCODE_DPAD_DOWN))
                KeyChip("←", onClick = key(KeyEvent.KEYCODE_DPAD_LEFT))
                KeyChip("→", onClick = key(KeyEvent.KEYCODE_DPAD_RIGHT))
                // Claude Code 的输入框里换行：`\` + Return（官方的「quick escape」，任何终端都认）。
                // 不用 Shift/Alt+Enter 的转义序列——那要终端和 TUI 两头都配好才不会被当成 Esc。
                KeyChip("换行", onClick = lit("\\\r"))
                KeyChip("Home", onClick = key(KeyEvent.KEYCODE_MOVE_HOME))
                KeyChip("End", onClick = key(KeyEvent.KEYCODE_MOVE_END))
                // 手机键盘上最难摸到的：路径与 slash 命令的 /（2026-09-06 去掉了 Tab、-、|、~）
                KeyChip("/", onClick = lit("/"))
                // 长按选区工具条里也有粘贴，但那要先长按选中；这里给一个直达入口
                KeyChip("粘贴") { pasteViaDaemon() }
            }
        }

        // 主体
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (showMessages) {
                MessagesView(
                    messages.value, messagesSupported, live = s?.state == "running",
                    sessionAlive = s?.state != "exited",
                    sessionCreatedAt = s?.created_at,
                    pending = pending.value,
                    onDeletePending = { deletePending(it) },
                    permission = s?.permission,
                    onPermission = { b -> store.client?.permission(sessionId, b) },
                    onAnswer = { _, answers ->
                        try {
                            store.client?.answer(sessionId, answers)
                        } catch (e: DaemonHttpException) {
                            Toast.makeText(context, e.message ?: "作答失败", Toast.LENGTH_LONG).show()
                            throw e
                        } catch (e: Exception) {
                            Toast.makeText(context, "作答失败：${e.message}", Toast.LENGTH_LONG).show()
                            throw e
                        }
                    },
                )
            } else {
                TerminalHost(
                    attachment, ctrlStickyState,
                    onHyperlinkClick = { openUrl(context, it) },
                    onPasteRequest = { pasteViaDaemon() },
                    screenText = { store.client?.screen(sessionId)?.text },
                    selectMode = selectMode,
                    inputRef = inputRef,
                )
            }
        }

        // composer：消息流视图始终在；终端视图下跟键位条一起收放
        if (showMessages || keysOpen) {
            Row(
                Modifier.fillMaxWidth().background(Tok.Surface).padding(horizontal = 8.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("📎", fontSize = 18.sp, modifier = Modifier.clickable { filePicker.launch("*/*") }.padding(6.dp))
                // 无边框的矮输入框：一块圆角底色，单行时一行高，最多长到 4 行。
                // 键盘回车是换行；多行文本 daemon 会包成一次粘贴发进去，不会在第一行就提交
                Box(
                    Modifier.weight(1f).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
                    contentAlignment = Alignment.CenterStart,
                ) {
                    if (composer.isEmpty()) Text("输入消息，可多行", color = Tok.Faint, fontSize = 14.sp)
                    BasicTextField(
                        composer, { composer = it },
                        modifier = Modifier.fillMaxWidth(),
                        maxLines = 4,
                        textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
                        cursorBrush = SolidColor(Tok.Accent),
                    )
                }
                Spacer(Modifier.width(6.dp))
                TextButton(
                    enabled = composer.isNotBlank() && s?.state != "exited",
                    onClick = { submitComposer() },
                    contentPadding = PaddingValues(horizontal = 10.dp, vertical = 4.dp),
                ) { Text("发送", color = if (composer.isNotBlank() && s?.state != "exited") Tok.Accent else Tok.Faint, fontSize = 14.sp) }
            }
        }
    }
    // 终端视图下键位条收起时：右下角一枚 ⌨ 把它放出来（悬浮在终端上，不占一行；
    // 条本身在顶部，按钮留在右下角是为了不盖住画面第一行的输出）
    if (!showMessages && !keysOpen) {
        Box(Modifier.align(Alignment.BottomEnd).padding(end = 14.dp, bottom = 14.dp)) { KeyChip("⌨") { keysOpen = true } }
    }
    }
    }

}

/** 进度清单的「3/7 完成」。清单本身在详情屏里画（DetailScreen.kt）。 */
fun checklistProgress(items: List<ChecklistItem>): String = "${items.count { it.done }}/${items.size} 完成"

@Composable
fun MessagesView(
    messages: List<ChatMessage>,
    supported: Boolean?,
    live: Boolean,
    sessionAlive: Boolean = true,
    sessionCreatedAt: String? = null,
    onAnswer: suspend (seq: Long, answers: List<AnswerItem>) -> Unit = { _, _ -> },
    /** 待发送（项目收件箱），画在末尾；✕ 撤回 */
    pending: List<InboxItem> = emptyList(),
    onDeletePending: (InboxItem) -> Unit = {},
    /** v1.16：正在等的权限对话框；浮在列表底部，允许 / 拒绝直接答 */
    permission: PermissionPrompt? = null,
    onPermission: suspend (behavior: String) -> Unit = {},
) {
    val scope = rememberCoroutineScope()
    val listState = rememberLazyListState()
    // 展开状态按轮 key 记；live 尾轮也默认折叠
    val expanded = remember { mutableStateMapOf<Long, Boolean>() }
    val expandedKeys = expanded.filterValues { it }.keys.toSet()
    val items = remember(messages, live, expandedKeys) { flattenForList(foldTurns(messages, live), expandedKeys) }
    // 表单状态：只有最新那条没被回答的 question 可交互；其余按「已回答 / 已结束 / 已过期」画成只读
    val pendingSeq = remember(messages, sessionAlive, sessionCreatedAt) { pendingQuestionSeq(messages, sessionAlive, sessionCreatedAt) }
    val answeredSeqs = remember(messages) { answeredQuestionSeqs(messages) }
    // 空列表算在底部：没东西可滚，浮动按钮也不该出现
    val atBottom by remember { derivedStateOf { !listState.canScrollForward } }
    val latestMessages by rememberUpdatedState(messages)
    val latestPending by rememberUpdatedState(pending)
    val latestItems by rememberUpdatedState(items)
    LaunchedEffect(Unit) {
        // 跟不跟到底看的是变化**之前**的位置：wasAtBottom 取自上一次快照，而不是新条目
        // 已经排进布局之后再读（那时 canScrollForward 必然为 true）。观察的是消息集
        // （条数 + 末条 seq + 待发送条数）而不是列表项数：展开 / 收起过程不是新消息，不触发滚动。
        var wasAtBottom = true
        var seen = -1L to -1L
        snapshotFlow { Triple(latestMessages.size + latestPending.size, latestMessages.lastOrNull()?.seq ?: -1L, atBottom) }
            .collect { (size, lastSeq, bottom) ->
                val sig = size.toLong() to lastSeq
                if (sig != seen) {
                    if (shouldFollowTail(wasAtBottom, hadMessages = seen.first > 0, hasMessages = size > 0)) {
                        listState.scrollToItem(latestItems.size + latestPending.size) // 尾部占位项才是真正的底
                    }
                    seen = sig
                }
                wasAtBottom = bottom
            }
    }
    if (messages.isEmpty() && pending.isEmpty()) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(
                when {
                    supported == null -> "加载消息…"
                    sessionAlive -> "新会话，还没有对话 · 在下面输入第一句话"
                    else -> "暂无消息"
                },
                color = Tok.Faint, fontSize = 13.sp,
            )
        }
        return
    }
    Box(Modifier.fillMaxSize()) {
        // v1.16：权限对话框（Bash 授权 / ExitPlanMode 批准…）浮在底部——以前它只弹在终端里，消息流一无所知
        if (permission != null && sessionAlive) {
            PermissionCard(permission, onPermission, Modifier.align(Alignment.BottomCenter).zIndex(2f))
        }
        LazyColumn(state = listState, modifier = Modifier.fillMaxSize().padding(horizontal = 14.dp)) {
            itemsIndexed(items, key = { _, it -> it.key }, contentType = { _, it -> it::class }) { i, item ->
                // 轮与轮之间 14dp（新一轮从用户消息开始），同轮内 User→Fold→Reply 8dp，展开的步骤间 3dp
                val gap = when {
                    i == 0 -> 0.dp
                    item is StreamItem.User -> 14.dp
                    item is StreamItem.Step -> 3.dp
                    else -> 8.dp
                }
                Box(Modifier.padding(top = gap)) {
                    StreamRow(
                        item, expanded,
                        formState = { q ->
                            when {
                                q.seq == pendingSeq -> FormState.PENDING
                                q.seq in answeredSeqs -> FormState.ANSWERED
                                !sessionAlive -> FormState.CLOSED
                                else -> FormState.STALE
                            }
                        },
                        onAnswer = onAnswer,
                    )
                }
            }
            // 待发送排在最后：还没进对话，但已经是「你说的话」，画在你这一侧
            itemsIndexed(pending, key = { _, it -> "pending-" + it.id }) { i, item ->
                Box(Modifier.padding(top = if (i == 0 && items.isEmpty()) 0.dp else 14.dp)) {
                    PendingBlock(item) { onDeletePending(item) }
                }
            }
            item(key = "tail") { Spacer(Modifier.height(14.dp)) }
        }
        ScrollToEndButton(
            visible = !atBottom,
            modifier = Modifier.align(Alignment.BottomEnd).padding(14.dp),
        ) { scope.launch { listState.animateScrollToItem(latestItems.size + latestPending.size) } }
    }
}

/** 表单的四种态：待答（可交互）/ 已回答 / 会话已结束 / 被更新的问题顶掉（悬着但不可答） */
enum class FormState { PENDING, ANSWERED, CLOSED, STALE }

@Composable
private fun StreamRow(
    item: StreamItem,
    expanded: MutableMap<Long, Boolean>,
    formState: (ChatMessage) -> FormState = { FormState.ANSWERED },
    onAnswer: suspend (Long, List<AnswerItem>) -> Unit = { _, _ -> },
) {
    when (item) {
        is StreamItem.User -> UserBlock(item.msg)
        is StreamItem.Fold -> FoldRow(item, open = expanded[item.foldKey] == true) {
            expanded[item.foldKey] = expanded[item.foldKey] != true
        }
        is StreamItem.Step -> StepRow(item.msg)
        is StreamItem.Reply -> ReplyBlock(item.msg)
        is StreamItem.Question -> QuestionCard(item.msg, formState(item.msg)) { answers -> onAnswer(item.msg.seq, answers) }
        is StreamItem.Answer -> AnswerBlock(item.msg)
    }
}

/** 用户消息：右对齐气泡（最宽 86%），强调色淡底 + 细边，右下角收小；时间在气泡上方靠右。 */
@Composable
private fun UserBlock(m: ChatMessage) {
    val time = remember(m.ts) { clockTime(m.ts) }
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        if (time.isNotEmpty()) BubbleCaption(time, Tok.Faint)
        UserBubble {
            SelectionContainer { Text(rememberLinkified(m), color = Tok.Ink, fontSize = 14.5.sp, lineHeight = 21.sp) }
        }
    }
}

/**
 * 待发送：和用户气泡同侧同款，但底色更淡、边框虚一点，小字写「待发送」，右侧 ✕ 撤回。
 * daemon 在 agent 这轮跑完（或此刻已空着）时把它写进去，写进去后它就变成一条普通的用户消息。
 */
@Composable
private fun PendingBlock(item: InboxItem, onDelete: () -> Unit) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        BubbleCaption("待发送 · 执行完自动发出", Tok.Amber)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                "✕", color = Tok.Faint, fontSize = 14.sp,
                modifier = Modifier.clickable(onClick = onDelete).padding(horizontal = 8.dp, vertical = 6.dp),
            )
            UserBubble(tint = Tok.Amber) {
                Text(item.text, color = Tok.Dim, fontSize = 14.5.sp, lineHeight = 21.sp)
            }
        }
    }
}

/**
 * 「发送」是直接写进 PTY 还是排成待发送：agent 在跑就排队（和终端里先敲好等它一样）；
 * 弹着问题也排队——自由文本会替你按下高亮项，问题请用表单答。空着才直接发。
 */
fun queueInsteadOfSend(state: String?, asking: Boolean): Boolean = state == "running" || asking

/** Claude 的回复：左对齐整宽、不画气泡；上方一行强调色的「✻ Claude」小字，正文仍是 Markdown。 */
@Composable
private fun ReplyBlock(m: ChatMessage) {
    Column(Modifier.fillMaxWidth()) {
        Text(
            "✻ Claude", color = Tok.Accent, fontSize = 10.5.sp, fontWeight = FontWeight.Medium,
            modifier = Modifier.padding(bottom = 4.dp),
        )
        // 长按选字复制；链接仍是点一下打开
        SelectionContainer { MarkdownBody(m.text, 15.sp, Tok.Ink, modifier = Modifier.fillMaxWidth()) }
    }
}

/** 气泡上方那行小字（时间 / 「回答」），与气泡同在右侧。 */
@Composable
private fun BubbleCaption(text: String, color: Color) {
    Text(text, color = color, fontSize = 10.sp, modifier = Modifier.padding(bottom = 3.dp, end = 2.dp))
}

/** 右下角收小的气泡：说话的人在右边。 */
private val UserBubbleShape = RoundedCornerShape(topStart = 14.dp, topEnd = 14.dp, bottomEnd = 6.dp, bottomStart = 14.dp)

/**
 * 用户一侧的气泡：[tint] 淡底（暗主题 0.18、亮主题 0.14——亮底上同样的透明度会显得脏）
 * 加 0.35 的一像素边。宽度跟着内容走，最多占父宽 86%。
 */
@Composable
private fun UserBubble(tint: Color = Tok.Accent, content: @Composable () -> Unit) {
    val fill = tint.copy(alpha = if (Tok.current.isDark) 0.18f else 0.14f)
    Box(
        Modifier.maxWidthFraction(0.86f)
            .background(fill, UserBubbleShape)
            .border(1.dp, tint.copy(alpha = 0.35f), UserBubbleShape)
            .padding(horizontal = 12.dp, vertical = 9.dp),
    ) { content() }
}

/** 只压最大宽度为父宽的 [fraction]，内容短就跟着窄——fillMaxWidth(f) 会把每个气泡都撑到一样宽。 */
private fun Modifier.maxWidthFraction(fraction: Float): Modifier = layout { measurable, constraints ->
    val max = if (constraints.hasBoundedWidth) (constraints.maxWidth * fraction).roundToInt() else constraints.maxWidth
    val placeable = measurable.measure(constraints.copy(minWidth = 0, maxWidth = max))
    layout(placeable.width, placeable.height) { placeable.placeRelative(0, 0) }
}

/** 折叠行。live 尾轮用旋转指示代替 ▸，折叠着也报最近一步在干什么。 */
@Composable
private fun FoldRow(f: StreamItem.Fold, open: Boolean, onToggle: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onToggle).padding(vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(10.dp), contentAlignment = Alignment.Center) {
            if (f.live) CircularProgressIndicator(Modifier.size(10.dp), color = Tok.Accent, strokeWidth = 1.5.dp)
            else StateDot(Tok.Faint, 6)
        }
        Spacer(Modifier.width(6.dp))
        val label = when {
            f.live && open -> "进行中 · ${stepCount(f.steps)} 步"
            f.live -> liveLabel(f.steps, f.liveTail)
            open -> "▾ 过程 · ${stepCount(f.steps)} 步"
            else -> "▸ ${foldLabel(f.steps)}"
        }
        Text(label, color = if (f.live) Tok.Dim else Tok.Faint, fontSize = 12.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** 展开后的单步：思考 / 工具复用原有行，中途文本压成 13sp 淡字，system 沿用淡字。 */
@Composable
private fun StepRow(m: ChatMessage) {
    Box(Modifier.fillMaxWidth().padding(start = 12.dp)) {
        when {
            m.kind == "thinking" -> ThinkingRow(m)
            m.kind == "tool_use" || m.kind == "tool_result" -> ToolRow(m)
            m.role == "system" -> Text(
                rememberLinkified(m), color = Tok.Faint, fontSize = 11.sp,
                modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
            )
            else -> MarkdownBody(
                m.text, 13.5.sp, Tok.Dim,
                modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
            )
        }
    }
}

/** 右下角「滚到最新」：到底了就淡出。 */
@Composable
private fun ScrollToEndButton(visible: Boolean, modifier: Modifier, onClick: () -> Unit) {
    AnimatedVisibility(visible, modifier = modifier, enter = fadeIn(), exit = fadeOut()) {
        Box(
            Modifier.size(40.dp).clip(CircleShape).background(Tok.Raised)
                .border(1.dp, Tok.Edge2, CircleShape).clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) { Text("↓", color = Tok.Accent, fontSize = 18.sp) }
    }
}

/** 消息正文 → 带可点链接的富文本。只在文本变化时重扫，滚动时不重复做正则。 */
@Composable
private fun rememberLinkified(m: ChatMessage): AnnotatedString {
    val context = LocalContext.current
    // 链接色烙在 AnnotatedString 里，换主题要重扫一遍
    return remember(m.seq, m.text, Tok.current) { linkified(m.text) { url -> openUrl(context, url) } }
}

@Composable
private fun ThinkingRow(m: ChatMessage) {
    var expanded by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth().padding(vertical = 3.dp)
            .background(Tok.Inset, RoundedCornerShape(10.dp))
            .clickable { expanded = !expanded }
            .padding(horizontal = 10.dp, vertical = 6.dp),
    ) {
        Text(
            (if (expanded) "▾ " else "▸ ") + "思考",
            color = Tok.Faint, fontSize = 11.sp, fontStyle = FontStyle.Italic,
        )
        if (expanded) Text(m.text, color = Tok.Faint, fontSize = 12.sp, fontStyle = FontStyle.Italic, modifier = Modifier.padding(top = 4.dp))
        else if (m.text.isNotBlank()) Text(m.text.lineSequence().first(), color = Tok.Faint, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, fontStyle = FontStyle.Italic)
    }
}

@Composable
private fun ToolRow(m: ChatMessage) {
    var expanded by remember { mutableStateOf(false) }
    val statusColor = when (m.tool?.status) {
        "ok" -> Tok.Green; "err" -> Tok.Red; "running" -> Tok.Amber; else -> Tok.Dim
    }
    Column(
        Modifier.fillMaxWidth().padding(vertical = 2.dp)
            .clickable { expanded = !expanded },
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StateDot(statusColor, 6)
            Spacer(Modifier.width(7.dp))
            Text(
                if (m.kind == "tool_result") "⎿ 结果" else (m.tool?.name ?: "tool"),
                color = Tok.Ink, fontSize = 12.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.width(8.dp))
            Text(
                m.tool?.summary.orEmpty(), color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
        }
        if (expanded && m.text.isNotBlank()) {
            // 工具输出（curl、报错里的文档地址）是链接最集中的地方，展开后要能点
            Text(
                rememberLinkified(m), color = Tok.Dim, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                modifier = Modifier.fillMaxWidth().padding(start = 13.dp, top = 3.dp)
                    .background(Tok.Inset, RoundedCornerShape(8.dp)).padding(8.dp),
            )
        }
    }
}

/**
 * 原生表单：agent 的 AskUserQuestion 不再是一行「? 问题」，而是按结构化数据画出来——
 * 单选画单选、多选画复选、末尾固定一条「其它」自填。提交交给 daemon 翻译成对话框按键。
 * 只有 [state] == PENDING 的那张卡可交互；其它状态同布局、只读、带一个小标签。
 */
@Composable
private fun QuestionCard(m: ChatMessage, state: FormState, onSubmit: suspend (List<AnswerItem>) -> Unit) {
    val spec = m.question
    val scope = rememberCoroutineScope()
    val pending = state == FormState.PENDING
    val n = spec?.questions?.size ?: 0
    var selections by remember(m.seq) { mutableStateOf(List(n) { emptySet<Int>() }) }
    var others by remember(m.seq) { mutableStateOf(List(n) { "" }) }
    var submitting by remember(m.seq) { mutableStateOf(false) }
    var error by remember(m.seq) { mutableStateOf<String?>(null) }

    fun complete(i: Int, q: QuestionItem): Boolean {
        val sel = selections[i]; val other = others[i].isNotBlank()
        return if (q.multi_select) sel.isNotEmpty() || other else (sel.size == 1 && !other) || (sel.isEmpty() && other)
    }
    val allComplete = spec != null && spec.questions.withIndex().all { (i, q) -> complete(i, q) }

    Column(
        Modifier.fillMaxWidth().padding(vertical = 4.dp)
            .border(1.dp, Tok.Amber.copy(alpha = 0.55f), RoundedCornerShape(10.dp))
            .background(Tok.Amber.copy(alpha = 0.08f), RoundedCornerShape(10.dp))
            .padding(12.dp)
            .alpha(if (pending) 1f else 0.6f),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (spec == null) {
            Text("? ${m.text}", color = Tok.Amber, fontSize = 13.sp, fontWeight = FontWeight.Bold)
            return@Column
        }
        spec.questions.forEachIndexed { qi, q ->
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (q.header.isNotBlank()) {
                        Text(q.header, color = Tok.Amber, fontSize = 11.sp, fontWeight = FontWeight.Bold)
                    }
                    if (q.multi_select) Text("可多选", color = Tok.Faint, fontSize = 10.sp)
                }
                Text(q.question, color = Tok.Ink, fontSize = 14.sp, fontWeight = FontWeight.Medium, lineHeight = 20.sp)
                q.options.forEachIndexed { oi, opt ->
                    val selected = oi in selections[qi]
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp))
                            .then(if (pending) Modifier.clickable {
                                selections = selections.toMutableList().also { list ->
                                    list[qi] = if (q.multi_select) (if (selected) list[qi] - oi else list[qi] + oi) else setOf(oi)
                                }
                                if (!q.multi_select) others = others.toMutableList().also { it[qi] = "" }
                            } else Modifier)
                            .padding(vertical = 4.dp, horizontal = 2.dp),
                        verticalAlignment = Alignment.Top,
                    ) {
                        Text(
                            if (q.multi_select) (if (selected) "☑" else "☐") else (if (selected) "●" else "○"),
                            color = if (selected) Tok.Accent else Tok.Dim, fontSize = 15.sp, fontFamily = FontFamily.Monospace,
                            modifier = Modifier.padding(end = 10.dp, top = 1.dp),
                        )
                        Column(Modifier.weight(1f)) {
                            Text(opt.label, color = Tok.Ink, fontSize = 14.sp)
                            if (opt.description.isNotBlank()) Text(opt.description, color = Tok.Dim, fontSize = 12.sp, lineHeight = 16.sp)
                        }
                    }
                }
                // 其它：自填一行；单选里有字就顶掉圆点，多选里算多勾一项
                val other = others[qi]
                Row(Modifier.fillMaxWidth().padding(vertical = 2.dp, horizontal = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                    val otherOn = other.isNotBlank()
                    Text(
                        if (q.multi_select) (if (otherOn) "☑" else "☐") else (if (otherOn) "●" else "○"),
                        color = if (otherOn) Tok.Accent else Tok.Dim, fontSize = 15.sp, fontFamily = FontFamily.Monospace,
                        modifier = Modifier.padding(end = 10.dp),
                    )
                    BasicTextField(
                        value = other,
                        onValueChange = { v ->
                            val one = v.replace('\n', ' ')
                            others = others.toMutableList().also { it[qi] = one }
                            if (!q.multi_select && one.isNotBlank()) selections = selections.toMutableList().also { it[qi] = emptySet() }
                        },
                        enabled = pending,
                        singleLine = true,
                        textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp),
                        cursorBrush = SolidColor(Tok.Accent),
                        modifier = Modifier.weight(1f).background(Tok.Inset, RoundedCornerShape(6.dp)).padding(horizontal = 10.dp, vertical = 7.dp),
                        decorationBox = { inner ->
                            if (other.isEmpty()) Text("其它…", color = Tok.Faint, fontSize = 14.sp)
                            inner()
                        },
                    )
                }
            }
        }
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            when (state) {
                FormState.PENDING -> {}
                FormState.ANSWERED -> Tag("已回答")
                FormState.CLOSED -> Tag("已结束")
                FormState.STALE -> Tag("已过期")
            }
            error?.let { Text(it, color = Tok.Red, fontSize = 11.sp, modifier = Modifier.weight(1f).padding(end = 8.dp)) }
                ?: Spacer(Modifier.weight(1f))
            if (pending) {
                val enabled = allComplete && !submitting
                Text(
                    if (submitting) "提交中…" else "提交",
                    color = if (enabled) Tok.OnAccent else Tok.Faint, fontSize = 13.sp, fontWeight = FontWeight.Bold,
                    modifier = Modifier
                        .background(if (enabled) Tok.Accent else Tok.Edge2, RoundedCornerShape(8.dp))
                        .then(if (enabled) Modifier.clickable {
                            val answers = spec.questions.indices.map { i ->
                                AnswerItem(selected = selections[i].sorted(), other = others[i].trim().ifBlank { null })
                            }
                            scope.launch {
                                submitting = true
                                try { onSubmit(answers); error = null }
                                catch (e: Exception) { error = (e.message ?: "作答失败") + " · 请到终端处理" }
                                finally { submitting = false }
                            }
                        } else Modifier)
                        .padding(horizontal = 16.dp, vertical = 8.dp),
                )
            }
        }
    }
}

@Composable
private fun Tag(label: String) {
    Text(
        label, color = Tok.Dim, fontSize = 10.sp,
        modifier = Modifier.border(1.dp, Tok.Edge2, RoundedCornerShape(50)).padding(horizontal = 8.dp, vertical = 2.dp),
    )
}

/** 用户对表单的回答：与 UserBlock 同一侧同一款气泡，小字写「回答」；出错时边与小字用红。 */
@Composable
private fun AnswerBlock(m: ChatMessage) {
    val time = remember(m.ts) { clockTime(m.ts) }
    val err = m.tool?.status == "err"
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        BubbleCaption(
            listOf(time, if (err) "回答 · 出错" else "回答").filter { it.isNotEmpty() }.joinToString(" · "),
            if (err) Tok.Red else Tok.Faint,
        )
        UserBubble(tint = if (err) Tok.Red else Tok.Accent) {
            Text(m.text, color = Tok.Ink, fontSize = 14.sp, lineHeight = 20.sp)
        }
    }
}

// ---------- A4 会话操作菜单 ----------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SheetItem(icon: String, label: String, note: String?, danger: Boolean = false, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 18.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(icon, fontSize = 14.sp, modifier = Modifier.width(28.dp))
        Text(label, color = if (danger) Tok.Red else Tok.Ink, fontSize = 15.sp, modifier = Modifier.weight(1f))
        note?.let { Text(it, color = Tok.Faint, fontSize = 11.sp) }
    }
}

@Composable
fun ConfirmDialog(title: String, body: String, confirmLabel: String, onConfirm: () -> Unit, onCancel: () -> Unit) {
    AlertDialog(
        onDismissRequest = onCancel,
        containerColor = Tok.Raised,
        title = { Text(title, color = Tok.Ink) },
        text = { Text(body, color = Tok.Dim) },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirmLabel, color = Tok.Red) } },
        dismissButton = { TextButton(onClick = onCancel) { Text("取消", color = Tok.Dim) } },
    )
}

// ---------- helpers ----------

fun copyToClipboard(context: Context, text: String) {
    (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
        .setPrimaryClip(ClipData.newPlainText("aaa-ui", text))
}

fun readUri(context: Context, uri: Uri): Pair<String, ByteArray> {
    var name = "attachment"
    context.contentResolver.query(uri, null, null, null, null)?.use { cursor ->
        val idx = cursor.getColumnIndex(android.provider.OpenableColumns.DISPLAY_NAME)
        if (idx >= 0 && cursor.moveToFirst()) cursor.getString(idx)?.let { name = it }
    }
    val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() }
        ?: throw IllegalStateException("无法读取文件")
    require(bytes.size <= 50 * 1024 * 1024) { "文件超过 50MB 限制" }
    return name to bytes
}

/** 权限对话框卡片：工具名 + 命令摘要 + 允许 / 拒绝；失败（对话框还在）把话留在卡片上 */
@Composable
private fun PermissionCard(p: PermissionPrompt, onDecide: suspend (String) -> Unit, modifier: Modifier = Modifier) {
    val scope = rememberCoroutineScope()
    var busy by remember(p) { mutableStateOf(false) }
    var error by remember(p) { mutableStateOf<String?>(null) }
    fun decide(b: String) {
        if (busy) return
        busy = true; error = null
        scope.launch {
            try { onDecide(b) } catch (e: Exception) { error = e.message ?: "失败" } finally { busy = false }
        }
    }
    Column(
        modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)
            .background(Tok.Amber.copy(alpha = 0.10f), RoundedCornerShape(10.dp))
            .border(1.dp, Tok.Amber.copy(alpha = 0.7f), RoundedCornerShape(10.dp))
            .padding(12.dp),
    ) {
        Text(if (p.kind == "elicitation") "⚠ 有个表单在等你" else "⚠ Claude 请求授权 · " + p.tool_name.ifBlank { "工具" }, color = Tok.Amber, fontSize = 13.sp, fontWeight = FontWeight.Bold)
        if (p.summary.isNotBlank()) Text(p.summary, color = Tok.Ink, fontSize = 12.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(top = 4.dp), maxLines = 4, overflow = TextOverflow.Ellipsis)
        if (p.kind == "elicitation") {
            Text("MCP 服务器要你填表单：这个只能在终端视图里答", color = Tok.Dim, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        } else Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("允许", color = Tok.Ink, fontSize = 13.sp, modifier = Modifier.background(Tok.Green.copy(alpha = 0.2f), RoundedCornerShape(6.dp)).border(1.dp, Tok.Green.copy(alpha = 0.6f), RoundedCornerShape(6.dp)).clickable(enabled = !busy) { decide("allow") }.padding(horizontal = 14.dp, vertical = 6.dp))
            Spacer(Modifier.width(8.dp))
            Text("拒绝", color = Tok.Ink, fontSize = 13.sp, modifier = Modifier.background(Tok.Red.copy(alpha = 0.2f), RoundedCornerShape(6.dp)).border(1.dp, Tok.Red.copy(alpha = 0.6f), RoundedCornerShape(6.dp)).clickable(enabled = !busy) { decide("deny") }.padding(horizontal = 14.dp, vertical = 6.dp))
            Spacer(Modifier.width(10.dp))
            Text(if (busy) "…" else "在终端里作答也一样", color = Tok.Faint, fontSize = 11.sp)
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp)) }
    }
}
