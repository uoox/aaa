package cc.uoox.aaaui

import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

// ---------- A6 项目 ----------

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ProjectsTab(store: AppStore, nav: NavHostController) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val projects by store.projects.collectAsState()
    val health by store.health.collectAsState()
    var query by rememberSaveable { mutableStateOf("") }
    var selectMode by rememberSaveable { mutableStateOf(false) }
    var selected by remember { mutableStateOf(setOf<String>()) }
    var actionsFor by remember { mutableStateOf<Project?>(null) }
    var batchConfirm by remember { mutableStateOf(false) }
    var purgeReport by remember { mutableStateOf<List<ProjectDeleteResult>?>(null) }

    LaunchedEffect(Unit) { store.refreshProjects() }

    val filtered = remember(projects, query) {
        projects.filter {
            query.isBlank() || it.name.contains(query, true) || it.session_title.orEmpty().contains(query, true)
        }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("项目", color = Tok.Ink, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Text(
                "${projects.size} 个 · SSD ${if (health?.ssd_mounted != false) "✓" else "✗"}",
                color = if (health?.ssd_mounted != false) Tok.Dim else Tok.Red, fontSize = 12.sp,
            )
        }
        OutlinedTextField(
            query, { query = it },
            placeholder = { Text("搜索项目 / 会话命名…", color = Tok.Faint, fontSize = 13.sp) },
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), singleLine = true,
        )
        if (selectMode) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
                horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("已选 ${selected.size} 个", color = Tok.Cyan, fontSize = 13.sp)
                Row {
                    TextButton(onClick = { selectMode = false; selected = emptySet() }) { Text("取消", color = Tok.Dim) }
                    Button(
                        enabled = selected.isNotEmpty(),
                        colors = ButtonDefaults.buttonColors(containerColor = Tok.Red.copy(alpha = 0.85f)),
                        onClick = { batchConfirm = true },
                    ) { Text("批量删除") }
                }
            }
        }
        LazyColumn(Modifier.fillMaxSize()) {
            items(filtered, key = { it.path }) { p ->
                val isSel = p.path in selected
                Card(
                    colors = CardDefaults.cardColors(containerColor = Tok.Surface),
                    border = if (isSel) androidx.compose.foundation.BorderStroke(1.dp, Tok.Cyan) else null,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                ) {
                    Column(
                        Modifier.combinedClickable(
                            onClick = {
                                if (selectMode) selected = if (isSel) selected - p.path else selected + p.path
                                else actionsFor = p
                            },
                            onLongClick = { selectMode = true; selected = selected + p.path },
                        ).padding(13.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            if (selectMode) {
                                Text(if (isSel) "☑" else "☐", color = if (isSel) Tok.Cyan else Tok.Faint, fontSize = 16.sp)
                                Spacer(Modifier.width(8.dp))
                            }
                            Text(p.name, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                            p.agent?.let { AgentChip(it) }
                        }
                        p.session_title?.takeIf { it.isNotBlank() }?.let {
                            Text("「$it」", color = Tok.Dim, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 2.dp))
                        }
                        Text(
                            "上下文 ${humanBytes(p.ctx_size)} · 目录 ${humanBytes(p.dir_size)} · ${relativeTime(p.mtime)}",
                            color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(top = 3.dp),
                        )
                    }
                }
            }
            item {
                Text(
                    "长按进入多选 · 批量删除与 CLI 的 a / d 键同义",
                    color = Tok.Faint, fontSize = 11.sp,
                    modifier = Modifier.fillMaxWidth().padding(14.dp),
                )
                Spacer(Modifier.height(60.dp))
            }
        }
    }

    actionsFor?.let { p ->
        ProjectActionsSheet(store, nav, p, onDismiss = { actionsFor = null }, onPurged = { purgeReport = it })
    }

    if (batchConfirm) {
        val totalSize = projects.filter { it.path in selected }.sumOf { it.dir_size }
        ConfirmDialog(
            "删除 ${selected.size} 个项目？",
            "将删除目录（共 ${humanBytes(totalSize)}）并清除所有 agent 的会话存储。与 aaa CLI 的 d 行为一致，不可恢复。",
            "删除",
            onConfirm = {
                batchConfirm = false
                scope.launch {
                    try {
                        val results = store.client?.deleteProjects(selected.toList()).orEmpty()
                        purgeReport = results
                        selectMode = false; selected = emptySet()
                        store.refreshProjects()
                    } catch (e: Exception) { Toast.makeText(context, "删除失败：${e.message}", Toast.LENGTH_LONG).show() }
                }
            },
            onCancel = { batchConfirm = false },
        )
    }

    purgeReport?.let { results -> PurgeReportDialog(results) { purgeReport = null } }
}

