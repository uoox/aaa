package cc.uoox.aaaui

import android.Manifest
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
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
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val pendingSessionId = mutableStateOf<String?>(null)
    private val pendingPrefill = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val store = AppStore.get(this)
        store.ensureStarted()
        consumeIntent(intent)
        setContent {
            AaaTheme { AaaApp(store, pendingSessionId, pendingPrefill) }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        consumeIntent(intent)
    }

    private fun consumeIntent(intent: Intent?) {
        intent?.getStringExtra(EXTRA_SESSION_ID)?.let { pendingSessionId.value = it }
        intent?.getStringExtra(EXTRA_PREFILL)?.let { pendingPrefill.value = it }
    }

    companion object {
        const val EXTRA_SESSION_ID = "cc.uoox.aaaui.SESSION_ID"
        const val EXTRA_PREFILL = "cc.uoox.aaaui.PREFILL"
    }
}

@Composable
fun AaaApp(
    store: AppStore,
    pendingSessionId: androidx.compose.runtime.MutableState<String?>,
    pendingPrefill: androidx.compose.runtime.MutableState<String?>,
) {
    val nav = rememberNavController()
    val settings by store.settings.flow.collectAsState(initial = null)
    val loaded = settings != null

    // deep link from notifications
    LaunchedEffect(pendingSessionId.value, loaded) {
        val id = pendingSessionId.value
        if (loaded && id != null) {
            pendingSessionId.value = null
            val prefill = pendingPrefill.value.orEmpty(); pendingPrefill.value = null
            nav.navigate("session/$id?prefill=${Uri.encode(prefill)}") { launchSingleTop = true }
        }
    }

    NavHost(nav, startDestination = "gate") {
        composable("gate") {
            LaunchedEffect(loaded) {
                if (loaded) {
                    if (settings?.server == null) nav.navigate("pair") { popUpTo("gate") { inclusive = true } }
                    else nav.navigate("home") { popUpTo("gate") { inclusive = true } }
                }
            }
            Box(Modifier.fillMaxSize().background(Tok.Bg))
        }
        composable("pair") { PairScreen(store) { nav.navigate("home") { popUpTo("pair") { inclusive = true } } } }
        composable("home") { HomeScaffold(store, nav) }
        composable(
            "session/{id}?prefill={prefill}",
            arguments = listOf(
                androidx.navigation.navArgument("prefill") { type = androidx.navigation.NavType.StringType; defaultValue = "" },
            ),
        ) { entry ->
            val id = entry.arguments?.getString("id").orEmpty()
            val prefill = entry.arguments?.getString("prefill").orEmpty()
            SessionScreen(store, nav, id, Uri.decode(prefill))
        }
        composable("diff/{id}") { entry ->
            DiffScreen(store, nav, entry.arguments?.getString("id").orEmpty())
        }
        composable("inbox/{path}") { entry ->
            InboxScreen(store, nav, Uri.decode(entry.arguments?.getString("path").orEmpty()))
        }
    }
}

// ---------- A1 配对 ----------

