package cc.uoox.aaaui

import android.widget.Toast
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
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
import kotlinx.coroutines.launch

// ---------- A6 首页 = 项目列表 ----------

/** 这个 app 只跑 Claude Code：新建项目、没登记 agent 的旧项目都用它。 */
const val DEFAULT_AGENT = "claude"

/**
 * 项目行的状态字（2026-09-06 用户拍板，三端一致；不再用色点）：
 * 执行中 = 会话在跑；待回复 = 弹着选项等你选，不选就卡住（asking，哪怕屏幕还在变）；
 * 已激活 = 会话活着、停在输入框轮到你（waiting）；未激活 = 没有存活会话
 * （退出了 / 只有旧对话 / 从没跑过）。
 * 一个项目只有一个 agent（建项目时定死，从不切换），项目 ↔ 会话事实上一对一，所以
 * 会话状态直接挂在项目行上，首页不再单开会话页。终端永远不代表项目。全部由客户端把
 * projects × sessions 两个流拼出来。
 */
enum class ProjectState(val label: String) {
    RUNNING("执行中"),
    NEEDS_REPLY("待回复"),
    ACTIVE("已激活"),
    INACTIVE("未激活"),
}

/** 首页一行要的全部东西，纯数据，方便单测。 */
data class ProjectRow(
    val project: Project,
    /** 该项目的主会话；一个都没有时为 null。 */
    val primary: Session?,
    val state: ProjectState,
    /**
     * 排序键 = 该项目最近更新的会话的 `updated_at`（状态翻转 / 改名的时刻；老 daemon
     * 没有 → created_at），包括已退出的会话；没有会话的用目录 mtime。ISO 串，字典序即时间序。
     */
    val updatedIso: String,
) {
    /**
     * 标题，与 mac 侧栏同一口径：活着的会话的 title，退出后 daemon 从 agent 存储读出的
     * session_title，没有才退到文件夹名。文件夹名不是标题——它是地址。
     */
    val title: String get() =
        primary?.takeIf { it.state != "exited" }?.title?.takeIf { it.isNotBlank() }
            ?: project.session_title?.takeIf { it.isNotBlank() }
            ?: project.name
    /** 激活 = 主会话活着。exited 的会话只是历史，点一行即 resume。 */
    val alive: Boolean get() = primary != null && primary.state != "exited"
}

/** 用户拍板口径：待回复 = asking，与 state 无关。 */
fun Session.needsReply(): Boolean = asking

/** 会话最近一次有意义的变化：updated_at（v1.5）→ created_at（老 daemon）→ 最近输出。 */
fun Session.updatedIso(): String = updated_at.ifBlank { created_at.ifBlank { last_output_at } }

/**
 * 主会话：优先活着的（非 exited），待回复 < 执行中 < 其它，同级按最近输出；
 * 全都退出了就取最近退出的那个；终端永远不进入候选池。
 */
