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
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.material3.OutlinedTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.ImeAction
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

/** 这个 app 只跑 Claude Code：新建项目、没登记 agent 的旧项目都用它。 */
const val DEFAULT_AGENT = "claude"

/**
 * 项目行的状态，只决定色点与摘要，**不决定分组**。一个项目只有一个 agent（建项目时
 * 定死，从不切换），项目 ↔ 会话事实上一对一，所以会话状态直接挂在项目行上，首页不再
 * 单开会话页。终端永远不代表项目。全部由客户端把 projects × sessions 两个流拼出来，
 * daemon 不用改。
 */
enum class ProjectState(val label: String) {
    NEEDS_REPLY("待回复"),
    RUNNING("执行中"),
    DONE("已完成"),
    NEVER("未开始"),
}

/**
 * 首页两栏（2026-09-03 用户拍板，三端一致）：**激活** = 有存活会话（running / waiting，
 * 在问只点亮黄点）；**未激活** = 其余（exited、只有旧对话、从没跑过）。以前按四态分组，
 * 几个会话同时在跑时行在「执行中 / 待回复 / 已完成」之间跳来跳去，点都点不准。
 * 枚举顺序就是首页分组顺序。
 */
enum class ProjectGroup(val label: String) {
    ACTIVE("激活"),
    INACTIVE("未激活"),
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
    /**
     * 第一行标题，与 mac 侧栏同一口径：会话的命名（活着的会话 title，退出后 daemon 从
     * agent 存储读出的 session_title），没有才退到文件夹名。文件夹名不是标题——它是地址。
     */
    val title: String get() =
        primary?.title?.takeIf { it.isNotBlank() }
            ?: project.session_title?.takeIf { it.isNotBlank() }
            ?: project.name
    /** 第二行：状态摘要。文件夹名不再单独占一行——没开过对话时它就是标题（2026-09-03 用户拍板）。 */
    val subtitle: String get() = summary
    /** 激活 = 主会话活着。exited 的会话只是历史，项目回到未激活栏，点一行即 resume。 */
    val group: ProjectGroup get() = if (primary != null && primary.state != "exited") ProjectGroup.ACTIVE else ProjectGroup.INACTIVE
    /** 行尾的相对时间：有会话按最近输出，没有按目录 mtime；都是 daemon 给的 ISO 时间串。 */
    val timeIso: String get() = primary?.last_output_at?.takeIf { it.isNotBlank() } ?: project.mtime
}

/** 用户拍板口径：待回复 = asking，与 state 无关。 */
fun Session.needsReply(): Boolean = asking

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
 * 第二行的一句话：只说状态。标题行已经是会话名，这里再放标题就是重复；也不用 preview——
 * 它是屏幕末 4 行，TUI 型 agent 那里永远是输入框和底栏，当摘要只会是垃圾。
 */
@Suppress("UNUSED_PARAMETER")
fun projectSummary(project: Project, primary: Session?, state: ProjectState): String = when (state) {
    ProjectState.NEEDS_REPLY -> "等你回答"
    ProjectState.RUNNING -> if (primary?.compacting == true) "整理上下文中" else "执行中"
    // 未激活的项目没什么可说的：点一行就是 resume，不必每行都写「点击继续」。
    // 上一轮以错误收场的除外：那句话值得留着
    ProjectState.DONE -> primary?.error?.let { errorLabel(it) } ?: ""
    ProjectState.NEVER -> "未开始"
}

/** StopFailure 的错误类型 → 一句人话 */
fun errorLabel(kind: String): String = when (kind) {
    "rate_limit" -> "上轮出错：限流"
    "overloaded" -> "上轮出错：服务过载"
    "authentication_failed" -> "上轮出错：登录失效"
    "billing_error" -> "上轮出错：账单问题"
    else -> "上轮出错：$kind"
}

fun projectRows(projects: List<Project>, sessions: List<Session>): List<ProjectRow> = projects.map { p ->
    val primary = primarySessionFor(p, sessions)
    val state = projectStateOf(p, primary)
    ProjectRow(p, primary, state, projectSummary(p, primary, state))
}

/**
 * 两栏分组，空栏不出现。**顺序不随状态或输出变**：激活栏按会话开启时间
 * （created_at 升序，末尾最新）再按路径稳住，几个会话同时在跑也不跳行——刚激活的
 * 落在上栏底部；未激活栏最近有动静的在前——刚关掉的会话所属项目排第一，从没跑过的
 * 按目录 mtime 靠后（与 mac 侧栏同一口径，2026-09-03）。状态交给色点。
 */
fun groupProjectRows(rows: List<ProjectRow>): List<Pair<ProjectGroup, List<ProjectRow>>> =
    ProjectGroup.entries
        .map { g ->
            val inGroup = rows.filter { it.group == g }
            g to when (g) {
                ProjectGroup.ACTIVE -> inGroup.sortedWith(compareBy({ it.primary?.created_at.orEmpty() }, { it.project.path }))
                ProjectGroup.INACTIVE -> inGroup.sortedWith(compareByDescending<ProjectRow> { it.timeIso }.thenBy { it.project.path })
            }
        }
        .filter { it.second.isNotEmpty() }