@Composable
fun PairScreen(store: AppStore, onConnected: () -> Unit) {
    val scope = rememberCoroutineScope()
    val conn by store.connState.collectAsState()
    var manualHost by rememberSaveable { mutableStateOf("") }
    var manualToken by rememberSaveable { mutableStateOf("") }
    var showManual by rememberSaveable { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    // 只有本屏发起过连接尝试才自动跳走——否则「重新配对」进来会被立刻弹回
    var attempted by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(conn, attempted) { if (attempted && conn is ConnState.Connected) onConnected() }

    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        val raw = result.contents ?: return@rememberLauncherForActivityResult
        try {
            val p = PairPayload.parse(raw)
            error = null
            attempted = true
            scope.launch { store.applyServer(ServerConfig(p.name, p.hosts, p.token)) }
        } catch (e: Exception) {
            error = "二维码无效：${e.message}"
        }
    }

    Column(
        Modifier.fillMaxSize().background(Tok.Bg).padding(24.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("AAA-UI", color = Tok.Magenta, fontSize = 34.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
        Text("连接 Mac 上的 aaa-daemon", color = Tok.Faint, fontFamily = FontFamily.Monospace, fontSize = 13.sp)
        Spacer(Modifier.height(28.dp))

        when (val c = conn) {
            is ConnState.Connecting -> DotWithText(Tok.Amber, "正在连接 ${c.host ?: "…"}")
            is ConnState.Failed -> Text("连接失败：${c.message}", color = Tok.Red, fontSize = 13.sp)
            is ConnState.Connected -> DotWithText(Tok.Green, "已连接 ${c.host} · ${c.latencyMs}ms")
            ConnState.NoServer -> Text("尚未配对", color = Tok.Dim, fontSize = 13.sp)
        }
        error?.let { Spacer(Modifier.height(8.dp)); Text(it, color = Tok.Red, fontSize = 13.sp) }
        Spacer(Modifier.height(20.dp))

        Button(
            onClick = {
                scanLauncher.launch(ScanOptions().apply {
                    setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                    setPrompt("扫描 Mac 端设置页的配对二维码")
                    setBeepEnabled(false)
                    setOrientationLocked(true)
                })
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("扫码配对（Mac 设置页出码）") }

        Spacer(Modifier.height(10.dp))
        OutlinedButton(onClick = { showManual = !showManual }, modifier = Modifier.fillMaxWidth()) {
            Text("手动输入 host:port 与 token")
        }

        if (showManual) {
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                manualHost, { manualHost = it },
                label = { Text("host:port（多个用逗号分隔）") },
                placeholder = { Text("mac-mini.tailxxxx.ts.net:2730") },
                modifier = Modifier.fillMaxWidth(), singleLine = true,
                keyboardOptions = KeyboardOptions.Default,
            )
            Spacer(Modifier.height(8.dp))
            OutlinedTextField(
                manualToken, { manualToken = it },
                label = { Text("token") }, placeholder = { Text("aaa_tk_…") },
                modifier = Modifier.fillMaxWidth(), singleLine = true,
            )
            Spacer(Modifier.height(10.dp))
            Button(
                onClick = {
                    val hosts = manualHost.split(',').map { it.trim() }.filter { it.isNotEmpty() }
                    if (hosts.isEmpty() || manualToken.isBlank()) { error = "请填写 host 与 token"; return@Button }
                    error = null
                    attempted = true
                    scope.launch { store.applyServer(ServerConfig("manual", hosts, manualToken.trim())) }
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text("连接 →") }
        }

        Spacer(Modifier.height(24.dp))
        Text(
            "daemon 仅监听内网 overlay 网卡\n链路经 WireGuard 加密，Token 防误连",
            color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
        )
    }
}

// ---------- home（会话 / 项目 / 设置 三 tab） ----------

@Composable
fun HomeScaffold(store: AppStore, nav: NavHostController) {
    var tab by rememberSaveable { mutableStateOf(0) }
    var showNewSheet by remember { mutableStateOf(false) }

    // POST_NOTIFICATIONS runtime permission (Android 13+)
    val permLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= 33) permLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    Scaffold(
        containerColor = Tok.Bg,
        bottomBar = {
            NavigationBar(containerColor = Tok.Surface) {
                NavigationBarItem(tab == 0, { tab = 0 }, icon = { Text("▣", fontSize = 16.sp) }, label = { Text("会话") })
                NavigationBarItem(tab == 1, { tab = 1 }, icon = { Text("▤", fontSize = 16.sp) }, label = { Text("项目") })
                NavigationBarItem(tab == 2, { tab = 2 }, icon = { Text("⚙", fontSize = 16.sp) }, label = { Text("设置") })
            }
        },
        floatingActionButton = {
            if (tab == 0) FloatingActionButton(onClick = { showNewSheet = true }, containerColor = Tok.Cyan) {
                Text("＋", color = Color(0xFF08252C), fontSize = 24.sp)
            }
        },
    ) { pad ->
        Box(Modifier.padding(pad)) {
            when (tab) {
                0 -> SessionsTab(store, nav)
                1 -> ProjectsTab(store, nav)
                2 -> SettingsTab(store, nav)
            }
        }
    }
    if (showNewSheet) NewSessionSheet(store, nav, initialPath = null) { showNewSheet = false }
}

// ---------- A2 会话首页 ----------

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionsTab(store: AppStore, nav: NavHostController) {
    val scope = rememberCoroutineScope()
    val sessions by store.sessions.collectAsState()
    val conn by store.connState.collectAsState()
    var filter by rememberSaveable { mutableStateOf("all") }
    var refreshing by remember { mutableStateOf(false) }

    val stateRank = mapOf("waiting" to 0, "running" to 1, "idle" to 2, "exited" to 3)
    val sorted = sessions.sortedWith(compareBy<Session> { stateRank[it.state] ?: 4 }.thenByDescending { it.last_output_at })
    val filtered = when (filter) {
        "waiting" -> sorted.filter { it.state == "waiting" }
        "running" -> sorted.filter { it.state == "running" }
        else -> sorted
    }
    val waitingCount = sessions.count { it.state == "waiting" }
    val runningCount = sessions.count { it.state == "running" }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
            horizontalArrangement = Arrangement.SpaceBetween, verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("会话", color = Tok.Ink, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            when (val c = conn) {
                is ConnState.Connected -> DotWithText(Tok.Green, "${c.host.substringBefore(':')} · ${c.latencyMs}ms")
                is ConnState.Connecting -> DotWithText(Tok.Amber, "连接中…")
                is ConnState.Failed -> DotWithText(Tok.Red, "已断开")
                ConnState.NoServer -> DotWithText(Tok.Dim, "未配对")
            }
        }

        sessions.firstOrNull { it.state == "waiting" }?.let { w ->
            Card(
                onClick = { nav.navigate("session/${w.id}") },
                colors = CardDefaults.cardColors(containerColor = Tok.Amber.copy(alpha = 0.12f)),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 2.dp),
            ) {
                Column(Modifier.padding(12.dp)) {
                    Text("⚡ ${Tok.agentLabel(w.agent)} 等待输入 · ${w.project_name}", color = Tok.Amber, fontSize = 13.sp, fontWeight = FontWeight.Bold)
                    w.question?.let { Text(it.text, color = Tok.Ink, fontSize = 13.sp, modifier = Modifier.padding(top = 3.dp)) }
                }
            }
        }

        Row(Modifier.padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            FilterChip(filter == "all", { filter = "all" }, label = { Text("全部 ${sessions.size}") })
            FilterChip(filter == "waiting", { filter = "waiting" }, label = { DotWithText(Tok.Amber, "等待输入 $waitingCount", Tok.Ink) })
            FilterChip(filter == "running", { filter = "running" }, label = { DotWithText(Tok.Green, "运行中 $runningCount", Tok.Ink) })
        }

        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch { store.refreshSessions(); store.refreshHealth(); refreshing = false }
            },
            modifier = Modifier.fillMaxSize(),
        ) {
            if (filtered.isEmpty()) {
                Column(Modifier.fillMaxSize(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center) {
                    Text("暂无会话", color = Tok.Faint)
                    Text("点右下 ＋ 新建", color = Tok.Faint, fontSize = 12.sp)
                }
            }
            LazyColumn(Modifier.fillMaxSize()) {
                items(filtered, key = { it.id }) { s -> SessionCard(s) { nav.navigate("session/${s.id}") } }
                item { Spacer(Modifier.height(80.dp)) }
            }
        }
    }
}

