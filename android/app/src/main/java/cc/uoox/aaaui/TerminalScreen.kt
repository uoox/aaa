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
import androidx.compose.material3.Button
import androidx.compose.material3.DrawerDefaults
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.launch

fun terminalTabLabel(index: Int, s: Session, root: String): String =
    // 尾斜杠要先去掉再比、再取叶子名（v1.22 补）：`/a/b/` 与 `/a/b` 是同一个目录，
    // 而 `"/a/b/".substringAfterLast('/')` 是空串——手机上就多出一截「终端 1 · 」的尾巴。
    // mac 侧 `ui/mod.rs` 的标签一直是 trim 过的。
    ("终端 " + (index + 1)).let { base ->
        val path = s.project_path.trimEnd('/')
        val leaf = path.substringAfterLast('/')
        if (path.isNotEmpty() && path != root.trimEnd('/') && leaf.isNotEmpty()) "$base · $leaf" else base
    }

/**
 * 一个终端一屏（2026-09-08 用户拍板：终端列表搬到项目面板里，和会话平级，这里就不再需要
 * 自己的标签条了）。`focusId` 指名要看哪一个；老的 `?focus=` 空参数还兼容——回落到最新的
 * 那个终端，一个都没有就给一个「开一个」。
 */
@Composable
fun TerminalScreen(store: AppStore, nav: NavHostController, focusId: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sessions by store.sessions.collectAsState()
    val conn by store.connState.collectAsState()
    val terminals = terminalSessions(sessions)
    val root = store.health.value?.project_root?.takeIf { it.isNotBlank() } ?: "/Volumes/SSD/project"
    var creating by remember { mutableStateOf(false) }
    val index = terminals.indexOfFirst { it.id == focusId }
    val current = terminals.getOrNull(index) ?: terminals.lastOrNull()
    val label = current?.let { terminalTabLabel(if (index >= 0) index else terminals.lastIndex, it, root) } ?: "终端"

    fun createTerminal() {
        if (creating) return
        creating = true
        scope.launch {
            try {
                val sess = store.client?.createSession(root, "shell", resume = false, fresh = true) ?: return@launch
                store.refreshSessions()
                store.prewarmAttachment(sess.id)
                nav.openTerminal(sess.id)
            } finally { creating = false }
        }
    }

    // 左上角是 ☰ 不是「返回」（2026-09-08 用户拍板）：终端和会话平级，两屏的左上角就该
    // 是同一个东西——拉出项目面板，从这里直接去任何一个项目或另一个终端。
    val drawerState = rememberDrawerState(DrawerValue.Closed)
    ModalNavigationDrawer(
        drawerState = drawerState,
        drawerContent = {
            // 与会话屏同一套：小屏铺满、大屏半屏
            val frac = sidebarFraction()
            ModalDrawerSheet(
                modifier = Modifier.fillMaxWidth(frac),
                drawerShape = if (frac >= 1f) RectangleShape else DrawerDefaults.shape,
                drawerContainerColor = Tok.Surface, drawerContentColor = Tok.Ink,
            ) {
                ProjectPanel(store, nav, currentPath = current?.project_path, onBeforeNavigate = { scope.launch { drawerState.close() } })
            }
        },
    ) {
        Column(Modifier.fillMaxSize().background(Tok.Bg)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "☰", color = Tok.Dim, fontSize = 20.sp,
                    modifier = Modifier.clickable { scope.launch { drawerState.open() } }.padding(horizontal = 8.dp, vertical = 2.dp),
                )
                Text(label, color = Tok.Ink, fontSize = 17.sp, maxLines = 1, modifier = Modifier.weight(1f))
                // 关掉这个终端：列表里立刻消失，kill + DELETE 在后台跑（AppStore.closeTerminal）
                if (current != null) TextButton(onClick = { store.closeTerminal(current.id); nav.popBackStack() }) {
                    Text("关闭", color = Tok.Faint, fontSize = 13.sp)
                }
            }
            if (current == null) Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                Column(horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("还没有终端", color = Tok.Faint)
                    Button(onClick = { createTerminal() }, modifier = Modifier.padding(top = 12.dp)) { Text("开一个") }
                }
            } else TerminalPane(store, context, conn, current.id, Modifier.weight(1f))
        }
    }
}

@Composable
private fun TerminalPane(store: AppStore, context: Context, conn: ConnState, sessionId: String, modifier: Modifier) {
    val scope = rememberCoroutineScope()
    val ctrlStickyState = remember { mutableStateOf(false) }
    var ctrlSticky by ctrlStickyState
    val inputRef = remember { mutableStateOf<TermInputView?>(null) }
    val selectMode = remember { mutableStateOf(false) }
    // attach 不随本屏销毁（2026-09-08）：终端就那么几个，socket 便宜，而每次重建都是
    // 一个新的 libvterm 模拟器 + 一次整屏 replay——「进终端很卡」的另一半。真正收掉的时机
    // 是关闭这个终端（AppStore.closeTerminal）或它自己退出（cleanupExitedShell）。
    var attachment by remember(sessionId) { mutableStateOf<TerminalAttachment?>(store.peekAttachment(sessionId)) }
    LaunchedEffect(conn, sessionId) { attachment = store.attachmentFor(sessionId) }
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
