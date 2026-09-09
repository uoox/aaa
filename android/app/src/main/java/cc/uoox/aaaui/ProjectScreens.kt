package cc.uoox.aaaui

import android.widget.Toast
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.clickable
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
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
import androidx.compose.runtime.produceState
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

/** 表里第一个、也是没登记 agent 的旧项目的回退。真正开谁以注册表 / `GET /agents` 为准。 */
const val DEFAULT_AGENT = "claude"

/** 首页一行要的全部东西，纯数据，方便单测。 */
data class ProjectRow(
    val project: Project,
    /**
     * daemon 指名的代表会话（`session_id`）在 `/sessions` 里的那一条；不在池子里就是 null。
     * 「点这行开哪个会话」看的是 `project.session_id`，不是这个对象——它只是拿来读会话细节的。
     */
    val primary: Session?,
    /**
     * 黄点：跑完一轮 / 在等你回话，而这台设备还没进去看过（本地状态，见 SettingsStore
     * 的 unreadProjects）。进会话就没了。
     */
    val unread: Boolean = false,
) {
    /** 五态之一，daemon 算好的；老 daemon 不给 → 空串（未知，排最后、不画底色） */
    val status: String get() = project.status

    /**
     * 标题。回退链（活会话标题 → agent 存储的 session_title → 目录名）v1.22 起在 daemon 里走完，
     * 这边只兜一下空：老 daemon 不给 `title`，退到目录名——文件夹名不是标题，它是地址。
     */
    val title: String get() = project.title?.takeIf { it.isNotBlank() } ?: project.name

    /** 排序键：daemon 的 `updated_at`（状态翻转 / 改名的时刻），一个会话都没有时是目录 mtime */
    val updatedIso: String get() = project.updated_at?.takeIf { it.isNotBlank() } ?: project.mtime

    /** 激活 = 此刻还有活会话。`paused`（含没有会话）之外的四态都算。 */
    val alive: Boolean get() = status.isNotBlank() && status != "paused"
}

/** 淡蓝底：它还在动，你不用管（自己在跑，或后台任务还没回来）。`asking` 不蓝——那是在等你。 */
fun statusRunning(s: String) = s == "running" || s == "background"

/**
 * 一项目一行。排序（2026-09-10 用户拍板拿掉置顶后）：**有黄底 > 状态 > 时间倒序 > 路径**。
 * 黄底排在蓝底前面——蓝底的还在自己往前走，黄底的那个是在等你。
 * 状态和时间都读 `/projects` 行上 daemon 算好的 `status` / `updated_at`：`updated_at` 只在状态
 * 翻转 / 改名时变，几个会话同时在跑时行才不会互相换位。同刻按路径稳住。
 *
 * v1.22 删掉了这里的 `primarySessionFor` / `projectStateOf`：那两个函数各自实现了一遍「谁代表
 * 这个项目」和「它是五态里的哪一个」，mac 的实现又是另一套（mac 取 updated_at 最大，Android 先
 * 按 agent 过滤再按「待回复 < 执行中 < 其它」排），同一台 daemon 在两端能显示出不同的标题和状态。
 */
fun projectRows(projects: List<Project>, sessions: List<Session>, unread: Set<String> = emptySet()): List<ProjectRow> {
    val byId = sessions.associateBy { it.id }
    return projects.map { p ->
        ProjectRow(p, p.session_id?.let { byId[it] }, pathListContains(unread, p.path))
    }.sortedWith(
        // 2026-09-10 用户拍板：就按行尾那个「xxx 分钟前」从新到旧排，不分档。
        // 状态已经由整行底色说了，再拿它排一遍是同一件事说两遍，而且行会因为状态翻转跳位置。
        compareByDescending<ProjectRow> { it.updatedIso }.thenBy { it.project.path },
    )
}

/**
 * 整行的状态底色（2026-09-10 用户拍板「去掉竖线状态的设计，改为背景色，用浅色」；2026-09-08
 * 那四版记号——转圈 → 蓝点 → 竖线 → 行尾竖线——全部作废）：**淡黄** = 在等你、而这台设备还
 * 没进去看过；**淡蓝** = 它还在动（含后台任务没回来，以及本行正在 resume）；**没有底色** =
 * 已读，没什么要你操心的。**黄盖过蓝**——黄的那个在等你。纯函数，好测；行高不随状态跳。
 */
