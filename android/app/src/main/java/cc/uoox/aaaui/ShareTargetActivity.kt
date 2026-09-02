package cc.uoox.aaaui

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch

/** 系统分享目标：图片/文件 → 选项目 → POST /projects/upload → 可跳到该项目会话并预填路径。 */
class ShareTargetActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val store = AppStore.get(this)
        store.ensureStarted()
        @Suppress("DEPRECATION")
        val uri: Uri? = if (intent?.action == Intent.ACTION_SEND) intent.getParcelableExtra(Intent.EXTRA_STREAM) else null
        val sharedText: String? = if (intent?.action == Intent.ACTION_SEND) intent.getStringExtra(Intent.EXTRA_TEXT) else null
        setContent { AaaTheme(store) { ShareScreen(store, uri, sharedText) { finish() } } }
    }
}

@Composable
private fun ShareScreen(store: AppStore, uri: Uri?, sharedText: String?, onDone: () -> Unit) {
    val scope = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current
    val projects by store.projects.collectAsState()
    val sessions by store.sessions.collectAsState()
    var phase by remember { mutableStateOf("pick") } // pick | uploading | done | error
    var savedPath by remember { mutableStateOf("") }
    var targetProject by remember { mutableStateOf<Project?>(null) }
    var error by remember { mutableStateOf("") }

    LaunchedEffect(Unit) { store.refreshProjects(); store.refreshSessions() }

    Column(Modifier.fillMaxSize().background(Tok.Bg).padding(16.dp)) {
        Text("发送到项目", color = Tok.Ink, fontSize = 20.sp, fontWeight = FontWeight.Bold)
        Text(
            uri?.lastPathSegment ?: sharedText?.take(60) ?: "无内容",
            color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
            modifier = Modifier.padding(vertical = 4.dp),
        )
        Spacer(Modifier.height(8.dp))

        when (phase) {
            "pick" -> {
                if (uri == null && sharedText == null) {
                    Text("分享内容为空", color = Tok.Red); return@Column
                }
                if (projects.isEmpty()) Text("加载项目…（需已连接 daemon）", color = Tok.Faint, fontSize = 13.sp)
                LazyColumn(Modifier.weight(1f)) {
                    items(projects, key = { it.path }) { p ->
                        Card(
                            onClick = {
                                phase = "uploading"; targetProject = p
                                scope.launch {
                                    try {
                                        val api = store.client ?: throw IllegalStateException("未连接 daemon")
                                        val (name, bytes) = if (uri != null) readUri(context, uri)
                                        else ("note.txt" to sharedText!!.toByteArray())
                                        savedPath = api.upload(p.path, name, bytes).saved_path
                                        phase = "done"
                                    } catch (e: Exception) { error = e.message.orEmpty(); phase = "error" }
                                }
                            },
                            colors = CardDefaults.cardColors(containerColor = Tok.Surface),
                            modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
                        ) {
                            Row(Modifier.padding(13.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(p.name, color = Tok.Ink, fontSize = 15.sp, modifier = Modifier.weight(1f))
                            }
                        }
                    }
                }
            }
            "uploading" -> Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
            "error" -> Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                Text("上传失败：$error", color = Tok.Red, fontSize = 14.sp)
                Spacer(Modifier.height(12.dp))
                OutlinedButton(onClick = { phase = "pick" }) { Text("重试") }
            }
            "done" -> Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                Text("✓ 已上传", color = Tok.Green, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                Text(savedPath, color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(vertical = 8.dp))
                val liveSession = sessions.filter { it.project_path == targetProject?.path && it.state != "exited" }
                    .maxByOrNull { it.last_output_at }
                Spacer(Modifier.height(10.dp))
                if (liveSession != null) {
                    Button(
                        onClick = {
                            context.startActivity(
                                Intent(context, MainActivity::class.java)
                                    .putExtra(MainActivity.EXTRA_SESSION_ID, liveSession.id)
                                    .putExtra(MainActivity.EXTRA_PREFILL, savedPath)
                                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                            )
                            onDone()
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) { Text("打开会话并把路径填入 composer") }
                    Spacer(Modifier.height(8.dp))
                } else {
                    Text("该项目当前没有活动会话", color = Tok.Faint, fontSize = 12.sp)
                    Spacer(Modifier.height(8.dp))
                }
                OutlinedButton(onClick = onDone, modifier = Modifier.fillMaxWidth()) { Text("完成") }
            }
        }
    }
}
