package cc.uoox.aaaui

import android.net.Uri
import android.widget.Toast
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
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
import androidx.compose.material3.Card
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

// ---------- A6 首页 = 项目列表 ----------

/**
 * 项目行的三态。一个项目只有一个 agent（建项目时定死，从不切换），项目 ↔ 会话
 * 事实上一对一，所以会话状态直接挂在项目行上，首页不再单开会话页。全部由客户端
 * 把 projects × sessions 两个流拼出来，daemon 不用改。
 *
 * 枚举顺序就是首页分组顺序。
 */
enum class ProjectState(val label: String) {
    NEEDS_REPLY("待回复"),
    RUNNING("执行中"),
    DONE("已完成"),
    NEVER("未开始"),
}

/** 首页一行要的全部东西，纯数据，方便单测。 */
data class ProjectRow(
    val project: Project,
    /** 该项目的主会话；一个都没有时为 null。 */
    val primary: Session?,
    val state: ProjectState,
    /** 第二行那一句话。 */
    val summary: String,
) {
    /** 组内排序键：有会话按最近输出，没有按目录 mtime；都是 daemon 给的 ISO 时间串，字典序即时间序。 */
    val sortKey: String get() = primary?.last_output_at?.takeIf { it.isNotBlank() } ?: project.mtime
    /** 行尾的相对时间用同一个来源。 */
    val timeIso: String get() = sortKey
}

/** 用户拍板口径：待回复 = waiting 且弹出了问题/选项；停在输入框的 waiting 不算。 */
fun Session.needsReply(): Boolean = state == "waiting" && question != null

/**
 * 主会话：优先活着的（非 exited），待回复 < 执行中 < 其它，同级按最近输出；
 * 全都退出了就取最近退出的那个。
 */
fun primarySessionFor(project: Project, sessions: List<Session>): Session? {
    val all = sessions.filter { it.project_path == project.path }
    if (all.isEmpty()) return null
    // 项目的 agent 建项目时就定了；同目录里另开的终端（shell）不能替它代表项目状态，
    // 只有项目 agent 的会话一个都没有时才退到其它会话
    val mine = all.filter { it.agent == project.agent }.ifEmpty { all }
    val live = mine.filter { it.state != "exited" }
    if (live.isNotEmpty()) {
        return live.minWithOrNull(
            compareBy<Session> { when { it.needsReply() -> 0; it.state == "running" -> 1; else -> 2 } }
                .thenByDescending { it.last_output_at },
        )
    }
    return mine.maxByOrNull { it.last_output_at }
}

/**
 * 三态归类。没有会话但有 session_title（daemon 从 agent 存储里读出的对话名）说明
 * 这个项目做过事、对话还在，点进去就是 resume——归「已完成」而不是「未开始」；
 * daemon 重启后 exited 会话从列表里消失，实测 13 个项目里 12 个都处于这种状态，
 * 若一律标「未开始」首页就全是灰点。真正一次都没跑过的才是「未开始」。
 */
fun projectStateOf(project: Project, primary: Session?): ProjectState = when {
    primary == null -> if (project.session_title.isNullOrBlank()) ProjectState.NEVER else ProjectState.DONE
    primary.needsReply() -> ProjectState.NEEDS_REPLY
    primary.state == "running" -> ProjectState.RUNNING
    else -> ProjectState.DONE
}

/**
 * 一句话摘要。running 取 preview 最后一行「有字」的——TUI 底部常是一整行边框或
 * 提示符，没有字母/数字/汉字的行跳过，否则摘要永远是一串横线。
 */
fun projectSummary(project: Project, primary: Session?, state: ProjectState): String = when (state) {
    ProjectState.NEEDS_REPLY -> primary?.question?.text?.trim().orEmpty().ifBlank { "等待回复" }
    // 不用 preview：它是屏幕末 4 行，TUI 型 agent 那里永远是输入框和底栏（"bypass permissions on…"），
    // 当摘要只会是垃圾。色点 + 分组标题已经说明「执行中」，这行给标题。
    ProjectState.RUNNING -> primary?.title?.takeIf { it.isNotBlank() }
        ?: project.session_title?.takeIf { it.isNotBlank() }
        ?: "运行中"
    ProjectState.DONE -> project.session_title?.takeIf { it.isNotBlank() }
        ?: primary?.title?.takeIf { it.isNotBlank() }
        ?: "点击继续"
    ProjectState.NEVER -> "未开始 · 点击启动"
}

