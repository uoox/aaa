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
import androidx.compose.material3.AlertDialog
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import android.widget.Toast
import kotlinx.coroutines.delay
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

    val sessions by store.sessions.collectAsState()
    var restartDialog by remember { mutableStateOf(false) }

    LaunchedEffect(conn) { if (conn is ConnState.Connected) store.refreshHealth() }

    fun restartDaemon(force: Boolean) {
        scope.launch {
            try {
                store.client?.restartDaemon(force)
                Toast.makeText(context, "已发出重启，稍候自动重连", Toast.LENGTH_SHORT).show()
                delay(2500); store.refreshHealth(); store.refreshSessions()
            } catch (e: Exception) {
                Toast.makeText(context, "重启失败：${e.message}", Toast.LENGTH_LONG).show()
            }
        }
    }
    if (restartDialog) {
        val alive = sessions.count { it.state != "exited" }
        AlertDialog(
            onDismissRequest = { restartDialog = false },
            containerColor = Tok.Raised,
            title = { Text("重启 daemon？", color = Tok.Ink) },
            text = { Text("有 $alive 个会话还活着。daemon 的 PTY 都是它的子进程，重启会把它们一起终止；屏幕回放保留，之后可从项目行继续。", color = Tok.Dim) },
            confirmButton = { TextButton(onClick = { restartDialog = false; restartDaemon(true) }) { Text("终止并重启", color = Tok.Red) } },
            dismissButton = { TextButton(onClick = { restartDialog = false }) { Text("取消", color = Tok.Dim) } },
        )
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg)) {
        // 顶栏：与 InboxScreen / SessionScreen 同一套「‹ + 标题」写法，不引 TopAppBar
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("设置", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
        }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        // 一张卡、几行到底（2026-09-07 用户拍板：不再按 服务器/通知/外观/界面/daemon 分组）
        Group {
            // 服务器：当前地址 + 状态一行；token 尾号进副标题；备用地址折叠
            val server = settings.server
            if (server == null) {
                SettingRow("未配对") { TextButton(onClick = { nav.navigate("pair") }) { Text("去配对") } }
            } else {
                val current = (conn as? ConnState.Connected)?.host ?: server.preferredHost ?: server.hosts.firstOrNull()
                val backups = server.hosts.filter { it != current }
                var showBackups by remember { mutableStateOf(false) }
                val status = when (val c = conn) {
                    is ConnState.Connected -> "已连接 · ${c.latencyMs}ms"
                    is ConnState.Connecting -> "连接中…"
                    is ConnState.Failed -> "已断开"
                    ConnState.NoServer -> "未配对"
                }
                Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    StateDot(if (conn is ConnState.Connected) Tok.Green else if (conn is ConnState.Failed) Tok.Red else Tok.Amber)
                    Spacer(Modifier.width(8.dp))
                    Column(Modifier.weight(1f)) {
                        Text(current ?: "—", color = Tok.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
                        Text(
                            buildString {
                                append(status); append(" · token ····"); append(server.token.takeLast(4))
                                if (backups.isNotEmpty()) append(" · 备用 ${backups.size} 个")
                            },
                            color = if (conn is ConnState.Failed) Tok.Red else Tok.Faint, fontSize = 11.sp,
                            modifier = Modifier.clickable { showBackups = !showBackups },
                        )
                    }
                    if (conn is ConnState.Failed) TextButton(onClick = { store.kickReconnect() }) { Text("重试", fontSize = 12.sp) }
                    TextButton(onClick = { nav.navigate("pair") }) { Text("重新配对", fontSize = 12.sp) }
                }
                if (showBackups) backups.forEach { host ->
                    Text(host, color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(start = 36.dp, end = 14.dp, bottom = 4.dp))
                }
            }
            ToggleRow("完成时通知", settings.notifyDone) { scope.launch { store.settings.setNotifyDone(it) } }
            ToggleRow("后台常驻", settings.serviceEnabled) { on ->
                scope.launch { store.settings.setServiceEnabled(on) }
                if (on) NotificationService.start(context) else NotificationService.stop(context)
            }
            if (settings.mutedProjects.isNotEmpty()) {
                // 静音的项目挤在一行里，点名字取消
                Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("已静音", color = Tok.Ink, fontSize = 14.sp)
                    Spacer(Modifier.width(10.dp))
                    Text(
                        settings.mutedProjects.joinToString(" · ") { it.substringAfterLast('/') } + "（点取消）",
                        color = Tok.Dim, fontSize = 12.sp, modifier = Modifier.weight(1f).clickable {
                            scope.launch { settings.mutedProjects.forEach { store.settings.setProjectMuted(it, false) } }
                        },
                    )
                }
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("主题", color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                SingleChoiceSegmentedButtonRow {
                    Palette.all.forEachIndexed { i, p ->
                        SegmentedButton(
                            selected = settings.theme == p.name,
                            onClick = { scope.launch { store.settings.setTheme(p.name) } },
                            shape = SegmentedButtonDefaults.itemShape(i, Palette.all.size),
                            icon = {},
                        ) { Text(p.label, fontSize = 12.sp, maxLines = 1) }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("默认视图", color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                SingleChoiceSegmentedButtonRow {
                    SegmentedButton(
                        selected = settings.defaultUi == "messages",
                        onClick = { scope.launch { store.settings.setDefaultUi("messages") } },
                        shape = SegmentedButtonDefaults.itemShape(0, 2), icon = {},
                    ) { Text("消息流", fontSize = 12.sp) }
                    SegmentedButton(
                        selected = settings.defaultUi == "terminal",
                        onClick = { scope.launch { store.settings.setDefaultUi("terminal") } },
                        shape = SegmentedButtonDefaults.itemShape(1, 2), icon = {},
                    ) { Text("终端", fontSize = 12.sp) }
                }
            }
            // daemon：一行说完 版本 · SSD · 运行时长 · 项目根；有新构建时右边亮「重启」
            val h = health
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(
                        if (h == null) "daemon 尚未连上" else "daemon v${h.version} · ${formatUptime(h.uptime_s)}" + (if (h.update_pending) " · 有新构建" else ""),
                        color = if (h?.update_pending == true) Tok.Amber else Tok.Ink, fontSize = 13.sp,
                    )
                    if (h != null) Text(
                        (if (h.ssd_mounted) "SSD 已挂载 · " else "SSD 未挂载 ✗ · ") + h.project_root,
                        color = if (h.ssd_mounted) Tok.Faint else Tok.Red, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                        maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                    )
                }
                if (h != null) {
                    val alive = sessions.count { it.state != "exited" }
                    TextButton(onClick = { if (alive > 0) restartDialog = true else restartDaemon(false) }) {
                        Text(if (alive > 0) "重启（$alive 活）" else "重启", color = if (h.update_pending) Tok.Amber else Tok.Dim, fontSize = 12.sp)
                    }
                }
            }
        }
        }
    }
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