fun rowBackground(status: String, unread: Boolean, busy: Boolean = false): Color? = when {
    unread -> Tok.RowUnread
    busy || statusRunning(status) -> Tok.RowRunning
    else -> null
}

/** 竖线本体：只剩看板卡片在用（项目行 2026-09-10 起改成整行淡底） */
@Composable
fun MarkBar(color: Color, modifier: Modifier = Modifier) {
    Box(modifier.width(2.5.dp).height(16.dp).background(color, RoundedCornerShape(1.25.dp)))
}

/** 新建项目：POST /projects + /sessions，返回新会话。名字留空 = 按日期命名。 */
suspend fun createProjectSession(store: AppStore, name: String?, agent: String = DEFAULT_AGENT): Session {
    val api = store.client ?: throw IllegalStateException("未连接 daemon")
    // agent 显式写进注册表：daemon 对「没有登记」的目录会自己猜，不留给它猜
    val path = api.createProject(name, agent).path
    return api.createSession(path, agent, resume = false)
}

/**
 * 表里下一个装了的 agent。只装了一个就是 null——没有「换」这回事，切换入口整个不画
 * （老 daemon 不给 `/agents`，表是空的，同样不画）。例外：当前这个**没装**（卸载了 /
 * 换了台机器）时给一条回到装了的那个的路，否则这一行永远换不回来。
 */
fun nextAgent(agents: List<AgentInfo>, current: String): String? {
    val usable = agents.filter { it.available }
    val first = usable.firstOrNull() ?: return null
    val i = usable.indexOfFirst { it.id == current }
    if (i < 0) return first.id
    if (usable.size < 2) return null
    return usable[(i + 1) % usable.size].id
}

