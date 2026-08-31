package cc.uoox.aaaui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Typeface
import android.net.Uri
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.inputmethod.InputMethodManager
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.navigation.NavHostController
import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient
import com.termux.view.TerminalView
import com.termux.view.TerminalViewClient
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

// ============================================================
// 会话屏：消息流 ⇄ 终端 双视图 + 共用 composer
// ============================================================

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionScreen(store: AppStore, nav: NavHostController, sessionId: String, prefill: String) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val sessions by store.sessions.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val session = sessions.find { it.id == sessionId }

    var helloSession by remember { mutableStateOf<Session?>(null) }
    val s = session ?: helloSession

    // 消息流支持探测：null=未知，true/false=已知
    var messagesSupported by remember { mutableStateOf<Boolean?>(null) }
    var uiMode by rememberSaveable { mutableStateOf("") } // "" = 未定，跟随默认设置
    val effectiveMode = when {
        uiMode.isNotEmpty() -> uiMode
        messagesSupported == false -> "terminal"
        else -> settings.defaultUi
    }
    val showMessages = effectiveMode == "messages" && messagesSupported != false

    var composer by rememberSaveable { mutableStateOf(prefill) }
    var showMenu by remember { mutableStateOf(false) }
    var ctrlSticky by remember { mutableStateOf(false) }

    // 终端 attach（惰性创建，跨模式切换保留；daemon 重连后换新 client 重建）
    val terminalViewRef = remember { mutableStateOf<TerminalView?>(null) }
    var wsConnected by remember { mutableStateOf(false) }
    val conn by store.connState.collectAsState()
    val attachmentHolder = remember(sessionId) { mutableStateOf<TerminalAttachment?>(null) }
    val terminalClient = remember(sessionId) {
        object : TerminalSessionClient {
                override fun onTextChanged(changedSession: TerminalSession) { terminalViewRef.value?.onScreenUpdated() }
                override fun onTitleChanged(changedSession: TerminalSession) {}
                override fun onSessionFinished(finishedSession: TerminalSession) {}
                override fun onCopyTextToClipboard(session: TerminalSession, text: String) { copyToClipboard(context, text) }
                override fun onPasteTextFromClipboard(session: TerminalSession?) {
                    val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
                    val text = clip?.getItemAt(0)?.coerceToText(context)?.toString()
                    if (!text.isNullOrEmpty()) session?.write(text)
                }
                override fun onBell(session: TerminalSession) {}
                override fun onColorsChanged(session: TerminalSession) {}
                override fun onTerminalCursorStateChange(state: Boolean) {}
                override fun setTerminalShellPid(session: TerminalSession, pid: Int) {}
                override fun getTerminalCursorStyle(): Int? = null
                override fun logError(tag: String?, message: String?) {}
                override fun logWarn(tag: String?, message: String?) {}
                override fun logInfo(tag: String?, message: String?) {}
                override fun logDebug(tag: String?, message: String?) {}
                override fun logVerbose(tag: String?, message: String?) {}
                override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) {}
                override fun logStackTrace(tag: String?, e: Exception?) {}
            }
    }
    LaunchedEffect(conn, sessionId) {
        val api = store.client
        if (api != null && attachmentHolder.value?.api !== api) {
            attachmentHolder.value?.stop()
            attachmentHolder.value = TerminalAttachment(
                api, sessionId, terminalClient,
                onHello = { helloSession = it },
                onConnectionChange = { wsConnected = it },
            ).also { it.start() }
        }
    }
    val attachment = attachmentHolder.value
    DisposableEffect(sessionId) {
        onDispose { attachmentHolder.value?.stop() }
    }
    LaunchedEffect(sessionId) { if (session == null) store.refreshSessions() }

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

    fun sendInput(text: String, enter: Boolean) {
        scope.launch {
            try { store.client?.input(sessionId, text, enter) }
            catch (e: Exception) { Toast.makeText(context, "发送失败：${e.message}", Toast.LENGTH_SHORT).show() }
        }
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
                        composer = (composer.trim() + " " + saved.saved_path).trim()
                        Toast.makeText(context, "已上传：${saved.saved_path}", Toast.LENGTH_SHORT).show()
                    }
                } catch (e: Exception) { Toast.makeText(context, "上传失败：${e.message}", Toast.LENGTH_LONG).show() }
            }
        }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding().imePadding()) {
        // 顶栏
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    s?.title?.ifBlank { s.project_name } ?: sessionId,
                    color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                Text(
                    listOfNotNull(s?.project_name, s?.agent, s?.resume_id?.let { "resume ${it.take(6)}" }).joinToString(" · "),
                    color = Tok.Faint, fontSize = 10.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            if (messagesSupported != false) {
                Text(
                    if (showMessages) ">_" else "💬",
                    color = Tok.Cyan, fontSize = 15.sp, fontFamily = FontFamily.Monospace,
                    modifier = Modifier
                        .clickable { uiMode = if (showMessages) "terminal" else "messages" }
                        .border(1.dp, Tok.Edge2, RoundedCornerShape(8.dp))
                        .padding(horizontal = 8.dp, vertical = 4.dp),
                )
                Spacer(Modifier.width(8.dp))
            }
            StateDot(Tok.stateColor(s?.state ?: ""))
            Text("⋮", color = Tok.Dim, fontSize = 22.sp, modifier = Modifier.clickable { showMenu = true }.padding(horizontal = 10.dp))
        }
        if (!wsConnected && !showMessages) {
            Text("连接中…", color = Tok.Amber, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 16.dp))
        }

        // 主体
        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (showMessages) {
                MessagesView(messages.value, messagesSupported)
            } else {
                TerminalHost(attachment, terminalViewRef, settings.fontSize,
                    viewClientFactory = { view ->
                        object : TerminalViewClient {
                            override fun onScale(scale: Float): Float {
                                if (scale < 0.9f || scale > 1.1f) {
                                    val target = (settings.fontSize * scale).toInt().coerceIn(8, 28)
                                    scope.launch { store.settings.setFontSize(target) }
                                    return 1.0f
                                }
                                return scale
                            }
                            override fun onSingleTapUp(e: MotionEvent?) {
                                view.requestFocus()
                                (context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager)
                                    .showSoftInput(view, InputMethodManager.SHOW_IMPLICIT)
                            }
                            override fun shouldBackButtonBeMappedToEscape() = false
                            override fun shouldEnforceCharBasedInput() = true
                            override fun shouldUseCtrlSpaceWorkaround() = false
                            override fun isTerminalViewSelected() = true
                            override fun copyModeChanged(copyMode: Boolean) {}
                            override fun onKeyDown(keyCode: Int, e: KeyEvent?, session: TerminalSession?) = false
                            override fun onKeyUp(keyCode: Int, e: KeyEvent?) = false
                            override fun onLongPress(event: MotionEvent?) = false
                            override fun readControlKey() = ctrlSticky
                            override fun readAltKey() = false
                            override fun readShiftKey() = false
                            override fun readFnKey() = false
                            override fun onCodePoint(codePoint: Int, ctrlDown: Boolean, session: TerminalSession?): Boolean {
                                if (ctrlSticky) view.post { ctrlSticky = false }
                                return false
                            }
                            override fun onEmulatorSet() {}
                            override fun logError(tag: String?, message: String?) {}
                            override fun logWarn(tag: String?, message: String?) {}
                            override fun logInfo(tag: String?, message: String?) {}
                            override fun logDebug(tag: String?, message: String?) {}
                            override fun logVerbose(tag: String?, message: String?) {}
                            override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) {}
                            override fun logStackTrace(tag: String?, e: Exception?) {}
                        }
                    })
            }
        }

        // waiting 时的 question 选项胶囊
        if (s?.state == "waiting" && s.question != null) {
            Column(Modifier.fillMaxWidth().background(Tok.Surface).padding(horizontal = 12.dp, vertical = 8.dp)) {
                Text("? ${s.question.text}", color = Tok.Amber, fontSize = 13.sp, fontWeight = FontWeight.Bold)
                if (s.question.options.isNotEmpty()) {
                    Row(
                        Modifier.horizontalScroll(rememberScrollState()).padding(top = 6.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        s.question.options.forEach { opt ->
                            Text(
                                "${opt.key} · ${opt.label}", color = Tok.Cyan, fontSize = 13.sp,
                                modifier = Modifier
                                    .background(Tok.Cyan.copy(alpha = 0.14f), RoundedCornerShape(50))
                                    .clickable { sendInput(opt.key, enter = !opt.key.all { ch -> ch.isDigit() }) }
                                    .padding(horizontal = 12.dp, vertical = 6.dp),
                            )
                        }
                    }
                }
            }
        }

        // 快捷键条（仅终端视图）
        if (!showMessages) {
            Row(
                Modifier.fillMaxWidth().background(Tok.Surface).horizontalScroll(rememberScrollState())
                    .padding(horizontal = 8.dp, vertical = 5.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                KeyChip("Esc") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_ESCAPE, 0) }
                KeyChip("Tab") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_TAB, 0) }
                KeyChip("Ctrl", active = ctrlSticky) { ctrlSticky = !ctrlSticky }
                KeyChip("↑") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_DPAD_UP, 0) }
                KeyChip("↓") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_DPAD_DOWN, 0) }
                KeyChip("←") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_DPAD_LEFT, 0) }
                KeyChip("→") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_DPAD_RIGHT, 0) }
                KeyChip("⏎") { terminalViewRef.value?.handleKeyCode(KeyEvent.KEYCODE_ENTER, 0) }
                KeyChip("/") { attachment?.session?.write("/") }
            }
        }

        // 快捷短语 chips
        if (settings.quickPhrases.isNotEmpty() && s?.state != "exited") {
            Row(
                Modifier.fillMaxWidth().background(Tok.Bg).horizontalScroll(rememberScrollState())
                    .padding(horizontal = 8.dp, vertical = 4.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                settings.quickPhrases.forEach { phrase ->
                    Text(
                        phrase, color = Tok.Dim, fontSize = 12.sp,
                        modifier = Modifier
                            .border(1.dp, Tok.Edge, RoundedCornerShape(50))
                            .clickable { sendInput(phrase, enter = true) }
                            .padding(horizontal = 10.dp, vertical = 4.dp),
                    )
                }
            }
        }

        // composer
        Row(
            Modifier.fillMaxWidth().background(Tok.Surface).padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("📎", fontSize = 18.sp, modifier = Modifier.clickable { filePicker.launch("*/*") }.padding(6.dp))
            OutlinedTextField(
                composer, { composer = it },
                placeholder = { Text("输入消息，⏎ 发送", color = Tok.Faint, fontSize = 13.sp) },
                modifier = Modifier.weight(1f),
                maxLines = 4,
                textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
            )
            Spacer(Modifier.width(6.dp))
            Button(
                enabled = composer.isNotBlank() && s?.state != "exited",
                onClick = { sendInput(composer, enter = true); composer = "" },
            ) { Text("发送") }
        }
    }

    if (showMenu && s != null) {
        SessionMenuSheet(store, nav, s, attachment, showMessagesMode = showMessages, onDismiss = { showMenu = false })
    }
}

