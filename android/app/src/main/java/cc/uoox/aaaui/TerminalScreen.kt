package cc.uoox.aaaui

import android.content.Context
import android.view.KeyEvent
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.launch

fun terminalTabLabel(index: Int, s: Session, root: String): String =
    "终端 " + (index + 1) + if (s.project_path != root) " · " + s.project_path.substringAfterLast('/') else ""

@Composable
fun TerminalScreen(store: AppStore, nav: NavHostController, focusId: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sessions by store.sessions.collectAsState()
    val conn by store.connState.collectAsState()
    val tabs = terminalSessions(sessions)
    val root = store.health.value?.project_root ?: "/Volumes/SSD/project"
    var activeId by rememberSaveable { mutableStateOf(focusId) }
    val effectiveId = if (tabs.any { it.id == activeId }) activeId else tabs.lastOrNull()?.id.orEmpty()
    LaunchedEffect(tabs.map { it.id }) { if (activeId != effectiveId) activeId = effectiveId }
    fun createTerminal() = scope.launch { runCatching { store.client?.createSession(root, "shell", resume = false, fresh = true) }.getOrNull()?.let { activeId = it.id } }
    Column(Modifier.fillMaxSize().background(Tok.Bg)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 28.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("终端", color = Tok.Ink, fontSize = 18.sp, modifier = Modifier.weight(1f))
            TextButton(onClick = { createTerminal() }) { Text("+", color = Tok.Accent, fontSize = 22.sp) }
        }
        Row(Modifier.fillMaxWidth().background(Tok.Surface).horizontalScroll(rememberScrollState()).padding(8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            tabs.forEachIndexed { index, session ->
                val active = session.id == effectiveId
                Row(Modifier.background(if (active) Tok.Raised else Tok.Surface, RoundedCornerShape(7.dp)).clickable { activeId = session.id }.padding(start = 11.dp, end = if (active) 4.dp else 11.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(terminalTabLabel(index, session, root), color = if (active) Tok.Ink else Tok.Dim, fontSize = 13.sp)
                    if (active) Text("×", color = Tok.Dim, fontSize = 16.sp, modifier = Modifier.padding(start = 8.dp).clickable {
                        scope.launch { runCatching { store.client?.kill(session.id) }; runCatching { store.client?.deleteSession(session.id) }; if (activeId == session.id) activeId = tabs.getOrNull((index - 1).coerceAtLeast(0))?.id.orEmpty() }
                    })
                }
            }
        }
        if (effectiveId.isEmpty()) Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) { Text("还没有终端", color = Tok.Faint); Button(onClick = { createTerminal() }, modifier = Modifier.padding(top = 12.dp)) { Text("开一个") } }
        } else TerminalPane(store, context, conn, effectiveId, Modifier.weight(1f))
    }
}

@Composable
private fun TerminalPane(store: AppStore, context: Context, conn: ConnState, sessionId: String, modifier: Modifier) {
    val scope = rememberCoroutineScope()
    val ctrlStickyState = remember { mutableStateOf(false) }
    var ctrlSticky by ctrlStickyState
    val inputRef = remember { mutableStateOf<TermInputView?>(null) }
    val selectMode = remember { mutableStateOf(false) }
    var attachment by remember(sessionId) { mutableStateOf<TerminalAttachment?>(null) }
    LaunchedEffect(conn, sessionId) { attachment = store.attachmentFor(sessionId) }
    DisposableEffect(sessionId) { onDispose { store.releaseAttachmentSoon(sessionId) } }
    fun pasteViaDaemon() {
        val clip = (context.getSystemService(Context.CLIPBOARD_SERVICE) as android.content.ClipboardManager).primaryClip
        val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
        if (!text.isNullOrEmpty()) scope.launch { runCatching { store.client?.input(sessionId, text, false) } }
    }
    Column(modifier) {
        // 快捷键条在终端上方（回车排最前，条会横向滚）
        Row(Modifier.fillMaxWidth().background(Tok.Surface).horizontalScroll(rememberScrollState()).padding(8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            fun key(code: Int) = {
                vtermKeyFor(code)?.let { k -> attachment?.emulator?.dispatchKey(if (ctrlSticky) VTERM_MOD_CTRL else 0, k); ctrlSticky = false }
                Unit
            }
            fun lit(s: String) = { attachment?.write(s); Unit }
            KeyChip("键盘") { inputRef.value?.showKeyboard() }
            KeyChip("⏎", onClick = key(KeyEvent.KEYCODE_ENTER))
            KeyChip("选择", active = selectMode.value) { selectMode.value = !selectMode.value }
            KeyChip("Esc", onClick = key(KeyEvent.KEYCODE_ESCAPE)); KeyChip("Ctrl", active = ctrlSticky) { ctrlSticky = !ctrlSticky }
            KeyChip("↑", onClick = key(KeyEvent.KEYCODE_DPAD_UP)); KeyChip("↓", onClick = key(KeyEvent.KEYCODE_DPAD_DOWN)); KeyChip("←", onClick = key(KeyEvent.KEYCODE_DPAD_LEFT)); KeyChip("→", onClick = key(KeyEvent.KEYCODE_DPAD_RIGHT))
            KeyChip("Home", onClick = key(KeyEvent.KEYCODE_MOVE_HOME)); KeyChip("End", onClick = key(KeyEvent.KEYCODE_MOVE_END)); KeyChip("/", onClick = lit("/")); KeyChip("粘贴") { pasteViaDaemon() }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
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
}