fun agentLabel(agents: List<AgentInfo>, id: String): String =
    agents.firstOrNull { it.id == id }?.label ?: id

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
    agentMark: String? = null,
    onSwapAgent: () -> Unit = {},
) {
    OutlinedTextField(
        value, onValueChange,
        placeholder = { Text("新建项目：文件夹名，回车", color = Tok.Faint, fontSize = 13.sp) },
        // 新项目开谁：点一下在装了的 agent 之间轮换。只有一个可用时 agentMark 是 null，整个不画
        leadingIcon = agentMark?.let { mark ->
            {
                Box(Modifier.size(28.dp).clickable(onClick = onSwapAgent), contentAlignment = Alignment.Center) {
                    Text(mark, color = Tok.Accent, fontSize = 14.sp, fontWeight = FontWeight.Bold)
                }
            }
        },
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
 * 项目面板 = 以前的首页整块（2026-09-07 用户拍板：☰ 抽屉要有首页所有按钮和功能，首页就没
 * 必要单独存在了）。会话页的 ☰ 抽屉和「一个会话都没打开」时的落地页画的都是它：顶栏
 * 一行（额度 / 看板 / 设置，2026-09-08 用户拍板砍到这三样）、新建项目框、项目列表（点开、长按操作）、
 * 下拉刷新。`currentPath` / `currentTerminalId` 标出正在看的那一行（强调色标题 + 整行边框，二选一：
 * 会话屏给项目路径，终端屏给终端 id——终端不是项目，按 cwd 去点亮项目行是错的）；
 * `onBeforeNavigate` 在抽屉里就是「先关抽屉」。
 *
 * 点一行进该项目的消息流——会话活着直接进，退出了/没有就 `POST /sessions`（daemon 幂等，
 * 且 resume 找不到旧对话会自动开新会话）再进；长按出项目操作单。顶部输入框既过滤列表也
 * 新建项目（与 mac 侧栏一致）。
 *
 * 面板本身画的东西分成四块：降级横幅 [SchemaTooOldBanner]、顶栏 [ProjectPanelHeader]、
 * 新建框 [NewProjectField]、列表（[projectSection] + [terminalSection]）。留在这里的是
 * **状态和动作**——建项目 / 开终端 / 开会话都要 store 与导航，收不进任何一块里。
 */
@OptIn(ExperimentalFoundationApi::class, ExperimentalMaterial3Api::class)
@Composable
fun ProjectPanel(store: AppStore, nav: NavHostController, currentPath: String? = null, currentTerminalId: String? = null, onBeforeNavigate: () -> Unit = {}) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val openSession = LocalOpenSession.current
    val projects by store.projects.collectAsState()
    val sessions by store.sessions.collectAsState()
    val conn by store.connState.collectAsState()
    val health by store.health.collectAsState()
    val plan by store.planUsage.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    var planDialog by remember { mutableStateOf(false) }
    /** 顶上那个框里的字：**只是新项目的文件夹名**，不是搜索词（v1.22 用户拍板） */
    var newName by rememberSaveable { mutableStateOf("") }
    /** 下一个新建项目用哪个 agent（表里第一个装了的；只有一个可用时不画切换） */
    var newAgent by rememberSaveable { mutableStateOf(DEFAULT_AGENT) }
    var refreshing by remember { mutableStateOf(false) }
    // 正在 POST /sessions 的项目路径：挡双击（daemon 虽幂等，但两次并发到达仍可能各开一个）
    var busy by remember { mutableStateOf(setOf<String>()) }
    var actionsFor by remember { mutableStateOf<Project?>(null) }
    var purgeReport by remember { mutableStateOf<List<ProjectDeleteResult>?>(null) }
    // 正在 POST /projects + /sessions：挡住第二次回车
    var creating by remember { mutableStateOf(false) }
    var creatingTerminal by remember { mutableStateOf(false) }
    val root = health?.project_root?.takeIf { it.isNotBlank() } ?: "/Volumes/SSD/project"
    // 行尾那个「N 分钟前」得自己会走：这一屏只在 daemon 推了东西时重组，整套系统闲着
    // 的时候那行字会一直停在「刚刚」。一分钟一跳，只有时间那一个 Text 跟着重组。
    val minuteTick by produceState(0L) {
        while (true) {
            kotlinx.coroutines.delay(60_000)
            value++
        }
    }
    val terminals = remember(sessions) { terminalSessions(sessions) }
    val focusManager = LocalFocusManager.current

    LaunchedEffect(Unit) { store.refreshProjects() }

    // 只在项目 / 会话 / 黄点变化时重算，不跟着 conn 延迟数字的重组一起算。
    // v1.22 用户拍板：**顶上那个框只用来新建项目，不当搜索框**（「侧栏就不要做搜索框了，
    // 双端都不要」）——所以这里不再按框里的字过滤，mac 侧本来也没有过滤，两端就此一致。
    val unread = settings.unreadProjects
    val rows = remember(projects, sessions, unread) { projectRows(projects, sessions, unread) }
    val agents by store.agents.collectAsState()
    // 选中的 agent 没装（或表里没有）就退到第一个装了的
    LaunchedEffect(agents) {
        if (agents.none { it.id == newAgent && it.available }) {
            agents.firstOrNull { it.available }?.let { newAgent = it.id }
        }
    }
    val swapNewAgent = nextAgent(agents, newAgent)

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_LONG).show()

    /** 顶部输入框回车 / ＋：框里的字就是文件夹名，留空 = 按日期命名；建完清空并进入。 */
    fun create() {
        if (creating) return
        creating = true
        val name = newName.trim().ifBlank { null }
        scope.launch {
            try {
                val sess = createProjectSession(store, name, newAgent)
                newName = ""
                focusManager.clearFocus()
                onBeforeNavigate(); openSession(sess.id, "")
            } catch (e: Exception) {
                toast(createErrorText(e))
            } finally { creating = false }
        }
    }

    /** 底部「＋ 新增终端」：在项目根开一个 shell，attach 先热起来，再进终端屏 */
    fun newTerminal() {
        if (creatingTerminal) return
        creatingTerminal = true
        scope.launch {
            try {
                val sess = store.client?.createSession(root, "shell", resume = false, fresh = true)
                    ?: throw IllegalStateException("未连接 daemon")
                store.refreshSessions()
                store.prewarmAttachment(sess.id)
                onBeforeNavigate(); nav.openTerminal(sess.id)
            } catch (e: Exception) {
                toast("开终端失败：${e.message}")
            } finally { creatingTerminal = false }
        }
    }

    fun open(row: ProjectRow) {
        val p = row.project
        // 还活着就直接进；进哪一个由 daemon 的 session_id 说了算，客户端不再自己从 /sessions 里挑代表
        if (row.alive) p.session_id?.let { onBeforeNavigate(); openSession(it, ""); return }
        // 注册表里没有这个目录（在别处 aaa open 开出来的），daemon 不认它，resume 只会建错东西
        if (!p.registered) { toast("这个目录不在项目注册表里，只能在它还有会话时打开"); return }
        // 注册表里登记了什么就跑什么；没登记的回退到默认 agent（长按单里可以换）
        val agent = p.agent ?: row.primary?.agent ?: DEFAULT_AGENT
        if (p.path in busy) return
        busy = busy + p.path
        scope.launch {
            try {
                val api = store.client ?: throw IllegalStateException("未连接 daemon")
                // resume 一律 true：daemon 找到旧对话就续、找不到就开新的（shell 无 resume
                // 模板，天然开新）。exited 会话被 daemon 重启清掉后列表里没有它，但
                // agent 存储里的对话还在，按 primary != null 判会把续聊变成开新对话。
                val sess = api.createSession(p.path, agent, resume = true)
                onBeforeNavigate(); openSession(sess.id, "")
            } catch (e: Exception) {
                toast("启动失败：${e.message}")
            } finally { busy = busy - p.path }
        }
    }

    // 顶栏只剩一栏（2026-09-08 用户拍板）：去掉 "AAA" 标题和 IP · 延迟——app 只有一个，
    // 标题是废话；IP 和 ms 连着好的时候没人看。留下的三样是真会用的：额度、看板、设置。
    // 连接**不**正常时那一格改写连接状态：断了得说一声，但这不值得常年占一整行。
    val planSegs = planLineSegments(plan)
    // 面板本身不管宽度：它铺满给它的地方。宽度由**外面那层**定——☰ 抽屉按
    // [sidebarFraction] 小屏铺满 / 大屏半屏，没打开会话时它就是整屏。
    Column(Modifier.fillMaxSize()) {
        // 降级横幅（PROTOCOL「版本兼容」）：schema 是唯一的闸门，且降级必须说出来。老 daemon 不给
        // status / title / session_id，这一屏就画不出项目状态——**不留第二套算法**，留着就等于把
        // 刚删掉的分歧又养回来（daemon 与 mac App 同机同版发布，只有手机可能先更新，是几分钟的窗口）
        health?.takeIf { it.schema < SCHEMA_PROJECT_STATUS }?.let { h -> SchemaTooOldBanner(h.version) }
        ProjectPanelHeader(
            conn = conn,
            planSegs = planSegs,
            ssdMissing = health?.ssd_mounted == false,
            onPlanClick = { planDialog = true },
            onHistory = { onBeforeNavigate(); nav.navigate("history") },
            onSettings = { onBeforeNavigate(); nav.navigate("settings") },
        )
        // 与 mac 侧栏顶上那一行同一件东西：框里的字就是文件夹名，回车或右边 ＋ 新建；
        // 左边的小标是「新项目开谁」（v1.22 拍板这个框不当搜索框，所以它不过滤列表）
        NewProjectField(
            newName, { newName = it }, creating, onCreate = { create() },
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            agentMark = swapNewAgent?.let { agentLabel(agents, newAgent).take(1) },
            onSwapAgent = { swapNewAgent?.let { newAgent = it } },
        )

        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch { store.refreshProjects(); store.refreshSessions(); store.refreshHealth(); refreshing = false }
            },
            modifier = Modifier.fillMaxSize(),
        ) {
            LazyColumn(Modifier.fillMaxSize().padding(top = 6.dp)) {
                projectSection(
                    rows = rows,
                    anyProject = projects.isNotEmpty(),
                    minuteTick = minuteTick,
                    busy = busy,
                    currentPath = currentPath,
                    agents = agents,
                    onOpen = { row -> open(row) },
                    onLongPress = { p -> actionsFor = p },
                    onSwapAgent = { path, next ->
                        scope.launch {
                            runCatching { store.client?.setProjectAgent(path, next) }
                                .onSuccess { store.refreshProjects() }
                                .onFailure { toast("失败：${it.message}") }
                        }
                    },
                )
                // 终端与会话平级（2026-09-08 用户拍板）：项目列表下面直接是终端列表，
                // 底部一行「新增终端」。以前它藏在顶栏一个 `>_` 按钮后面，是另一个世界。
                terminalSection(
                    terminals = terminals,
                    root = root,
                    creatingTerminal = creatingTerminal,
                    currentTerminalId = currentTerminalId,
                    onOpen = { t ->
                        // 点下去就把 attach 拉起来，等屏幕组合完 replay 往往已经到了
                        store.prewarmAttachment(t.id)
                        onBeforeNavigate(); nav.openTerminal(t.id)
                    },
                    onClose = { t -> store.closeTerminal(t.id) },
                    onNew = { newTerminal() },
                )
            }
        }
    }

    actionsFor?.let { p ->
        ProjectActionsSheet(store, nav, p, onDismiss = { actionsFor = null }, onPurged = { purgeReport = it }, onBeforeNavigate = onBeforeNavigate)
    }
    purgeReport?.let { results -> PurgeReportDialog(results) { purgeReport = null } }
    if (planDialog) plan?.let { PlanUsageDialog(it) { planDialog = false } }
}