/** A8 删除结果（purge 报告渲染） */
@Composable
fun PurgeReportDialog(results: List<ProjectDeleteResult>, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Tok.Raised,
        title = { Text("删除完成", color = Tok.Ink) },
        text = {
            Column {
                results.forEach { r ->
                    Text(
                        (if (r.ok) "✓ " else "✗ ") + r.path.substringAfterLast('/'),
                        color = if (r.ok) Tok.Green else Tok.Red, fontSize = 14.sp, fontWeight = FontWeight.Bold,
                    )
                    val purged = r.purged.filter { it.count > 0 }
                    Text(
                        if (purged.isEmpty()) "无会话存储残留" else purged.joinToString(" · ") { "${it.agent_label} ${it.count} 条" },
                        color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
                        modifier = Modifier.padding(bottom = 6.dp),
                    )
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("好") } },
    )
}

// ---------- A7 项目操作 ----------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ProjectActionsSheet(
    store: AppStore,
    nav: NavHostController,
    p: Project,
    onDismiss: () -> Unit,
    onPurged: (List<ProjectDeleteResult>) -> Unit,
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val openSession = LocalOpenSession.current
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    var agentPicker by remember { mutableStateOf<String?>(null) } // "open" | "default"
    var deleteConfirm by remember { mutableStateOf(false) }
    var newSheet by remember { mutableStateOf(false) }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
    val muted = p.path in settings.mutedProjects

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = Tok.Surface) {
        Column(Modifier.padding(bottom = 20.dp)) {
            Column(Modifier.padding(horizontal = 18.dp, vertical = 4.dp)) {
                Text(p.name, color = Tok.Ink, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                Text(
                    "${p.agent ?: "无默认 agent"} · ${p.path} · ${humanBytes(p.dir_size)}",
                    color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                )
            }
            SheetItem("▶", "继续会话", p.agent?.let { "${it} resume" } ?: "无默认 agent") {
                val agent = p.agent
                if (agent == null) { toast("未设置默认 agent"); return@SheetItem }
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, agent, resume = true) ?: return@launch
                        onDismiss(); openSession(sess.id, "")
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
            }
            SheetItem("✳", "用其它 agent 打开…", null) { newSheet = true }
            SheetItem("＞", "在此目录开终端", "zsh") {
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, "shell", resume = false) ?: return@launch
                        onDismiss(); openSession(sess.id, "")
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
            }
            SheetItem("📥", "任务收件箱", "agent 等待输入时自动喂入") {
                onDismiss(); nav.navigate("inbox/${Uri.encode(p.path)}")
            }
            SheetItem("⚙", "更换默认 agent", "当前 ${p.agent ?: "无"}") { agentPicker = "default" }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text("🔕", fontSize = 14.sp, modifier = Modifier.width(28.dp))
                Text("静音此项目通知", color = Tok.Ink, fontSize = 15.sp, modifier = Modifier.weight(1f))
                Switch(muted, { v -> scope.launch { store.settings.setProjectMuted(p.path, v) } })
            }
            SheetItem("🗑", "删除项目…", "目录 + 全部会话", danger = true) { deleteConfirm = true }
            TextButton(onClick = onDismiss, modifier = Modifier.fillMaxWidth()) { Text("取消", color = Tok.Dim) }
        }
    }

    if (newSheet) {
        NewSessionSheet(store, nav, initialPath = p.path) { newSheet = false; onDismiss() }
    }

    if (agentPicker != null) {
        AlertDialog(
            onDismissRequest = { agentPicker = null },
            containerColor = Tok.Raised,
            title = { Text("更换默认 agent", color = Tok.Ink) },
            text = {
                Column {
                    listOf("claude", "codex", "pi", "reasonix", "agy").forEach { ag ->
                        Row(
                            Modifier.fillMaxWidth().clickable {
                                agentPicker = null
                                scope.launch {
                                    runCatching { store.client?.setProjectAgent(p.path, ag) }
                                        .onSuccess { toast("已设为 $ag"); store.refreshProjects() }
                                        .onFailure { toast("失败：${it.message}") }
                                }
                            }.padding(vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            AgentChip(ag)
                            Spacer(Modifier.width(10.dp))
                            Text(Tok.agentLabel(ag), color = if (p.agent == ag) Tok.Cyan else Tok.Ink)
                        }
                    }
                }
            },
            confirmButton = { TextButton(onClick = { agentPicker = null }) { Text("取消", color = Tok.Dim) } },
        )
    }

    if (deleteConfirm) {
        // A8 删除确认
        ConfirmDialog(
            "删除项目 ${p.name}？",
            "将删除目录（${humanBytes(p.dir_size)}）并清除所有 agent 的会话存储。与 aaa CLI 的 d 行为一致，不可恢复。",
            "删除",
            onConfirm = {
                deleteConfirm = false
                scope.launch {
                    try {
                        val results = store.client?.deleteProjects(listOf(p.path)).orEmpty()
                        onDismiss()
                        onPurged(results)
                        store.refreshProjects()
                    } catch (e: Exception) { toast("删除失败：${e.message}") }
                }
            },
            onCancel = { deleteConfirm = false },
        )
    }
}