fun projectRows(projects: List<Project>, sessions: List<Session>): List<ProjectRow> = projects.map { p ->
    val primary = primarySessionFor(p, sessions)
    val state = projectStateOf(p, primary)
    ProjectRow(p, primary, state, projectSummary(p, primary, state))
}

/** 按 [ProjectState] 顺序分组，组内最近的在前，空组不出现。 */
fun groupProjectRows(rows: List<ProjectRow>): List<Pair<ProjectState, List<ProjectRow>>> =
    ProjectState.entries
        .map { st -> st to rows.filter { it.state == st }.sortedByDescending { it.sortKey } }
        .filter { it.second.isNotEmpty() }

private fun ProjectState.dotColor(): Color = when (this) {
    ProjectState.NEEDS_REPLY -> Tok.Amber
    ProjectState.RUNNING -> Tok.Green
    ProjectState.DONE -> Tok.Faint
    ProjectState.NEVER -> Tok.Faint.copy(alpha = 0.45f)
}

private fun ProjectState.summaryColor(): Color = when (this) {
    ProjectState.NEEDS_REPLY -> Tok.Amber
    ProjectState.RUNNING, ProjectState.DONE -> Tok.Dim
    ProjectState.NEVER -> Tok.Faint
}

/**
 * 首页：项目列表本身。点一行进该项目的消息流——会话活着直接进，退出了/没有就
 * `POST /sessions`（daemon 幂等，且 resume 找不到旧对话会自动开新会话）再进。
 * 长按出项目操作单。新建按钮与设置入口由外层 HomeScreen 摆。
 */
@OptIn(ExperimentalFoundationApi::class, ExperimentalMaterial3Api::class)
@Composable
fun ProjectsHome(store: AppStore, nav: NavHostController) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val openSession = LocalOpenSession.current
    val projects by store.projects.collectAsState()
    val sessions by store.sessions.collectAsState()
    val conn by store.connState.collectAsState()
    val health by store.health.collectAsState()
    var query by rememberSaveable { mutableStateOf("") }
    var refreshing by remember { mutableStateOf(false) }
    // 正在 POST /sessions 的项目路径：挡双击（daemon 虽幂等，但两次并发到达仍可能各开一个）
    var busy by remember { mutableStateOf(setOf<String>()) }
    var actionsFor by remember { mutableStateOf<Project?>(null) }
    var purgeReport by remember { mutableStateOf<List<ProjectDeleteResult>?>(null) }

    LaunchedEffect(Unit) { store.refreshProjects() }

    // 只在输入变化时重算，不跟着 conn 延迟数字的重组一起算
    val groups = remember(projects, sessions, query) {
        val rows = projectRows(projects, sessions).filter { r ->
            query.isBlank() || r.project.name.contains(query, true) ||
                r.project.session_title.orEmpty().contains(query, true)
        }
        groupProjectRows(rows)
    }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_LONG).show()

    fun open(row: ProjectRow) {
        val p = row.project
        val primary = row.primary
        if (primary != null && primary.state != "exited") { openSession(primary.id, ""); return }
        val agent = p.agent ?: primary?.agent
        if (agent.isNullOrBlank()) {
            // 注册表里没登记 agent：让人先在操作单里选一个，不替他猜
            toast("项目未设置 agent，先在菜单里选一个"); actionsFor = p; return
        }
        if (p.path in busy) return
        busy = busy + p.path
        scope.launch {
            try {
                val api = store.client ?: throw IllegalStateException("未连接 daemon")
                // resume 一律 true：daemon 找到旧对话就续、找不到就开新的（shell 无 resume
                // 模板，天然开新）。exited 会话被 daemon 重启清掉后列表里没有它，但
                // agent 存储里的对话还在，按 primary != null 判会把续聊变成开新对话。
                val sess = api.createSession(p.path, agent, resume = true)
                openSession(sess.id, "")
            } catch (e: Exception) {
                toast("启动失败：${e.message}")
            } finally { busy = busy - p.path }
        }
    }

    Column(Modifier.fillMaxSize()) {
        // 顶栏：标题 + 连接状态；右侧齿轮进设置
        Row(
            Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("AAA", color = Tok.Ink, fontSize = 22.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
            Spacer(Modifier.width(12.dp))
            Box(Modifier.weight(1f)) {
                when (val c = conn) {
                    is ConnState.Connected -> DotWithText(Tok.Green, "${c.host.substringBefore(':')} · ${c.latencyMs}ms")
                    is ConnState.Connecting -> DotWithText(Tok.Amber, "连接中…")
                    is ConnState.Failed -> DotWithText(Tok.Red, "已断开")
                    ConnState.NoServer -> DotWithText(Tok.Dim, "未配对")
                }
            }
            // SSD 掉了是事故，才值得占顶栏；正常时不显示
            if (health?.ssd_mounted == false) {
                Text("SSD ✗", color = Tok.Red, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                Spacer(Modifier.width(4.dp))
            }
            IconButton(onClick = { nav.navigate("settings") }) {
                Text("⚙", color = Tok.Dim, fontSize = 20.sp)
            }
        }
        OutlinedTextField(
            query, { query = it },
            placeholder = { Text("搜索项目 / 会话命名…", color = Tok.Faint, fontSize = 13.sp) },
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), singleLine = true,
            textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
        )

        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch { store.refreshProjects(); store.refreshSessions(); store.refreshHealth(); refreshing = false }
            },
            modifier = Modifier.fillMaxSize(),
        ) {
            if (groups.isEmpty()) {
                Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                    if (projects.isEmpty()) {
                        Text("暂无项目", color = Tok.Faint)
                        Text("点右下 ＋ 新建", color = Tok.Faint, fontSize = 12.sp)
                    } else {
                        Text("没有匹配的项目", color = Tok.Faint)
                    }
                }
            }
            LazyColumn(Modifier.fillMaxSize()) {
                groups.forEach { (state, rows) ->
                    item(key = "hdr-${state.name}") {
                        Row(
                            Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 2.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            StateDot(state.dotColor(), 6)
                            Spacer(Modifier.width(7.dp))
                            Text(
                                "${state.label} ${rows.size}", color = Tok.Faint, fontSize = 12.sp,
                                fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace,
                            )
                        }
                    }
                    items(rows, key = { it.project.path }) { row ->
                        ProjectRowItem(
                            row,
                            busy = row.project.path in busy,
                            onClick = { open(row) },
                            onLongClick = { actionsFor = row.project },
                        )
                    }
                }
                item {
                    Text(
                        "点一行进入消息流 · 长按查看项目操作",
                        color = Tok.Faint, fontSize = 11.sp,
                        modifier = Modifier.fillMaxWidth().padding(14.dp),
                    )
                    Spacer(Modifier.height(80.dp))
                }
            }
        }
    }

    actionsFor?.let { p ->
        ProjectActionsSheet(store, nav, p, onDismiss = { actionsFor = null }, onPurged = { purgeReport = it })
    }
    purgeReport?.let { results -> PurgeReportDialog(results) { purgeReport = null } }
}