/**
 * 列表的项目一节：空态 / 一项目一行 / 底下一句用法提示。
 *
 * 写成 `LazyListScope` 的扩展而不是 `@Composable`：这一节是**若干个 item**（空态一项、项目
 * 若干项、提示一项），包进一个 composable 会把它们压成列表里的一项，行的复用和
 * `animateItem` 的位移过渡都跟着没了。
 *
 * [minuteTick] 一分钟跳一次，只为让「N 分钟前」那一个 Text 自己会走：整套系统闲着时没有
 * 任何推送，时间字会一直停在「刚刚」。
 */
private fun LazyListScope.projectSection(
    rows: List<ProjectRow>,
    /** 有没有项目（区分「一个都没有」和「被搜索词滤没了」两种空） */
    anyProject: Boolean,
    minuteTick: Long,
    /** 正在 POST /sessions 的项目路径，行先按「在跑」画淡蓝底 */
    busy: Set<String>,
    currentPath: String?,
    /** agent 表：装了两个以上时每行尾部才画那个字母小标 */
    agents: List<AgentInfo>,
    onOpen: (ProjectRow) -> Unit,
    onLongPress: (Project) -> Unit,
    onSwapAgent: (String, String) -> Unit,
) {
    // 空态也是列表里的一项：下面还有终端一节，浮一层居中文字会盖住它
    if (rows.isEmpty()) item(key = "projects-empty") { ProjectsEmpty(anyProject = anyProject) }
    items(rows, key = { it.project.path }) { row ->
        // 顺序随最近更新变：Compose 按 key 做位移过渡，上移/下移都有动画
        // 这一行下次开谁：注册表里登记的，没登记就按默认（表里第一个装了的）算——
        // 直接给 `agentLabel("")` 会让每一行都常驻一个原样回显的空标签
        val agent = row.project.agent?.takeIf { it.isNotEmpty() }
            ?: agents.firstOrNull { it.available }?.id ?: DEFAULT_AGENT
        val next = if (row.project.registered) nextAgent(agents, agent) else null
        ProjectRowItem(
            row,
            timeText = remember(row.updatedIso, minuteTick) { relativeTime(row.updatedIso) },
            modifier = Modifier.animateItem(),
            busy = row.project.path in busy,
            current = row.project.path == currentPath,
            agentMark = next?.let { agentLabel(agents, agent).take(1) },
            onSwapAgent = next?.let { { onSwapAgent(row.project.path, it) } },
            onClick = { onOpen(row) },
            onLongClick = { onLongPress(row.project) },
        )
    }
    if (rows.isNotEmpty()) item(key = "projects-hint") {
        Text(
            "点一行进入消息流 · 长按查看项目操作",
            color = Tok.Faint, fontSize = 11.sp,
            modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 4.dp),
        )
    }
}