// ---------- v1.1 任务收件箱 ----------

@Composable
fun InboxScreen(store: AppStore, nav: NavHostController, projectPath: String) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var items by remember { mutableStateOf<List<InboxItem>>(emptyList()) }
    var input by rememberSaveable { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }

    suspend fun reload() {
        try { items = store.client?.inbox(projectPath).orEmpty(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持收件箱（需 v1.1）" else e.message }
        catch (e: Exception) { error = e.message }
    }
    LaunchedEffect(projectPath) { reload() }
    LaunchedEffect(projectPath) {
        store.frames.collectLatest { if (it is EventFrame.InboxChanged && it.path == projectPath) reload() }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Column {
                Text("任务收件箱", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
                Text(projectPath.substringAfterLast('/'), color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
            }
        }
        Text(
            "排队的任务会在 agent 下次等待输入时自动喂入（拼成一条任务清单消息）。",
            color = Tok.Dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }

        LazyColumn(Modifier.weight(1f).fillMaxWidth().padding(top = 6.dp)) {
            items(items, key = { it.id }) { item ->
                Card(
                    colors = CardDefaults.cardColors(containerColor = Tok.Surface),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp),
                ) {
                    Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(item.text, color = Tok.Ink, fontSize = 14.sp)
                            Text(relativeTime(item.created_at), color = Tok.Faint, fontSize = 11.sp)
                        }
                        Text(
                            "✕", color = Tok.Faint, fontSize = 16.sp,
                            modifier = Modifier.clickable {
                                scope.launch {
                                    runCatching { store.client?.inboxDelete(item.id) }
                                        .onFailure { Toast.makeText(context, "删除失败：${it.message}", Toast.LENGTH_SHORT).show() }
                                    reload()
                                }
                            }.padding(8.dp),
                        )
                    }
                }
            }
            if (items.isEmpty() && error == null) {
                item { Text("收件箱为空", color = Tok.Faint, modifier = Modifier.fillMaxWidth().padding(24.dp)) }
            }
        }

        Row(
            Modifier.fillMaxWidth().background(Tok.Surface).padding(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                input, { input = it },
                placeholder = { Text("添加任务…", color = Tok.Faint, fontSize = 13.sp) },
                modifier = Modifier.weight(1f), maxLines = 3,
            )
            Spacer(Modifier.width(8.dp))
            Button(
                enabled = input.isNotBlank(),
                onClick = {
                    val text = input.trim(); input = ""
                    scope.launch {
                        runCatching { store.client?.inboxAdd(projectPath, text) }
                            .onFailure { Toast.makeText(context, "添加失败：${it.message}", Toast.LENGTH_SHORT).show() }
                        reload()
                    }
                },
            ) { Text("添加") }
        }
    }
}