@Composable
private fun KeyChip(label: String, active: Boolean = false, onClick: () -> Unit) {
    Text(
        label,
        color = if (active) Tok.Bg else Tok.Ink,
        fontSize = 13.sp,
        fontFamily = FontFamily.Monospace,
        modifier = Modifier
            .background(if (active) Tok.Cyan else Tok.Raised, RoundedCornerShape(7.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
    )
}

// ---------- 终端宿主 ----------

@Composable
private fun TerminalHost(
    attachment: TerminalAttachment?,
    viewRef: androidx.compose.runtime.MutableState<TerminalView?>,
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
                setBackgroundColor(android.graphics.Color.parseColor("#0A0E12"))
                setTerminalViewClient(viewClientFactory(this))
                // setTextSize must come first: it constructs the renderer (and
                // is null-safe), while setTypeface reads the existing one and
                // would NPE on a freshly built view.
                setTextSize(with(density) { fontSize.sp.toPx() }.toInt())
                setTypeface(Typeface.MONOSPACE)
                attachSession(attachment.session)
                keepScreenOn = true
                viewRef.value = this
            }
        },
        update = { view ->
            val px = with(density) { fontSize.sp.toPx() }.toInt()
            if (view.tag != px) { view.tag = px; view.setTextSize(px) }
            if (view.currentSession !== attachment.session) view.attachSession(attachment.session)
        },
        modifier = Modifier.fillMaxSize().background(Tok.TermBg),
    )
}