/** 列表的终端一节：表头 / 一终端一行 / 最后一行「＋ 新增终端」。分节的理由同 [projectSection]。 */
private fun LazyListScope.terminalSection(
    terminals: List<Session>,
    /** 项目根，用来把终端的工作目录缩成一个短名字 */
    root: String,
    creatingTerminal: Boolean,
    /** 正在看的那个终端（终端屏才有），强调色标题 + 整行边框 */
    currentTerminalId: String? = null,
    onOpen: (Session) -> Unit,
    onClose: (Session) -> Unit,
    onNew: () -> Unit,
) {
    item(key = "terminals-hdr") { TerminalsHeader() }
    itemsIndexed(terminals, key = { _, t -> "term-" + t.id }) { i, t ->
        TerminalRowItem(
            terminalTabLabel(i, t, root),
            modifier = Modifier.animateItem(),
            current = t.id == currentTerminalId,
            onClick = { onOpen(t) },
            onClose = { onClose(t) },
        )
    }
    item(key = "terminal-new") { NewTerminalRow(creatingTerminal, onNew) }
}

/**
 * 降级横幅（PROTOCOL「版本兼容」）：schema 是唯一的闸门，且降级必须说出来。老 daemon 不给
 * status / title / session_id，这一屏就画不出项目状态——**不留第二套算法**，留着就等于把刚
 * 删掉的分歧又养回来（daemon 与 mac App 同机同版发布，只有手机可能先更新，是几分钟的窗口）。
 */