@Composable
fun SessionCard(s: Session, onClick: () -> Unit) {
    val stateColor = Tok.stateColor(s.state)
    val isWaiting = s.state == "waiting"
    Card(
        onClick = onClick,
        colors = CardDefaults.cardColors(containerColor = if (isWaiting) Tok.Raised else Tok.Surface),
        border = if (isWaiting) androidx.compose.foundation.BorderStroke(1.dp, Tok.Amber.copy(alpha = 0.6f)) else null,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 5.dp),
    ) {
        Column(Modifier.padding(13.dp).let { if (s.state == "exited") it.background(Color.Transparent) else it }) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                StateDot(stateColor)
                Spacer(Modifier.width(8.dp))
                Text(
                    s.title.ifBlank { s.project_name }, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
                )
                AgentChip(s.agent)
            }
            val stateText = when (s.state) {
                "running" -> "运行中 · ${relativeTime(s.last_output_at)}有输出"
                "waiting" -> "等待输入"
                "idle" -> "空闲 · ${relativeTime(s.last_output_at)}"
                "exited" -> "已退出 · 回放已保留"
                else -> s.state
            }
            Row(Modifier.padding(top = 3.dp)) {
                Text(s.project_name, color = Tok.Dim, fontSize = 12.sp)
                s.resume_id?.let { Text(" · resume ${it.take(6)}", color = Tok.Dim, fontSize = 12.sp) }
                Text(" · ", color = Tok.Dim, fontSize = 12.sp)
                Text(stateText, color = if (isWaiting) Tok.Amber else Tok.Dim, fontSize = 12.sp)
            }
            if (s.preview.isNotBlank() && s.state != "exited") {
                Text(
                    s.preview.trimEnd(), color = Tok.Dim, fontFamily = FontFamily.Monospace, fontSize = 11.sp,
                    maxLines = 4, overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.padding(top = 8.dp).fillMaxWidth()
                        .background(Tok.TermBg, RoundedCornerShape(8.dp)).padding(8.dp),
                )
            }
            s.question?.let { q ->
                if (isWaiting && q.options.isNotEmpty()) {
                    Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        q.options.take(3).forEach { opt ->
                            Text(
                                "${opt.key} · ${opt.label}", color = Tok.Cyan, fontSize = 12.sp,
                                modifier = Modifier.background(Tok.Cyan.copy(alpha = 0.12f), RoundedCornerShape(50)).padding(horizontal = 10.dp, vertical = 3.dp),
                            )
                        }
                    }
                }
            }
        }
    }
}

