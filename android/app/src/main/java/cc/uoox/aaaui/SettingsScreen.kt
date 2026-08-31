package cc.uoox.aaaui

import android.content.Intent
import android.widget.Toast
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
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.launch

@Composable
fun SettingsTab(store: AppStore, nav: NavHostController) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val conn by store.connState.collectAsState()
    val health by store.health.collectAsState()
    var permissions by remember { mutableStateOf<List<Permission>?>(null) }
    var permError by remember { mutableStateOf<String?>(null) }
    var phraseDialog by remember { mutableStateOf(false) }

    suspend fun loadPermissions() {
        try { permissions = store.client?.macPermissions(); permError = null }
        catch (e: Exception) { permError = e.message }
    }
    LaunchedEffect(conn) { if (conn is ConnState.Connected) { loadPermissions(); store.refreshHealth() } }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 90.dp)) {
        Text("设置", color = Tok.Ink, fontSize = 22.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(16.dp))

        // ---------- 服务器 ----------
        GroupTitle("服务器")
        Group {
            val server = settings.server
            if (server == null) {
                SettingRow("未配对") { TextButton(onClick = { nav.navigate("pair") }) { Text("去配对") } }
            } else {
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
                SettingRow("Token") {
                    Text("····" + server.token.takeLast(4), color = Tok.Dim, fontFamily = FontFamily.Monospace, fontSize = 12.sp)
                    Spacer(Modifier.width(10.dp))
                    TextButton(onClick = { nav.navigate("pair") }) { Text("重新配对") }
                }
                SettingRow("连接状态") {
                    when (val c = conn) {
                        is ConnState.Connected -> Text("正常", color = Tok.Green, fontSize = 13.sp)
                        is ConnState.Connecting -> Text("连接中…", color = Tok.Amber, fontSize = 13.sp)
                        is ConnState.Failed -> Row {
                            Text(c.message, color = Tok.Red, fontSize = 12.sp)
                            TextButton(onClick = { store.kickReconnect() }) { Text("重试") }
                        }
                        ConnState.NoServer -> Text("未配对", color = Tok.Dim, fontSize = 13.sp)
                    }
                }
            }
        }

        // ---------- 通知 ----------
        GroupTitle("通知")
        Group {
            ToggleRow("agent 等待输入时推送", settings.notifyWaiting) { scope.launch { store.settings.setNotifyWaiting(it) } }
            ToggleRow("任务完成时推送", settings.notifyExited) { scope.launch { store.settings.setNotifyExited(it) } }
            ToggleRow("会话疑似空转时推送", settings.notifyStalled) { scope.launch { store.settings.setNotifyStalled(it) } }
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
            SettingRow("快捷短语") {
                Text(settings.quickPhrases.joinToString(" · ").ifBlank { "无" }, color = Tok.Faint, fontSize = 11.sp, modifier = Modifier.weight(1f, fill = false))
                TextButton(onClick = { phraseDialog = true }) { Text("编辑") }
            }
        }

        // ---------- macOS 权限 ----------
        GroupTitle("macOS 权限")
        Group {
            Text(
                "授权弹窗出现在 Mac 上，请在 Mac 前完成一次；此后手机远程操作不再被权限弹窗卡死。",
                color = Tok.Dim, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 6.dp),
            )
            permError?.let { Text("读取失败：$it", color = Tok.Red, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp)) }
            permissions?.forEach { p ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(p.label, color = Tok.Ink, fontSize = 13.sp, modifier = Modifier.weight(1f))
                    val (label, color) = when (p.status) {
                        "granted" -> "已授权" to Tok.Green
                        "denied" -> "已拒绝" to Tok.Red
                        "undetermined" -> "待授权" to Tok.Amber
                        "needs_settings" -> "需在系统设置操作" to Tok.Amber
                        else -> "未知" to Tok.Faint
                    }
                    Text(label, color = color, fontSize = 12.sp)
                }
            }
            Row(Modifier.padding(horizontal = 14.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Button(onClick = {
                    scope.launch {
                        try {
                            store.client?.requestMacPermissions()
                            toast("已触发，弹窗在 Mac 上")
                        } catch (e: Exception) { toast("失败：${e.message}") }
                    }
                }) { Text("一键申请全部") }
                TextButton(onClick = { scope.launch { loadPermissions(); toast("已刷新") } }) { Text("刷新") }
            }
        }

        // ---------- 关于 ----------
        GroupTitle("关于")
        Group {
            SettingRow("aaa-daemon") { Text(health?.version?.let { "v$it" } ?: "—", color = Tok.Dim, fontSize = 13.sp, fontFamily = FontFamily.Monospace) }
            SettingRow("SSD 挂载状态") {
                Text(
                    if (health?.ssd_mounted == true) "已挂载 ✓" else if (health == null) "—" else "未挂载 ✗",
                    color = if (health?.ssd_mounted == true) Tok.Green else Tok.Red, fontSize = 13.sp,
                )
            }
            SettingRow("项目根") { Text(health?.project_root ?: "—", color = Tok.Dim, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
            SettingRow("daemon 运行时长") { Text(health?.uptime_s?.let { formatUptime(it) } ?: "—", color = Tok.Dim, fontSize = 13.sp) }
        }
    }

    if (phraseDialog) {
        var phrases by remember { mutableStateOf(settings.quickPhrases) }
        var newPhrase by remember { mutableStateOf("") }
        AlertDialog(
            onDismissRequest = { phraseDialog = false },
            containerColor = Tok.Raised,
            title = { Text("快捷短语", color = Tok.Ink) },
            text = {
                Column {
                    phrases.forEach { ph ->
                        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(ph, color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                            Text("✕", color = Tok.Faint, modifier = Modifier.clickable { phrases = phrases - ph }.padding(6.dp))
                        }
                    }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(newPhrase, { newPhrase = it }, modifier = Modifier.weight(1f), singleLine = true, placeholder = { Text("新短语", fontSize = 13.sp) })
                        TextButton(
                            onClick = { if (newPhrase.isNotBlank()) { phrases = phrases + newPhrase.trim(); newPhrase = "" } },
                        ) { Text("添加") }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch { store.settings.setQuickPhrases(phrases) }
                    phraseDialog = false
                }) { Text("保存") }
            },
            dismissButton = { TextButton(onClick = { phraseDialog = false }) { Text("取消", color = Tok.Dim) } },
        )
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