@Composable
private fun SchemaTooOldBanner(version: String) {
    Text(
        "daemon 版本过旧（v$version），项目状态不可用 —— 请更新 daemon",
        color = Tok.Amber, fontSize = 12.sp,
        modifier = Modifier.fillMaxWidth()
            .background(Tok.Amber.copy(alpha = 0.10f))
            .padding(horizontal = 16.dp, vertical = 8.dp),
    )
}

/**
 * 面板顶栏一行（2026-09-08 用户拍板砍到三样）：额度 / 看板 / 设置。去掉了 "AAA" 标题和
 * IP · 延迟——app 只有一个，标题是废话；IP 和 ms 连着好的时候没人看。连接**不**正常时左边
 * 那一格改写连接状态：断了得说一声，但这不值得常年占一整行。
 */
@Composable
private fun ProjectPanelHeader(
    conn: ConnState,
    /** 套餐用量的各段，空 = 没数据（或 daemon 不给），那一格就空着 */
    planSegs: List<UsageSegment>,
    /** SSD 掉了是事故，才值得占顶栏；正常时不显示 */
    ssdMissing: Boolean,
    onPlanClick: () -> Unit,
    onHistory: () -> Unit,
    onSettings: () -> Unit,
) {
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 4.dp, top = 2.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f)) {
            when (conn) {
                is ConnState.Connected ->
                    // 套餐用量：5h / 7d / 按模型，最高的那个 ≥70 琥珀、≥90 红；点开看重置时间
                    if (planSegs.isNotEmpty()) Text(
                        segmentsAnnotated(planSegs, Tok.Faint),
                        fontSize = 11.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.fillMaxWidth().clickable(onClick = onPlanClick),
                    )
                is ConnState.Connecting -> DotWithText(Tok.Amber, "连接中…")
                is ConnState.Failed -> DotWithText(Tok.Red, "已断开")
                ConnState.NoServer -> DotWithText(Tok.Dim, "未配对")
            }
        }
        if (ssdMissing) {
            Text("SSD ✗", color = Tok.Red, fontSize = 12.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.width(4.dp))
        }
        IconButton(onClick = onHistory) {
            Text("▦", color = Tok.Dim, fontSize = 17.sp)
        }
        IconButton(onClick = onSettings) {
            Text("⚙", color = Tok.Dim, fontSize = 20.sp)
        }
    }
}

/**
 * 项目列表的空态。它是列表里的**一项**而不是浮在中间的一层字：下面还有终端一节，
 * 居中浮层会盖住它。[anyProject] 分开两种空：一个项目都没有，还是搜索词把它们滤没了。
 */
@Composable
private fun ProjectsEmpty(anyProject: Boolean) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 28.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        if (!anyProject) {
            Text("暂无项目", color = Tok.Faint)
            Text("在上方输入文件夹名，回车新建", color = Tok.Faint, fontSize = 12.sp)
        } else {
            Text("没有匹配的项目", color = Tok.Faint)
        }
    }
}

