package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController

/**
 * 历史：daemon 的会话日志——所有出现过的会话，含已退出、已删除，最新在前。
 * 会话还在（活着或有回放）点一行就打开；已删除的只能看。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    val sessions by store.sessions.collectAsState()
    var entries by remember { mutableStateOf<List<HistoryEntry>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        try { entries = store.client?.history().orEmpty(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持会话日志（需 v1.9）" else e.message }
        catch (e: Exception) { error = e.message }
    }
    val alive = remember(sessions) { sessions.map { it.id }.toSet() }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("历史", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            entries?.let { Text("${it.size} 条", color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }
        val list = entries
        when {
            list == null && error == null -> Text("加载中…", color = Tok.Faint, modifier = Modifier.padding(16.dp))
            list != null && list.isEmpty() -> Text("还没有记录", color = Tok.Faint, modifier = Modifier.padding(16.dp))
            list != null -> LazyColumn(Modifier.fillMaxSize()) {
                items(list, key = { it.id }) { e ->
                    val openable = e.id in alive
                    val (status, color) = when {
                        e.deleted_at != null -> "已删除" to Tok.Faint
                        e.last_state == "running" -> "执行中" to Tok.Green
                        e.last_state == "waiting" -> "已激活" to Tok.Accent
                        else -> "已退出" to Tok.Dim
                    }
                    val items = parseChecklist(e.summary)
                    val meta = buildString {
                        append(e.project_name)
                        if (e.agent == "shell") append(" · 终端")
                        append(" · "); append(artifactTimeLabel(e.created_at))
                        e.ended_at?.let { append(" → "); append(artifactTimeLabel(it)) }
                        if (items.isNotEmpty()) { append(" · "); append(checklistProgress(items)) }
                    }
                    Column(
                        Modifier.fillMaxWidth()
                            .then(if (openable) Modifier.clickable { openSession(e.id, "") } else Modifier)
                            .padding(horizontal = 16.dp, vertical = 9.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(status, color = color, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.width(48.dp))
                            Spacer(Modifier.width(6.dp))
                            Text(
                                e.title.ifBlank { e.project_name }, color = if (e.deleted_at != null) Tok.Dim else Tok.Ink,
                                fontSize = 14.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                        }
                        Text(meta, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 54.dp, top = 2.dp))
                    }
                    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 70.dp))
                }
            }
        }
    }
}