// ---------- 消息流视图 ----------

@Composable
fun MessagesView(messages: List<ChatMessage>, supported: Boolean?) {
    val listState = rememberLazyListState()
    LaunchedEffect(messages.size) {
        if (messages.isNotEmpty()) listState.scrollToItem(messages.size - 1)
    }
    if (messages.isEmpty()) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text(if (supported == null) "加载消息…" else "暂无消息", color = Tok.Faint)
        }
        return
    }
    LazyColumn(state = listState, modifier = Modifier.fillMaxSize().padding(horizontal = 10.dp)) {
        items(messages, key = { it.seq }) { m -> MessageRow(m) }
        item { Spacer(Modifier.height(8.dp)) }
    }
}

@Composable
private fun MessageRow(m: ChatMessage) {
    when {
        m.kind == "thinking" -> ThinkingRow(m)
        m.kind == "tool_use" || m.kind == "tool_result" -> ToolRow(m)
        m.kind == "question" -> QuestionRow(m)
        m.role == "user" -> Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), horizontalArrangement = Arrangement.End) {
            Text(
                m.text, color = Tok.Ink, fontSize = 14.sp,
                modifier = Modifier.widthIn(max = 300.dp)
                    .background(Tok.Cyan.copy(alpha = 0.16f), RoundedCornerShape(14.dp, 14.dp, 4.dp, 14.dp))
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
        m.role == "system" -> Text(
            m.text, color = Tok.Faint, fontSize = 11.sp,
            modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        )
        else -> Row(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
            Text(
                m.text, color = Tok.Ink, fontSize = 14.sp,
                modifier = Modifier.widthIn(max = 320.dp)
                    .background(Tok.Surface, RoundedCornerShape(14.dp, 14.dp, 14.dp, 4.dp))
                    .padding(horizontal = 12.dp, vertical = 8.dp),
            )
        }
    }
}

