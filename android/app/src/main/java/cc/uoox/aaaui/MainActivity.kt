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
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import androidx.compose.ui.Modifier
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
            AaaTheme(store) { AaaApp(store, pendingSessionId, pendingPrefill) }
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

    // 一个入口：会话卡片、通知深链、项目页「继续会话」、会话页 ☰ 切换都走这里，行为不会各走各的。
    // 回退栈始终是 home → 当前会话：从一个会话切到另一个不叠页，返回键直接回首页
    val scope = rememberCoroutineScope()
    val openSession: (String, String) -> Unit = { id, prefill ->
        // 记住：下次 app 起来直接回这个会话（没有首页了）
        autoOpenedLastSession = true
        scope.launch { store.settings.setLastSession(id) }
        nav.navigate("session/$id?prefill=${Uri.encode(prefill)}") {
            launchSingleTop = true
            popUpTo("home")
        }
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
            composable("history") { HistoryScreen(store, nav) }
            composable("detail/{id}") { entry ->
                SessionDetailScreen(store, nav, entry.arguments?.getString("id").orEmpty())
            }
            composable("terminal?focus={focus}", arguments = listOf(androidx.navigation.navArgument("focus") { defaultValue = "" })) { entry ->
                TerminalScreen(store, nav, focusId = entry.arguments?.getString("focus").orEmpty())
            }
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
        }
    }
}

/** 会话页右上角的详情按钮（顶替了原来的 ⋮） */
fun NavHostController.openDetail(sessionId: String) {
    navigate("detail/" + Uri.encode(sessionId)) { launchSingleTop = true }
}

fun NavHostController.openTerminal(focusId: String? = null) {
    navigate("terminal?focus=${Uri.encode(focusId ?: "")}") { launchSingleTop = true }
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
 * 首页只有一屏：项目列表（含每个项目的会话三态），右上角齿轮进设置，顶部输入框
 * 既过滤列表也新建项目（输入文件夹名回车，与 mac 侧栏一致）。原来的「会话 / 项目 /
 * 设置」三 tab 和右下角 ＋ 都收掉了——一个项目一个 agent，项目即会话。
 * 2026-09-07：连这一屏也不再独立存在，见下面 HomeScreen。
 */

/** 进程内只自动回一次最近的会话：之后用户按返回回到落地页，不再被弹回去 */
private var autoOpenedLastSession = false

/**
 * 落地页：没有独立的首页了（2026-09-07 用户拍板），这里画的就是 ☰ 抽屉那块项目面板——
 * 只在「一个会话都没打开」时出现（首次进入、按返回退出会话）。app 起来时若记着上次的
 * 会话且它还在，直接进去。
 */
@Composable
fun HomeScreen(store: AppStore, nav: NavHostController) {
    // POST_NOTIFICATIONS runtime permission (Android 13+)
    val permLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { }
    LaunchedEffect(Unit) {
        if (Build.VERSION.SDK_INT >= 33) permLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
    val openSession = LocalOpenSession.current
    val sessions by store.sessions.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = null)
    LaunchedEffect(sessions, settings) {
        val last = settings?.lastSession ?: return@LaunchedEffect
        if (!autoOpenedLastSession && sessions.any { it.id == last }) {
            autoOpenedLastSession = true
            openSession(last, "")
        }
    }
    Box(Modifier.fillMaxSize().background(Tok.Bg)) { ProjectPanel(store, nav) }
}