private fun ProjectGroup.dotColor(): Color = when (this) {
    ProjectGroup.ACTIVE -> Tok.Green
    ProjectGroup.INACTIVE -> Tok.Faint
}

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
    val groups = remember(projects, sessions, query) {
        val rows = projectRows(projects, sessions).filter { r ->
            query.isBlank() || r.project.name.contains(query, true) ||
                r.project.session_title.orEmpty().contains(query, true)
        }
        groupProjectRows(rows)
    }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_LONG).show()

    /** 顶部输入框回车 / ＋：框里的字就是文件夹名，留空 = 按日期命名；建完清空并进入。 */
    fun create() {
        if (creating) return
        creating = true
        val name = query.trim().ifBlank { null }
        scope.launch {
            try {
                val api = store.client ?: throw IllegalStateException("未连接 daemon")
                // agent 显式写进注册表：daemon 对「没有登记」的目录会自己猜，不留给它猜
                val path = api.createProject(name, DEFAULT_AGENT).path
                val sess = api.createSession(path, DEFAULT_AGENT, resume = false)
                query = ""
                focusManager.clearFocus()
                openSession(sess.id, "")
            } catch (e: Exception) {
                toast(if (e is DaemonHttpException && e.errorCode == "conflict") "项目已存在" else "新建失败：${e.message}")
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
        // 与 mac 侧栏同一件东西：边输入边过滤列表，回车或右边 ＋ 就按这个名字新建项目
        OutlinedTextField(
            query, { query = it },
            placeholder = { Text("新建项目：文件夹名，回车", color = Tok.Faint, fontSize = 13.sp) },
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp), singleLine = true,
            textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { create() }),
            trailingIcon = {
                if (creating) {
                    CircularProgressIndicator(Modifier.width(18.dp).height(18.dp), strokeWidth = 2.dp, color = Tok.Accent)
                } else {
                    IconButton(onClick = { create() }) { Text("＋", color = Tok.Accent, fontSize = 22.sp) }
                }
            },
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
                        Text("在上方输入文件夹名，回车新建", color = Tok.Faint, fontSize = 12.sp)
                    } else {
                        Text("没有匹配的项目", color = Tok.Faint)
                    }
                }
            }
            LazyColumn(Modifier.fillMaxSize()) {
                groups.forEach { (group, rows) ->
                    item(key = "hdr-${group.name}") {
                        Row(
                            Modifier.padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 2.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            StateDot(group.dotColor(), 6)
                            Spacer(Modifier.width(7.dp))
                            Text(
                                "${group.label} ${rows.size}", color = Tok.Faint, fontSize = 12.sp,
                                fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace,
                            )
                        }
                    }
                    items(rows, key = { it.project.path }) { row ->
                        // 同一个 LazyColumn 里跨栏搬家：Compose 按 key 做位移过渡，上移/下移都有动画
                        ProjectRowItem(
                            row,
                            modifier = Modifier.animateItem(),
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

/** 一行：色点 + 项目名 + 时间；第二行一句摘要。目录大小等细节在长按单里。 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ProjectRowItem(row: ProjectRow, busy: Boolean, onClick: () -> Unit, onLongClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier) {
    Column(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(12.dp), contentAlignment = Alignment.CenterStart) {
                if (busy) CircularProgressIndicator(Modifier.width(10.dp).height(10.dp), strokeWidth = 1.5.dp, color = Tok.Accent)
                else StateDot(row.state.dotColor())
            }
            Spacer(Modifier.width(8.dp))
            Text(
                row.title, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(8.dp))
            Text(relativeTime(row.timeIso), color = Tok.Faint, fontSize = 11.sp)
        }
        if (row.subtitle.isNotEmpty()) Text(
            row.subtitle, color = row.state.summaryColor(), fontSize = 13.sp,
            maxLines = 1, overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 20.dp, top = 3.dp),
        )
    }
    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 36.dp))
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
                SheetItem("■", "结束会话", "项目回到未激活栏", danger = true) {
                    if (primary?.state == "running") killConfirm = true else killPrimary()
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
                // 点行 = resume 新会话；上一条已退出的会话仍留着 transcript / diff / 回滚入口
                SheetItem("↺", "上次会话回放", "消息流 · diff · 回滚") { onDismiss(); openSession(primary.id, "") }
            }
            SheetItem("＞", "在此目录开终端", "zsh") {
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, "shell", resume = false, fresh = true) ?: return@launch
                        onDismiss(); nav.openTerminal(sess.id)
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

        // 添加框放在列表上方（跟首页的新建框一个位置），不钉在屏幕底部
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.weight(1f).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
                contentAlignment = Alignment.CenterStart,
            ) {
                if (input.isEmpty()) Text("添加任务…", color = Tok.Faint, fontSize = 14.sp)
                BasicTextField(
                    input, { input = it },
                    modifier = Modifier.fillMaxWidth(), maxLines = 3,
                    textStyle = androidx.compose.ui.text.TextStyle(color = Tok.Ink, fontSize = 14.sp),
                    cursorBrush = SolidColor(Tok.Accent),
                )
            }
            Spacer(Modifier.width(6.dp))
            TextButton(
                enabled = input.isNotBlank(),
                onClick = {
                    val text = input.trim(); input = ""
                    scope.launch {
                        runCatching { store.client?.inboxAdd(projectPath, text) }
                            .onFailure { Toast.makeText(context, "添加失败：${it.message}", Toast.LENGTH_SHORT).show() }
                        reload()
                    }
                },
            ) { Text("添加", color = if (input.isNotBlank()) Tok.Accent else Tok.Faint) }
        }

        LazyColumn(Modifier.weight(1f).fillMaxWidth()) {
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

    }
}