/** 一行：色点 + 项目名 + agent 徽记 + 时间；第二行一句摘要。目录大小等细节在长按单里。 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ProjectRowItem(row: ProjectRow, busy: Boolean, onClick: () -> Unit, onLongClick: () -> Unit) {
    val p = row.project
    Column(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(12.dp), contentAlignment = Alignment.CenterStart) {
                if (busy) CircularProgressIndicator(Modifier.width(10.dp).height(10.dp), strokeWidth = 1.5.dp, color = Tok.Cyan)
                else StateDot(row.state.dotColor())
            }
            Spacer(Modifier.width(8.dp))
            Text(
                p.name, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            // agent 只用着色小字，不画边框：列表里十几个方块叠起来比项目名还抢眼
            p.agent?.let {
                Text(if (it == "shell") "终端" else it, color = Tok.agentColor(it).copy(alpha = 0.85f), fontSize = 11.sp)
                Text(" · ", color = Tok.Faint, fontSize = 11.sp)
            }
            Text(relativeTime(row.timeIso), color = Tok.Faint, fontSize = 11.sp)
        }
        Text(
            row.summary, color = row.state.summaryColor(), fontSize = 13.sp,
            maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 20.dp, top = 3.dp),
        )
    }
    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 36.dp))
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
    val sessions by store.sessions.collectAsState()
    val primary = primarySessionFor(p, sessions)
    var deleteConfirm by remember { mutableStateOf(false) }

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
            if (primary != null && primary.state == "exited") {
                // 点行 = resume 新会话；上一条已退出的会话仍留着 transcript / diff / 回滚入口
                SheetItem("↺", "上次会话回放", "消息流 · diff · 回滚") { onDismiss(); openSession(primary.id, "") }
            }
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