@Composable
private fun ThinkingRow(m: ChatMessage) {
    var expanded by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth().padding(vertical = 3.dp)
            .background(Tok.TermBg, RoundedCornerShape(10.dp))
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
            Text(
                m.text, color = Tok.Dim, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                modifier = Modifier.fillMaxWidth().padding(start = 13.dp, top = 3.dp)
                    .background(Tok.TermBg, RoundedCornerShape(8.dp)).padding(8.dp),
            )
        }
    }
}

@Composable
private fun QuestionRow(m: ChatMessage) {
    Column(
        Modifier.fillMaxWidth().padding(vertical = 4.dp)
            .border(1.dp, Tok.Amber.copy(alpha = 0.55f), RoundedCornerShape(10.dp))
            .background(Tok.Amber.copy(alpha = 0.08f), RoundedCornerShape(10.dp))
            .padding(10.dp),
    ) {
        Text("? ${m.text}", color = Tok.Amber, fontSize = 13.sp, fontWeight = FontWeight.Bold)
    }
}

// ---------- A4 会话操作菜单 ----------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionMenuSheet(
    store: AppStore,
    nav: NavHostController,
    s: Session,
    attachment: TerminalAttachment?,
    showMessagesMode: Boolean,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    var renameDialog by remember { mutableStateOf(false) }
    var killDialog by remember { mutableStateOf(false) }
    var deleteDialog by remember { mutableStateOf(false) }
    var portsDialog by remember { mutableStateOf<List<PortInfo>?>(null) }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = Tok.Surface) {
        Column(Modifier.padding(bottom = 20.dp)) {
            Column(Modifier.padding(horizontal = 18.dp, vertical = 4.dp)) {
                Text(s.title.ifBlank { s.project_name }, color = Tok.Ink, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                Text("${s.agent} · ${s.project_path}", color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
            }
            SheetItem("🌐", "打开 Web 预览", "检测端口 · 内网直连") {
                scope.launch {
                    try {
                        val ports = store.client?.ports(s.id).orEmpty()
                        if (ports.isEmpty()) toast("未检测到监听端口")
                        else if (ports.size == 1) { openPort(context, store, ports[0].port); onDismiss() }
                        else portsDialog = ports
                    } catch (e: Exception) { toast("获取端口失败：${e.message}") }
                }
            }
            SheetItem("✏️", "重命名会话", if (s.resume_id != null) "当前为 AI 命名" else null) { renameDialog = true }
            if (!showMessagesMode) SheetItem("📋", "复制屏幕内容", null) {
                val text = attachment?.session?.emulator?.screen?.transcriptText?.trim()
                if (text.isNullOrBlank()) toast("屏幕为空") else { copyToClipboard(context, text); toast("已复制") }
                onDismiss()
            }
            SheetItem("±", "本次改动", "diff · 回滚") { onDismiss(); nav.navigate("diff/${s.id}") }
            SheetItem("📥", "任务收件箱", s.project_name) { onDismiss(); nav.navigate("inbox/${Uri.encode(s.project_path)}") }
            SheetItem("🔁", "重启 agent", "resume 同一会话") {
                scope.launch {
                    try {
                        val api = store.client ?: return@launch
                        runCatching { api.kill(s.id) }
                        val fresh = api.createSession(s.project_path, s.agent, resume = true)
                        onDismiss()
                        nav.navigate("session/${fresh.id}") { popUpTo("home") }
                    } catch (e: Exception) { toast("重启失败：${e.message}") }
                }
            }
            // 字号
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("Aa", color = Tok.Dim, fontSize = 14.sp)
                Spacer(Modifier.width(12.dp))
                Text("字号", color = Tok.Ink, fontSize = 15.sp, modifier = Modifier.weight(1f))
                TextButton(onClick = { scope.launch { store.settings.setFontSize(settings.fontSize - 1) } }) { Text("−", fontSize = 18.sp) }
                Text("${settings.fontSize}", color = Tok.Ink, fontFamily = FontFamily.Monospace)
                TextButton(onClick = { scope.launch { store.settings.setFontSize(settings.fontSize + 1) } }) { Text("＋", fontSize = 16.sp) }
            }
            if (s.state != "exited") SheetItem("⛔", "结束进程", "保留回放", danger = true) { killDialog = true }
            SheetItem("🗑", "删除会话记录", null, danger = true) { deleteDialog = true }
            TextButton(onClick = onDismiss, modifier = Modifier.fillMaxWidth()) { Text("取消", color = Tok.Dim) }
        }
    }

    if (renameDialog) {
        var title by remember { mutableStateOf(s.title) }
        AlertDialog(
            onDismissRequest = { renameDialog = false },
            containerColor = Tok.Raised,
            title = { Text("重命名会话", color = Tok.Ink) },
            text = { OutlinedTextField(title, { title = it }, singleLine = true) },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { store.client?.rename(s.id, title.trim()) }
                            .onFailure { toast("重命名失败：${it.message}") }
                        store.refreshSessions()
                    }
                    renameDialog = false; onDismiss()
                }) { Text("确定") }
            },
            dismissButton = { TextButton(onClick = { renameDialog = false }) { Text("取消", color = Tok.Dim) } },
        )
    }
    if (killDialog) {
        ConfirmDialog("结束进程？", "进程将被终止，屏幕回放保留。", "结束",
            onConfirm = {
                scope.launch { runCatching { store.client?.kill(s.id) }.onFailure { toast("失败：${it.message}") } }
                killDialog = false; onDismiss()
            }, onCancel = { killDialog = false })
    }
    if (deleteDialog) {
        ConfirmDialog("删除会话记录？", "删除会话与回放（进程若存活将先结束），不可恢复。", "删除",
            onConfirm = {
                scope.launch {
                    runCatching { store.client?.deleteSession(s.id) }.onFailure { toast("失败：${it.message}") }
                    store.refreshSessions()
                }
                deleteDialog = false; onDismiss()
                nav.popBackStack("home", inclusive = false)
            }, onCancel = { deleteDialog = false })
    }
    portsDialog?.let { ports ->
        AlertDialog(
            onDismissRequest = { portsDialog = null },
            containerColor = Tok.Raised,
            title = { Text("Web 预览", color = Tok.Ink) },
            text = {
                Column {
                    ports.forEach { p ->
                        Text(
                            ":${p.port}  ${p.cmd}", color = Tok.Cyan, fontFamily = FontFamily.Monospace,
                            modifier = Modifier.fillMaxWidth().clickable {
                                openPort(context, store, p.port); portsDialog = null; onDismiss()
                            }.padding(vertical = 8.dp),
                        )
                    }
                }
            },
            confirmButton = { TextButton(onClick = { portsDialog = null }) { Text("取消", color = Tok.Dim) } },
        )
    }
}

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

