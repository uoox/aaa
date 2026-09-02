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
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
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
            // 折叠/展开只是配置变化（manifest 里已接管），整棵 composition 不重建。
            // 首页是单栏项目列表，宽窄屏同一套布局，不再按窗口宽度切导航位置。
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

    // 一个入口：会话卡片、通知深链、项目页「继续会话」都走这里，行为不会各走各的。
    val openSession: (String, String) -> Unit = { id, prefill ->
        nav.navigate("session/$id?prefill=${Uri.encode(prefill)}") { launchSingleTop = true }
    }

    // deep link from notifications
    LaunchedEffect(pendingSessionId.value, loaded) {
        val id = pendingSessionId.value
        if (loaded && id != null) {
            pendingSessionId.value = null
            val prefill = pendingPrefill.value.orEmpty(); pendingPrefill.value = null
            openSession(id, prefill)
        }
    }

    CompositionLocalProvider(LocalOpenSession provides openSession) {
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
            composable("home") { HomeScreen(store, nav) }
            composable("settings") { SettingsScreen(store, nav) }
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

// ---------- home：项目列表就是首页 ----------

/**
 * 首页只有一屏：项目列表（含每个项目的会话三态），右上角齿轮进设置，右下角 ＋ 新建。
 * 原来的「会话 / 项目 / 设置」三 tab 收掉了——一个项目一个 agent，项目即会话，
 * 两张列表说的是同一件事。宽屏也不摆 rail：没有 tab 就没有导航可放。
 */
@Composable
fun HomeScreen(store: AppStore, nav: NavHostController) {
    var showNewSheet by remember { mutableStateOf(false) }

    // POST_NOTIFICATIONS runtime permission (Android 13+)
    val permLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= 33) permLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
    }

    Box(Modifier.fillMaxSize().background(Tok.Bg)) {
        ProjectsHome(store, nav)
        // safeDrawingPadding：手势条 / 展开态横屏的系统栏不吃掉 FAB
        Box(
            Modifier.align(Alignment.BottomEnd)
                .safeDrawingPadding()
                .padding(20.dp),
        ) {
            FloatingActionButton(onClick = { showNewSheet = true }, containerColor = Tok.Cyan) {
                Text("＋", color = Color(0xFF08252C), fontSize = 24.sp)
            }
        }
    }
    if (showNewSheet) NewSessionSheet(store, nav, initialPath = null) { showNewSheet = false }
}

// ---------- A5 新建（也用于「用其它 agent 打开」） ----------

private val NEW_AGENTS = listOf("claude", "codex", "pi", "reasonix", "agy", "shell")

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionSheet(store: AppStore, nav: NavHostController, initialPath: String?, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    val openSession = LocalOpenSession.current
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
                            // 连 shell 也写进注册表：不写的话 daemon 那边「没有登记」
                            // 会回落到默认的 claude，从普通终端建的文件夹一转头就
                            // 变成 claude 项目了。改 agent 只该由人来做。
                            val path = initialPath ?: api.createProject(
                                name.trim().ifBlank { null },
                                selected,
                            ).path
                            val sess = api.createSession(path, selected, resume = initialPath != null)
                            onDismiss()
                            openSession(sess.id, "")
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
