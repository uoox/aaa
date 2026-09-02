package cc.uoox.aaaui

import android.content.Context
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.inputmethod.InputMethodManager
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
import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient
import com.termux.view.TerminalView
import com.termux.view.TerminalViewClient
import kotlinx.coroutines.launch

fun terminalTabLabel(index: Int, s: Session, root: String): String =
    "终端 " + (index + 1) + if (s.project_path != root) " · " + s.project_path.substringAfterLast('/') else ""

@Composable
fun TerminalScreen(store: AppStore, nav: NavHostController, focusId: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sessions by store.sessions.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
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
        } else TerminalPane(store, context, conn, effectiveId, settings.fontSize, Modifier.weight(1f))
    }
}

@Composable
private fun TerminalPane(store: AppStore, context: Context, conn: ConnState, sessionId: String, fontSize: Int, modifier: Modifier) {
    val viewRef = remember { mutableStateOf<TerminalView?>(null) }
    var ctrlSticky by remember { mutableStateOf(false) }
    val terminalClient = remember(sessionId) { object : TerminalSessionClient {
        override fun onTextChanged(changedSession: TerminalSession) { viewRef.value?.onScreenUpdated() }
        override fun onTitleChanged(changedSession: TerminalSession) {}
        override fun onSessionFinished(finishedSession: TerminalSession) {}
        override fun onCopyTextToClipboard(session: TerminalSession, text: String) { copyToClipboard(context, text) }
        override fun onPasteTextFromClipboard(session: TerminalSession?) { pasteIntoPty(context, session) }
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
    } }
    var attachment by remember(sessionId) { mutableStateOf<TerminalAttachment?>(null) }
    LaunchedEffect(conn, sessionId) { attachment = store.attachmentFor(sessionId, terminalClient) }
    DisposableEffect(sessionId) { onDispose { store.releaseAttachmentSoon(sessionId) } }
    Column(modifier) {
        TerminalHost(attachment, viewRef, fontSize) { view -> object : TerminalViewClient {
            override fun onScale(scale: Float) = scale
            override fun onSingleTapUp(e: MotionEvent?) { view.requestFocus(); (context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager).showSoftInput(view, InputMethodManager.SHOW_IMPLICIT) }
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
            override fun onCodePoint(codePoint: Int, ctrlDown: Boolean, session: TerminalSession?): Boolean { if (ctrlSticky) view.post { ctrlSticky = false }; return false }
            override fun onEmulatorSet() {}
            override fun logError(tag: String?, message: String?) {}
            override fun logWarn(tag: String?, message: String?) {}
            override fun logInfo(tag: String?, message: String?) {}
            override fun logDebug(tag: String?, message: String?) {}
            override fun logVerbose(tag: String?, message: String?) {}
            override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) {}
            override fun logStackTrace(tag: String?, e: Exception?) {}
        } }
        Row(Modifier.fillMaxWidth().background(Tok.Surface).horizontalScroll(rememberScrollState()).padding(8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            fun key(code: Int) = { viewRef.value?.handleKeyCode(code, 0); Unit }
            fun lit(s: String) = { attachment?.session?.write(s); Unit }
            KeyChip("Esc", onClick = key(KeyEvent.KEYCODE_ESCAPE)); KeyChip("Tab", onClick = key(KeyEvent.KEYCODE_TAB)); KeyChip("Ctrl", active = ctrlSticky) { ctrlSticky = !ctrlSticky }
            KeyChip("↑", onClick = key(KeyEvent.KEYCODE_DPAD_UP)); KeyChip("↓", onClick = key(KeyEvent.KEYCODE_DPAD_DOWN)); KeyChip("←", onClick = key(KeyEvent.KEYCODE_DPAD_LEFT)); KeyChip("→", onClick = key(KeyEvent.KEYCODE_DPAD_RIGHT))
            KeyChip("Home", onClick = key(KeyEvent.KEYCODE_MOVE_HOME)); KeyChip("End", onClick = key(KeyEvent.KEYCODE_MOVE_END)); KeyChip("⏎", onClick = key(KeyEvent.KEYCODE_ENTER)); KeyChip("-", onClick = lit("-")); KeyChip("/", onClick = lit("/")); KeyChip("|", onClick = lit("|")); KeyChip("~", onClick = lit("~")); KeyChip("粘贴") { pasteIntoPty(context, attachment?.session) }
        }
    }
}