fun primarySessionFor(project: Project, sessions: List<Session>): Session? {
    val all = sessions.filter { it.project_path == project.path && it.agent != "shell" }
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

/** 四态之一。在问 = 待回复，哪怕屏幕还在变也不算「执行中」。 */
fun projectStateOf(primary: Session?): ProjectState = when {
    primary == null || primary.state == "exited" -> ProjectState.INACTIVE
    primary.needsReply() -> ProjectState.NEEDS_REPLY
    primary.state == "running" -> ProjectState.RUNNING
    else -> ProjectState.ACTIVE
}

/**
 * 一项目一行，**最近更新的会话在前**（2026-09-06 用户拍板，单列，不再分「激活 / 未激活」
 * 两栏）。排序看的是 daemon 的 `updated_at`（状态翻转 / 改名），不是每个字节都动的
 * last_output_at——几个会话同时在跑时行才不会互相换位。同刻按路径稳住。
 */
fun projectRows(projects: List<Project>, sessions: List<Session>): List<ProjectRow> = projects.map { p ->
    val primary = primarySessionFor(p, sessions)
    val latest = sessions
        .filter { it.project_path == p.path && it.agent != "shell" }
        .maxOfOrNull { it.updatedIso() }
        ?.takeIf { it.isNotBlank() }
    ProjectRow(p, primary, projectStateOf(primary), latest ?: p.mtime)
}.sortedWith(compareByDescending<ProjectRow> { it.project.pinned }.thenByDescending { it.updatedIso }.thenBy { it.project.path })

private fun ProjectState.color(): Color = when (this) {
    ProjectState.RUNNING -> Tok.Green
    ProjectState.NEEDS_REPLY -> Tok.Amber
    ProjectState.ACTIVE -> Tok.Accent
    ProjectState.INACTIVE -> Tok.Faint
}

/**
 * 状态字做成带边框的小标签：只靠字色分不开「已激活 / 未激活」，边框把它从标题里
 * 框出来，颜色（绿 / 黄 / 强调色 / 淡灰）再把四态拉开。定宽，标题才对得齐。
 */
@Composable
private fun StateTag(state: ProjectState) {
    Box(
        Modifier.width(48.dp).border(1.dp, state.color().copy(alpha = 0.7f), RoundedCornerShape(5.dp)).padding(vertical = 2.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(state.label, color = state.color(), fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, maxLines = 1)
    }
}

/** 新建项目：POST /projects + /sessions，返回新会话。名字留空 = 按日期命名。 */
suspend fun createProjectSession(store: AppStore, name: String?): Session {
    val api = store.client ?: throw IllegalStateException("未连接 daemon")
    // agent 显式写进注册表：daemon 对「没有登记」的目录会自己猜，不留给它猜
    val path = api.createProject(name, DEFAULT_AGENT).path
    return api.createSession(path, DEFAULT_AGENT, resume = false)
}

fun createErrorText(e: Exception): String =
    if (e is DaemonHttpException && e.errorCode == "conflict") "项目已存在" else "新建失败：${e.message}"

/**
 * 新建项目的输入框（首页顶部、会话页 ☰ 抽屉共用）：框里有光标时按回车 = 新建。
 * 回车从三条路来都接住：软键盘的动作键（Go / Done / Send / Search，输入法各不相同）、
 * 实体键盘 / 折叠屏外接键盘的 Enter，以及右边的 ＋。
 */
@Composable
fun NewProjectField(
    value: String,
    onValueChange: (String) -> Unit,
    creating: Boolean,
    onCreate: () -> Unit,
    modifier: Modifier = Modifier,
) {
    OutlinedTextField(
        value, onValueChange,
        placeholder = { Text("新建项目：文件夹名，回车", color = Tok.Faint, fontSize = 13.sp) },
        modifier = modifier.onPreviewKeyEvent { ev ->
            if (ev.type == KeyEventType.KeyDown && (ev.key == Key.Enter || ev.key == Key.NumPadEnter)) { onCreate(); true } else false
        },
        singleLine = true,
        textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { onCreate() }, onDone = { onCreate() }, onSend = { onCreate() }, onSearch = { onCreate() }),
        trailingIcon = {
            if (creating) {
                CircularProgressIndicator(Modifier.width(18.dp).height(18.dp), strokeWidth = 2.dp, color = Tok.Accent)
            } else {
                IconButton(onClick = onCreate) { Text("＋", color = Tok.Accent, fontSize = 22.sp) }
            }
        },
    )
}

/**
 * 首页：项目列表本身。点一行进该项目的消息流——会话活着直接进，退出了/没有就
 * `POST /sessions`（daemon 幂等，且 resume 找不到旧对话会自动开新会话）再进。
 * 长按出项目操作单。顶部输入框既过滤列表也新建项目（与 mac 侧栏一致）；设置入口在顶栏。
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
    val plan by store.planUsage.collectAsState()
    var planDialog by remember { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    var refreshing by remember { mutableStateOf(false) }
    // 正在 POST /sessions 的项目路径：挡双击（daemon 虽幂等，但两次并发到达仍可能各开一个）
    var busy by remember { mutableStateOf(setOf<String>()) }
    var actionsFor by remember { mutableStateOf<Project?>(null) }
    var purgeReport by remember { mutableStateOf<List<ProjectDeleteResult>?>(null) }
    // 正在 POST /projects + /sessions：挡住第二次回车
    var creating by remember { mutableStateOf(false) }
    val focusManager = LocalFocusManager.current

    LaunchedEffect(Unit) { store.refreshProjects() }

    // 只在输入变化时重算，不跟着 conn 延迟数字的重组一起算
    val rows = remember(projects, sessions, query) {
        projectRows(projects, sessions).filter { r ->
            query.isBlank() || r.project.name.contains(query, true) ||
                r.project.session_title.orEmpty().contains(query, true)
        }
    }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_LONG).show()

    /** 顶部输入框回车 / ＋：框里的字就是文件夹名，留空 = 按日期命名；建完清空并进入。 */
    fun create() {
        if (creating) return
        creating = true
        val name = query.trim().ifBlank { null }
        scope.launch {
            try {
                val sess = createProjectSession(store, name)
                query = ""
                focusManager.clearFocus()
                openSession(sess.id, "")
            } catch (e: Exception) {
                toast(createErrorText(e))
            } finally { creating = false }
        }
    }

    fun open(row: ProjectRow) {
        val p = row.project
        val primary = row.primary
        if (primary != null && primary.state != "exited") { openSession(primary.id, ""); return }
        // 注册表里登记了什么就跑什么（旧项目可能还是别的 agent，daemon 那头照样认）；
        // 没登记的一律 claude——这个 app 只跑 Claude Code，没有别的可选
        val agent = p.agent ?: primary?.agent ?: DEFAULT_AGENT
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
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = { nav.openTerminal() }) {
                    Text(">_", color = Tok.Dim, fontSize = 16.sp, fontFamily = FontFamily.Monospace)
                }
                val terminalCount = terminalSessions(sessions).size
                if (terminalCount > 0) Text(terminalCount.toString(), color = Tok.Accent, fontSize = 10.sp)
            }
            IconButton(onClick = { nav.navigate("settings") }) {
                Text("⚙", color = Tok.Dim, fontSize = 20.sp)
            }
        }
        // 套餐用量一行：5h / 7d / 按模型，最高的那个 ≥70 琥珀、≥90 红；点开看重置时间。没数据不占行
        val planSegs = planLineSegments(plan)
        if (planSegs.isNotEmpty()) {
            Text(
                segmentsAnnotated(planSegs, Tok.Faint),
                fontSize = 11.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis,
                modifier = Modifier.fillMaxWidth().clickable { planDialog = true }.padding(start = 16.dp, end = 16.dp, top = 0.dp, bottom = 6.dp),
            )
        }
        // 与 mac 侧栏同一件东西：边输入边过滤列表，回车或右边 ＋ 就按这个名字新建项目
        NewProjectField(query, { query = it }, creating, onCreate = { create() }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp))

        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch { store.refreshProjects(); store.refreshSessions(); store.refreshHealth(); refreshing = false }
            },
            modifier = Modifier.fillMaxSize(),
        ) {
            if (rows.isEmpty()) {
                Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                    if (projects.isEmpty()) {
                        Text("暂无项目", color = Tok.Faint)
                        Text("在上方输入文件夹名，回车新建", color = Tok.Faint, fontSize = 12.sp)
                    } else {
                        Text("没有匹配的项目", color = Tok.Faint)
                    }
                }
            }
            LazyColumn(Modifier.fillMaxSize().padding(top = 6.dp)) {
                items(rows, key = { it.project.path }) { row ->
                    // 顺序随最近更新变：Compose 按 key 做位移过渡，上移/下移都有动画
                    ProjectRowItem(
                        row,
                        modifier = Modifier.animateItem(),
                        busy = row.project.path in busy,
                        onClick = { open(row) },
                        onLongClick = { actionsFor = row.project },
                    )
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
    if (planDialog) plan?.let { PlanUsageDialog(it) { planDialog = false } }
}

/**
 * 会话页 ☰ 抽屉里的项目列表：与首页同一份行（同一排序、同一状态字），点一行切过去——
 * 会话活着直接进，退出了 / 没有就 `POST /sessions` resume 再进。当前项目高亮。
 */
@Composable
fun ProjectSwitcher(store: AppStore, currentPath: String?, onHome: () -> Unit, onOpened: () -> Unit) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val openSession = LocalOpenSession.current
    val projects by store.projects.collectAsState()
    val sessions by store.sessions.collectAsState()
    var busy by remember { mutableStateOf<String?>(null) }
    val rows = remember(projects, sessions) { projectRows(projects, sessions) }
    LaunchedEffect(Unit) { store.refreshProjects() }

    fun open(row: ProjectRow) {
        val primary = row.primary
        if (primary != null && primary.state != "exited") { onOpened(); openSession(primary.id, ""); return }
        if (busy != null) return
        busy = row.project.path
        scope.launch {
            try {
                val api = store.client ?: throw IllegalStateException("未连接 daemon")
                val sess = api.createSession(row.project.path, row.project.agent ?: DEFAULT_AGENT, resume = true)
                onOpened(); openSession(sess.id, "")
            } catch (e: Exception) {
                Toast.makeText(context, "启动失败：${e.message}", Toast.LENGTH_LONG).show()
            } finally { busy = null }
        }
    }

    // 抽屉里也能直接新建项目：建完关抽屉、进新会话
    var newName by remember { mutableStateOf("") }
    var creating by remember { mutableStateOf(false) }
    fun create() {
        if (creating) return
        creating = true
        val name = newName.trim().ifBlank { null }
        scope.launch {
            try {
                val sess = createProjectSession(store, name)
                newName = ""
                onOpened(); openSession(sess.id, "")
            } catch (e: Exception) {
                Toast.makeText(context, createErrorText(e), Toast.LENGTH_LONG).show()
            } finally { creating = false }
        }
    }

    Column(Modifier.fillMaxSize().navigationBarsPadding()) {
        Row(
            Modifier.fillMaxWidth().padding(start = 18.dp, end = 8.dp, top = 14.dp, bottom = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("项目", color = Tok.Ink, fontSize = 16.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            TextButton(onClick = onHome) { Text("首页", color = Tok.Accent, fontSize = 13.sp) }
        }
        NewProjectField(newName, { newName = it }, creating, onCreate = { create() }, modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp))
        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
            items(rows, key = { it.project.path }) { row ->
                val current = row.project.path == currentPath
                Row(
                    Modifier.fillMaxWidth()
                        .background(if (current) Tok.Raised else Color.Transparent)
                        .clickable { open(row) }
                        .padding(horizontal = 18.dp, vertical = 11.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.width(48.dp), contentAlignment = Alignment.Center) {
                        if (busy == row.project.path) CircularProgressIndicator(Modifier.width(12.dp).height(12.dp), strokeWidth = 1.5.dp, color = Tok.Accent)
                        else StateTag(row.state)
                    }
                    Spacer(Modifier.width(10.dp))
                    if (row.project.pinned) Text("📌", fontSize = 11.sp, modifier = Modifier.padding(end = 4.dp))
                    Text(
                        row.title, color = if (row.alive || current) Tok.Ink else Tok.Dim, fontSize = 14.sp,
                        fontWeight = if (current) FontWeight.Bold else FontWeight.Medium,
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
                    )
                }
            }
            if (rows.isEmpty()) item { Text("暂无项目", color = Tok.Faint, modifier = Modifier.padding(18.dp)) }
        }
    }
}

