package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.launch

/** 设置：从首页右上角齿轮压栈进来的独立路由，不再是 tab。 */
@Composable
fun SettingsScreen(store: AppStore, nav: NavHostController) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val conn by store.connState.collectAsState()
    val health by store.health.collectAsState()

    LaunchedEffect(conn) { if (conn is ConnState.Connected) store.refreshHealth() }

    Column(Modifier.fillMaxSize().background(Tok.Bg)) {
        // 顶栏：与 InboxScreen / SessionScreen 同一套「‹ + 标题」写法，不引 TopAppBar
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("设置", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {

        // ---------- 服务器 ----------
        GroupTitle("服务器")
        Group {
            val server = settings.server
            if (server == null) {
                SettingRow("未配对") { TextButton(onClick = { nav.navigate("pair") }) { Text("去配对") } }
            } else {
                // 每个 host 一行，右侧直接写状态——原来另有一行「连接状态」重复说
                // 同一件事，删掉了；连接失败时的重试按钮挪到当前 host 这一行上。
                server.hosts.forEach { host ->
                    val active = (conn as? ConnState.Connected)?.host == host
                    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        StateDot(if (active) Tok.Green else Tok.Dim)
                        Spacer(Modifier.width(8.dp))
                        Text(host, color = Tok.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
                        Text(
                            if (active) "已连接 · ${(conn as ConnState.Connected).latencyMs}ms" else "备用",
                            color = if (active) Tok.Green else Tok.Faint, fontSize = 11.sp,
                        )
                    }
                }
                when (val c = conn) {
                    is ConnState.Connecting -> SettingRow("连接中…") {}
                    is ConnState.Failed -> Row(
                        Modifier.fillMaxWidth().padding(start = 14.dp, end = 4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(c.message, color = Tok.Red, fontSize = 12.sp, modifier = Modifier.weight(1f))
                        TextButton(onClick = { store.kickReconnect() }) { Text("重试") }
                    }
                    else -> {}
                }
                SettingRow("Token ····${server.token.takeLast(4)}") {
                    TextButton(onClick = { nav.navigate("pair") }) { Text("重新配对") }
                }
            }
        }

        // ---------- 通知 ----------
        GroupTitle("通知")
        Group {
            ToggleRow("任务完成时通知", settings.notifyDone) { scope.launch { store.settings.setNotifyDone(it) } }
            ToggleRow("后台常驻（前台服务维持连接）", settings.serviceEnabled) { on ->
                scope.launch { store.settings.setServiceEnabled(on) }
                if (on) NotificationService.start(context) else NotificationService.stop(context)
            }
            if (settings.mutedProjects.isNotEmpty()) {
                Text("已静音项目", color = Tok.Faint, fontSize = 11.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp))
                settings.mutedProjects.forEach { path ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(path.substringAfterLast('/'), color = Tok.Dim, fontSize = 13.sp, modifier = Modifier.weight(1f))
                        TextButton(onClick = { scope.launch { store.settings.setProjectMuted(path, false) } }) { Text("取消静音", fontSize = 12.sp) }
                    }
                }
            }
        }

        // ---------- 界面 ----------
        GroupTitle("界面")
        Group {
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("会话默认视图", color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                SingleChoiceSegmentedButtonRow {
                    SegmentedButton(
                        selected = settings.defaultUi == "messages",
                        onClick = { scope.launch { store.settings.setDefaultUi("messages") } },
                        shape = SegmentedButtonDefaults.itemShape(0, 2),
                    ) { Text("消息流", fontSize = 12.sp) }
                    SegmentedButton(
                        selected = settings.defaultUi == "terminal",
                        onClick = { scope.launch { store.settings.setDefaultUi("terminal") } },
                        shape = SegmentedButtonDefaults.itemShape(1, 2),
                    ) { Text("终端", fontSize = 12.sp) }
                }
            }
            SettingRow("终端字号") {
                TextButton(onClick = { scope.launch { store.settings.setFontSize(settings.fontSize - 1) } }) { Text("−", fontSize = 18.sp) }
                Text("${settings.fontSize}", color = Tok.Ink, fontFamily = FontFamily.Monospace)
                TextButton(onClick = { scope.launch { store.settings.setFontSize(settings.fontSize + 1) } }) { Text("＋", fontSize = 16.sp) }
            }
        }

        // ---------- daemon 状态 ----------
        GroupTitle("daemon")
        Group {
            val h = health
            // 没连上时这四行全是「—」，与其摆四行破折号不如说清楚现在拿不到
            if (h == null) {
                SettingRow("尚未拿到 daemon 状态") {}
            } else {
                SettingRow("版本") { Text("v${h.version}", color = Tok.Dim, fontSize = 13.sp, fontFamily = FontFamily.Monospace) }
                SettingRow("SSD") {
                    Text(
                        if (h.ssd_mounted) "已挂载 ✓" else "未挂载 ✗",
                        color = if (h.ssd_mounted) Tok.Green else Tok.Red, fontSize = 13.sp,
                    )
                }
                SettingRow("项目根") { Text(h.project_root, color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
                SettingRow("运行时长") { Text(formatUptime(h.uptime_s), color = Tok.Dim, fontSize = 13.sp) }
            }
        }
        }
    }
}

@Composable
private fun GroupTitle(title: String) {
    Text(title, color = Tok.Faint, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp))
}

@Composable
private fun Group(content: @Composable () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
            .background(Tok.Surface, RoundedCornerShape(12.dp))
            .border(1.dp, Tok.Edge, RoundedCornerShape(12.dp))
            .padding(vertical = 6.dp),
    ) { content() }
}

@Composable
private fun SettingRow(label: String, trailing: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(label, color = Tok.Ink, fontSize = 14.sp)
        Row(verticalAlignment = Alignment.CenterVertically) { trailing() }
    }
}

@Composable
private fun ToggleRow(label: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
        Switch(value, onChange)
    }
}

private fun formatUptime(s: Long): String = when {
    s < 3600 -> "${s / 60} 分钟"
    s < 86400 -> "${s / 3600} 小时"
    else -> "${s / 86400} 天"
}