/**
 * 终端一节的表头：一条分隔线 + 「终端」两个字。终端与会话平级（2026-09-08 用户拍板），
 * 所以它只是项目列表下面的一节，不是另一个页面。
 */
@Composable
private fun TerminalsHeader() {
    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(top = 10.dp))
    Text(
        "终端",
        color = Tok.Dim, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
    )
}

/**
 * 列表最后一行「＋ 新增终端」，以及它下面那 80dp 留白——留白是给会话页右下角那枚悬浮
 * 按钮让位的，滚到底时最后一行不该被它盖住。
 */
@Composable
private fun NewTerminalRow(creating: Boolean, onClick: () -> Unit) {
    Text(
        if (creating) "＋ 新增终端…" else "＋ 新增终端",
        color = if (creating) Tok.Faint else Tok.Accent, fontSize = 13.5.sp,
        modifier = Modifier.fillMaxWidth().clickable(enabled = !creating, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
    )
    Spacer(Modifier.height(80.dp))
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
    AaaDialog("套餐用量", onDismiss, confirmLabel = "关闭", confirmColor = Tok.Dim) {
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
    }
}

/**
 * 一行到底：标题 + 更新时间。不再有第二行——目录大小等细节在长按单里。状态由整行的淡底色说
 * （[rowBackground]，2026-09-10 用户拍板），行上不画任何记号；选中的那一行标题用强调色、整行
 * 套一圈强调色边框——底色归状态用了，选中态不能再拿整行底色去抢它（此前是 `Raised` 底）。
 * 边框画在自己的边界内，不占布局，所以选中与否行高一样。
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ProjectRowItem(
    row: ProjectRow,
    timeText: String,
    busy: Boolean,
    current: Boolean = false,
    /** 这个项目下次开谁的首字母；null = 只装了一个 agent（或这一行换不了），整个不画 */
    agentMark: String? = null,
    onSwapAgent: (() -> Unit)? = null,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier) {
        Row(
            Modifier.fillMaxWidth()
                .background(rowBackground(row.status, row.unread, busy) ?: Color.Transparent)
                .border(1.dp, if (current) Tok.Accent else Color.Transparent)
                .combinedClickable(onClick = onClick, onLongClick = onLongClick)
                .padding(horizontal = 16.dp, vertical = 11.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                row.title,
                color = if (current) Tok.Accent else if (row.alive) Tok.Ink else Tok.Dim,
                fontSize = 15.sp, fontWeight = FontWeight.Bold,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            // agent 字母小标：一列扫下来看得出哪几行不是默认那个。点它换「下次开谁」——
            // 与 mac 同一处入口（长按单里那一条仍在，两条路同一个动作）。
            // v1.30 前这里什么都不画（mac 有悬停、手机没有，所以只给了长按单）；
            // mac 那边改成常显之后，这条不对称的理由就没有了
            if (agentMark != null && onSwapAgent != null) {
                Spacer(Modifier.width(6.dp))
                Box(
                    Modifier.size(24.dp).clickable(onClick = onSwapAgent),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(agentMark, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                }
            }
            Spacer(Modifier.width(8.dp))
            Text(timeText, color = Tok.Faint, fontSize = 11.sp)
        }
        HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 16.dp))
    }
}

/**
 * 终端一行：`终端 N · 目录名` + 行尾一个小 ×。2026-09-08 用户拍板「终端列表前面不需要三道杠」
 * 「android终端列表高度太高了」：记号拿掉——终端没有状态可言，三道横杠只是占着行首那一格；
 * 标题就此顶格起，上下各 6dp、字号降一档，终端不是项目，它该比项目行更矮。行尾的 × 换成一个
 * 28dp 见方的可点 Box，IconButton 那 48dp 的触摸区本身就把整行撑得比项目行还高。
 *
 * 终端行没有状态底色（终端没有状态可言）；[current] = 正在看的那个终端，标题跟项目行用同一
 * 套选中语言：强调色 + 整行一圈强调色边框。
 */