/** 每个窗口一行：名称 + 百分比（按级别着色）+ 重置时间 */
@Composable
fun PlanUsageDialog(plan: PlanUsage, onDismiss: () -> Unit) {
    data class Line(val name: String, val pct: Double?, val resetsAt: kotlinx.serialization.json.JsonElement?)
    val lines = buildList {
        plan.five_hour?.let { add(Line("5 小时", it.used_percentage, it.resets_at)) }
        plan.seven_day?.let { add(Line("7 天", it.used_percentage, it.resets_at)) }
        plan.model_scoped.orEmpty().forEach { add(Line(it.display_name.ifBlank { "模型" }, it.utilization, it.resets_at)) }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Tok.Raised,
        title = { Text("套餐用量", color = Tok.Ink) },
        text = {
            Column {
                lines.forEach { l ->
                    Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(l.name, color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                        Text(
                            l.pct?.let { pctText(it) } ?: "—",
                            color = pctColor(pctColorLevel(l.pct), Tok.Ink), fontSize = 14.sp, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.Bold,
                        )
                        resetLabel(parseResetsAt(l.resetsAt))?.let {
                            Spacer(Modifier.width(12.dp))
                            Text(it, color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                        }
                    }
                }
                if (lines.isEmpty()) Text("暂无数据", color = Tok.Faint)
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("关闭", color = Tok.Dim) } },
    )
}

/** 一行到底：状态字 + 标题 + 更新时间。不再有第二行——目录大小等细节在长按单里。 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ProjectRowItem(row: ProjectRow, busy: Boolean, onClick: () -> Unit, onLongClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier) {
        Row(
            Modifier.fillMaxWidth()
                .combinedClickable(onClick = onClick, onLongClick = onLongClick)
                .padding(horizontal = 16.dp, vertical = 11.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // 正在 resume 的行用转圈顶替标签
            Box(Modifier.width(48.dp), contentAlignment = Alignment.Center) {
                if (busy) CircularProgressIndicator(Modifier.width(12.dp).height(12.dp), strokeWidth = 1.5.dp, color = Tok.Accent)
                else StateTag(row.state)
            }
            Spacer(Modifier.width(10.dp))
            if (row.project.pinned) Text("📌", fontSize = 12.sp, modifier = Modifier.padding(end = 4.dp))
            Text(
                row.title, color = if (row.alive) Tok.Ink else Tok.Dim, fontSize = 15.sp, fontWeight = FontWeight.Bold,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            Text(relativeTime(row.updatedIso), color = Tok.Faint, fontSize = 11.sp)
        }
        HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 74.dp))
    }
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
    var killConfirm by remember { mutableStateOf(false) }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
    val muted = p.path in settings.mutedProjects
    val alive = primary != null && primary.state != "exited"
    /** 结束会话 = 项目回到未激活栏。用户自己动的手，随后的 exited 不弹通知。 */
    fun killPrimary() {
        val id = primary?.id ?: return
        scope.launch {
            store.markUserKilled(id)
            runCatching { store.client?.kill(id) }.onFailure { toast("失败：${it.message}") }
            store.refreshSessions()
        }
        onDismiss()
    }

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = Tok.Surface) {
        Column(Modifier.padding(bottom = 20.dp)) {
            Column(Modifier.padding(horizontal = 18.dp, vertical = 4.dp)) {
                Text(p.name, color = Tok.Ink, fontSize = 16.sp, fontWeight = FontWeight.Bold)
                Text(
                    "${p.path} · ${humanBytes(p.dir_size)}",
                    color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                )
            }
            if (alive) {
                // 与 mac 侧栏的 × 同一规则：只有执行中的才确认（被顺手点掉最伤），等你的直接结束
                SheetItem("■", "结束会话", "项目变为未激活", danger = true) {
                    if (primary.state == "running") killConfirm = true else killPrimary()
                }
            }
            if (!alive) SheetItem("▶", "继续会话", "resume") {
                val agent = p.agent ?: DEFAULT_AGENT
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, agent, resume = true) ?: return@launch
                        onDismiss(); openSession(sess.id, "")
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
            }
            if (primary != null && primary.state == "exited") {
                // 点行 = resume 新会话；上一条已退出的会话仍留着 transcript 回放入口
                SheetItem("↺", "上次会话回放", "消息流 · 终端回放") { onDismiss(); openSession(primary.id, "") }
            }
            SheetItem("📌", if (p.pinned) "取消置顶" else "置顶", "列表最前") {
                scope.launch {
                    runCatching { store.client?.setPinned(p.path, !p.pinned) }.onFailure { toast("失败：${it.message}") }
                    store.refreshProjects()
                }
                onDismiss()
            }
            SheetItem("＞", "在此目录开终端", "zsh") {
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, "shell", resume = false, fresh = true) ?: return@launch
                        onDismiss(); nav.openTerminal(sess.id)
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
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

    if (killConfirm) {
        ConfirmDialog(
            "结束正在执行的会话？",
            "agent 正在跑，结束后这一轮的工作会中断；对话记录保留，之后可以 resume。",
            "结束",
            onConfirm = { killConfirm = false; killPrimary() },
            onCancel = { killConfirm = false },
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