// ---------- v1.1 diff 屏 ----------

@Composable
fun DiffScreen(store: AppStore, nav: NavHostController, sessionId: String) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val sessions by store.sessions.collectAsState()
    val session = sessions.find { it.id == sessionId }
    var diff by remember { mutableStateOf<DiffResponse?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var expandedPath by remember { mutableStateOf<String?>(null) }
    var rollbackDialog by remember { mutableStateOf(false) }

    LaunchedEffect(sessionId) {
        try { diff = store.client?.diff(sessionId) }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持 diff（需 v1.1）" else e.message }
        catch (e: Exception) { error = e.message }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg)) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Column(Modifier.weight(1f)) {
                Text("本次改动", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
                diff?.base?.takeIf { it.isNotBlank() }?.let {
                    Text("基线 $it", color = Tok.Faint, fontSize = 10.sp, fontFamily = FontFamily.Monospace)
                }
            }
        }
        when {
            error != null -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { Text(error!!, color = Tok.Red, fontSize = 13.sp) }
            diff == null -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { Text("加载中…", color = Tok.Faint) }
            diff?.supported == false -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { Text("此会话不支持 diff（无 checkpoint）", color = Tok.Faint) }
            diff!!.files.isEmpty() -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { Text("会话开始以来无改动", color = Tok.Faint) }
            else -> LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
                items(diff!!.files, key = { it.path }) { f ->
                    val statusColor = when (f.status) { "added" -> Tok.Green; "deleted" -> Tok.Red; else -> Tok.Amber }
                    Card(
                        onClick = { expandedPath = if (expandedPath == f.path) null else f.path },
                        colors = CardDefaults.cardColors(containerColor = Tok.Surface),
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                    ) {
                        Column(Modifier.padding(12.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(
                                    when (f.status) { "added" -> "A"; "deleted" -> "D"; else -> "M" },
                                    color = statusColor, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold,
                                )
                                Spacer(Modifier.width(8.dp))
                                Text(f.path, color = Tok.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                                Text("+${f.additions}", color = Tok.Green, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                                Spacer(Modifier.width(6.dp))
                                Text("−${f.deletions}", color = Tok.Red, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                            }
                            if (expandedPath == f.path && f.patch.isNotBlank()) {
                                PatchView(f.patch, f.truncated)
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(70.dp)) }
            }
        }
        if (diff?.supported == true) {
            Button(
                onClick = { rollbackDialog = true },
                colors = androidx.compose.material3.ButtonDefaults.buttonColors(containerColor = Tok.Red.copy(alpha = 0.85f)),
                modifier = Modifier.fillMaxWidth().padding(14.dp).navigationBarsPadding(),
            ) { Text("回滚到会话开始", color = Color.White) }
        }
    }

    if (rollbackDialog) {
        val alive = session != null && session.state != "exited"
        ConfirmDialog(
            "回滚到会话开始？",
            buildString {
                append("工作区将恢复到本会话 start 检查点，start 之后新增的文件会被删除（.git 与忽略文件不动）。不可撤销。")
                if (alive) append("\n\n会话仍在运行：将先结束进程再回滚。")
            },
            "回滚",
            onConfirm = {
                rollbackDialog = false
                scope.launch {
                    try {
                        store.client?.rollback(sessionId, force = alive)
                        Toast.makeText(context, "已回滚", Toast.LENGTH_SHORT).show()
                        diff = store.client?.diff(sessionId)
                    } catch (e: Exception) {
                        Toast.makeText(context, "回滚失败：${e.message}", Toast.LENGTH_LONG).show()
                    }
                }
            },
            onCancel = { rollbackDialog = false },
        )
    }
}

@Composable
private fun PatchView(patch: String, truncated: Boolean) {
    Column(
        Modifier.fillMaxWidth().padding(top = 8.dp)
            .background(Tok.TermBg, RoundedCornerShape(8.dp))
            .horizontalScroll(rememberScrollState())
            .padding(8.dp),
    ) {
        patch.lineSequence().forEach { line ->
            val color = when {
                line.startsWith("+++") || line.startsWith("---") -> Tok.Dim
                line.startsWith("@@") -> Tok.Cyan
                line.startsWith("+") -> Tok.Green
                line.startsWith("-") -> Tok.Red
                else -> Tok.Dim
            }
            Text(line, color = color, fontSize = 11.sp, fontFamily = FontFamily.Monospace, maxLines = 1, softWrap = false)
        }
        if (truncated) Text("… patch 过大已截断（64KB）", color = Tok.Faint, fontSize = 11.sp, modifier = Modifier.padding(top = 4.dp))
    }
}

// ---------- helpers ----------

fun copyToClipboard(context: Context, text: String) {
    (context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
        .setPrimaryClip(ClipData.newPlainText("aaa-ui", text))
}

private fun openPort(context: Context, store: AppStore, port: Int) {
    val host = (store.connState.value as? ConnState.Connected)?.host?.substringBefore(':') ?: return
    context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("http://$host:$port")))
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