@Composable
private fun TerminalRowItem(label: String, onClick: () -> Unit, onClose: () -> Unit, current: Boolean = false, modifier: Modifier = Modifier) {
    Column(modifier) {
        Row(
            Modifier.fillMaxWidth()
                .border(1.dp, if (current) Tok.Accent else Color.Transparent)
                .clickable(onClick = onClick)
                .padding(start = 16.dp, end = 8.dp, top = 6.dp, bottom = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                label,
                color = if (current) Tok.Accent else Tok.Ink,
                fontSize = 13.5.sp,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Box(Modifier.size(28.dp).clickable(onClick = onClose), contentAlignment = Alignment.Center) {
                Text("×", color = Tok.Faint, fontSize = 16.sp)
            }
        }
        HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 16.dp))
    }
}

/** A8 删除结果（purge 报告渲染） */
@Composable
fun PurgeReportDialog(results: List<ProjectDeleteResult>, onDismiss: () -> Unit) {
    AaaDialog("删除完成", onDismiss, confirmLabel = "好") {
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
    }
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
    onBeforeNavigate: () -> Unit = {},
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val openSession = LocalOpenSession.current
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val sessions by store.sessions.collectAsState()
    val agents by store.agents.collectAsState()
    // 代表会话是 daemon 指的（`session_id`）；这里只按 id 去池子里取那条会话，取不到就当它不在池子里
    val primary = p.session_id?.let { id -> sessions.firstOrNull { it.id == id } }
    var deleteConfirm by remember { mutableStateOf(false) }
    var killConfirm by remember { mutableStateOf(false) }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
    // 激活与否读 daemon 的 status（没有活会话 = paused），不再自己看会话的 state
    val alive = p.status.isNotBlank() && p.status != "paused"
    /** 结束会话 = 项目回到未激活栏。用户自己动的手，随后的 exited 不弹通知。 */
    fun killPrimary() {
        val id = p.session_id ?: return
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
                    if (p.status == "running") killConfirm = true else killPrimary()
                }
            }
            // 注册表里没有的目录不能 resume（daemon 不认它），删项目同理——都收起来，
            // 只留「它此刻这个会话」能做的事
            if (!alive && p.registered) SheetItem("▶", "继续会话", "resume") {
                val agent = p.agent ?: DEFAULT_AGENT
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, agent, resume = true) ?: return@launch
                        onDismiss(); onBeforeNavigate(); openSession(sess.id, "")
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
            }
            if (!alive && primary != null) {
                // 点行 = resume 新会话；上一条已退出的会话只要还在池子里就留着 transcript 回放入口
                SheetItem("↺", "上次会话回放", "消息流 · 终端回放") { onDismiss(); onBeforeNavigate(); openSession(primary.id, "") }
            }
            // 换 agent：只改注册表里的一行「下次开谁」，活着的会话不碰。
            // 没登记的目录换不了（daemon 认的就是注册表）。
            if (p.registered) nextAgent(agents, p.agent ?: DEFAULT_AGENT)?.let { next ->
                SheetItem("⇄", "换成 ${agentLabel(agents, next)}", "下次开会话时生效") {
                    scope.launch {
                        runCatching { store.client?.setProjectAgent(p.path, next) }
                            .onSuccess { store.refreshProjects() }
                            .onFailure { toast("失败：${it.message}") }
                    }
                    onDismiss()
                }
            }
            SheetItem("＞", "在此目录开终端", "zsh") {
                scope.launch {
                    try {
                        val sess = store.client?.createSession(p.path, "shell", resume = false, fresh = true) ?: return@launch
                        onDismiss(); onBeforeNavigate(); nav.openTerminal(sess.id)
                    } catch (e: Exception) { toast("失败：${e.message}") }
                }
            }
            if (p.registered) SheetItem("🗑", "删除项目…", "目录 + 全部会话", danger = true) { deleteConfirm = true }
            else Text(
                "这个目录不在项目注册表里（在别处 aaa open 开出来的），只能操作它此刻的会话",
                color = Tok.Faint, fontSize = 11.sp,
                modifier = Modifier.padding(horizontal = 18.dp, vertical = 6.dp),
            )
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
            "将删除目录（${humanBytes(p.dir_size)}）并清除所有 agent 的会话存储。与 aaa CLI 的 d 行为一致，不可恢复。" +
                if (alive) "\n\n该项目还有会话在跑，daemon 会先结束它——否则目录没了、进程还活着。" else "",
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