// ---------- A5 新建（也用于「用其它 agent 打开」） ----------

private val NEW_AGENTS = listOf("claude", "codex", "pi", "reasonix", "agy", "shell")

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionSheet(store: AppStore, nav: NavHostController, initialPath: String?, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var name by rememberSaveable { mutableStateOf("") }
    var selected by rememberSaveable { mutableStateOf("claude") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var agents by remember { mutableStateOf<List<Agent>>(emptyList()) }
    LaunchedEffect(Unit) { runCatching { store.client?.agents()?.let { agents = it } } }

    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = Tok.Surface) {
        Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 24.dp)) {
            Text(if (initialPath == null) "新建" else "用其它 agent 打开", color = Tok.Ink, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Text(
                initialPath ?: "${store.health.collectAsState().value?.project_root ?: "/Volumes/SSD/project"}/<名称>",
                color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
            )
            if (initialPath == null) {
                Spacer(Modifier.height(10.dp))
                OutlinedTextField(
                    name, { name = it }, label = { Text("项目名称") },
                    placeholder = { Text("留空 = 按日期命名") },
                    modifier = Modifier.fillMaxWidth(), singleLine = true,
                )
            }
            Spacer(Modifier.height(12.dp))
            NEW_AGENTS.chunked(2).forEach { rowAgents ->
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    rowAgents.forEach { ag ->
                        val available = agents.isEmpty() || agents.find { it.id == ag }?.available != false
                        val sel = selected == ag
                        Row(
                            Modifier.weight(1f).padding(vertical = 4.dp)
                                .background(if (sel) Tok.Cyan.copy(alpha = 0.08f) else Color.Transparent, RoundedCornerShape(11.dp))
                                .androidBorder(sel)
                                .clickable { selected = ag }
                                .padding(horizontal = 12.dp, vertical = 12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            AgentChip(ag)
                            Spacer(Modifier.width(8.dp))
                            Text(Tok.agentLabel(ag), color = if (available) Tok.Ink else Tok.Faint, fontSize = 14.sp)
                        }
                    }
                    if (rowAgents.size == 1) Spacer(Modifier.weight(1f))
                }
            }
            error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(top = 6.dp)) }
            Spacer(Modifier.height(14.dp))
            Button(
                enabled = !busy,
                onClick = {
                    busy = true; error = null
                    scope.launch {
                        try {
                            val api = store.client ?: throw IllegalStateException("未连接 daemon")
                            val path = initialPath ?: api.createProject(
                                name.trim().ifBlank { null },
                                if (selected == "shell") null else selected,
                            ).path
                            val sess = api.createSession(path, selected, resume = initialPath != null)
                            onDismiss()
                            nav.navigate("session/${sess.id}")
                        } catch (e: Exception) {
                            error = if (e is DaemonHttpException && e.errorCode == "conflict") "项目已存在" else e.message
                        } finally { busy = false }
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            ) { if (busy) CircularProgressIndicator(Modifier.height(18.dp).width(18.dp), strokeWidth = 2.dp) else Text("创建并进入 →") }
            TextButton(onClick = onDismiss, modifier = Modifier.fillMaxWidth()) { Text("取消", color = Tok.Dim) }
        }
    }
}

private fun Modifier.androidBorder(selected: Boolean): Modifier =
    this.border(1.dp, if (selected) Tok.Cyan else Tok.Edge, RoundedCornerShape(11.dp))
