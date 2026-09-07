package cc.uoox.aaaui

import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch

// ============================================================
// 详情屏（2026-09-08 用户拍板）：会话页右上角那个按钮点开的就是它，顶替了原来的 ⋮ 菜单。
//
// 为什么值得单开一屏：消息流里思考和工具步骤都折叠成一行，**开过哪些子代理、后台还挂着
// 什么、传上去过什么文件、用过哪些技能**——这些事发生过，但翻不出来。⋮ 里那九项则大半
// 是「一年用一次」的操作，占着顶栏没道理。
//
// 不做的事：结束会话、删除。会话的生杀归项目列表（长按项目行）——人不会进到一条对话
// 里面去管这条对话（用户 2026-09-08）。
//
// 分段照 Antigravity 的辅助面板：一行一个名字 + 计数，点开才展开。数据来自
// `GET /sessions/:id/detail`（子代理 / 后台任务 / 已上传 / 技能）与
// `GET /sessions/:id/artifacts`（产物）；用量和进度清单会话对象自带。
// ============================================================

/** 会话详情。`sessionId` 不在会话表里（刚被删）时只画一句话。 */
@Composable
fun SessionDetailScreen(store: AppStore, nav: NavHostController, sessionId: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sessions by store.sessions.collectAsState()
    val settings by store.settings.flow.collectAsState(initial = AppSettings())
    val openSession = LocalOpenSession.current
    val s = sessions.find { it.id == sessionId }

    var detail by remember(sessionId) { mutableStateOf<SessionDetail?>(null) }
    var artifacts by remember(sessionId) { mutableStateOf<List<ArtifactInfo>?>(null) }
    var error by remember(sessionId) { mutableStateOf<String?>(null) }
    var renameDialog by remember { mutableStateOf(false) }
    var urlsDialog by remember { mutableStateOf<List<String>?>(null) }

    fun toast(msg: String) = Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()

    suspend fun fetch() {
        val api = store.client ?: return
        // 404 = daemon 还没升到 v1.17：给空壳而不是报错，别让整屏红掉
        detail = runCatching { api.detail(sessionId) }
            .recover { if (it is DaemonHttpException && it.code == 404) SessionDetail() else throw it }
            .onFailure { error = it.message }
            .getOrNull() ?: detail
        artifacts = runCatching { sortArtifacts(api.artifacts(sessionId)) }
            .recover { if (it is DaemonHttpException && it.code == 404) emptyList() else throw it }
            .getOrNull() ?: artifacts
    }
    LaunchedEffect(sessionId) { fetch() }
    // 消息流有动静就重拉：子代理起没起来、后台任务回没回来，都从 transcript 来
    LaunchedEffect(sessionId) {
        store.frames.collectLatest { f ->
            if (f is EventFrame.MessagesChanged && f.id == sessionId) {
                kotlinx.coroutines.delay(1_500)
                fetch()
            }
        }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 28.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("详情", color = Tok.Ink, fontSize = 18.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
        }
        if (s == null) {
            Text("这个会话已经不在了", color = Tok.Faint, fontSize = 13.sp, modifier = Modifier.padding(18.dp))
            return@Column
        }

        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
            Column(Modifier.padding(horizontal = 18.dp, vertical = 6.dp)) {
                Text(s.title.ifBlank { s.project_name }, color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
                Text(s.project_path, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
            }
            error?.let { Text("获取失败：$it", color = Tok.Red, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 18.dp, vertical = 4.dp)) }

            // ── 用量：顶栏只留了模型和上下文，缓存命中率、花费、时长、行数在这里
            DetailSection("用量", null, initiallyOpen = true) { UsageBlock(s.usage) }

            // ── 进度：daemon 每轮 Stop 后让 haiku 重写的清单
            val items = remember(s.summary) { parseChecklist(s.summary) }
            DetailSection("进度", items.size.takeIf { it > 0 }, initiallyOpen = true, note = if (items.isEmpty()) null else checklistProgress(items)) {
                if (items.isEmpty()) {
                    EmptyHint("每轮回复结束后这里会更新一份「做了什么 / 还没做什么」")
                } else {
                    items.forEach { it ->
                        Row(Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 4.dp), verticalAlignment = Alignment.Top) {
                            Text(if (it.done) "☑" else "☐", color = if (it.done) Tok.Green else Tok.Amber, fontSize = 13.sp, modifier = Modifier.width(22.dp))
                            Text(it.text, color = if (it.done) Tok.Dim else Tok.Ink, fontSize = 13.sp, lineHeight = 18.sp)
                        }
                    }
                }
            }

            val d = detail
            // ── 子代理：Agent / Task 调用，还在跑的转圈
            DetailSection("子代理", d?.subagents?.size) {
                if (d == null) LoadingRow()
                else if (d.subagents.isEmpty()) EmptyHint("这个会话还没开过子代理")
                else d.subagents.forEach { a -> SubagentRow(a) }
            }

            // ── 后台任务：还没等到 <task-notification> 的那些
            DetailSection("后台任务", d?.background_tasks?.size) {
                if (d == null) LoadingRow()
                else if (d.background_tasks.isEmpty()) EmptyHint("没有挂着的后台任务")
                else d.background_tasks.forEach { t ->
                    TwoLineRow(t.tool, t.summary.ifBlank { "（没有摘要）" }, relativeTime(t.ts), accent = true)
                }
            }

            // ── 已上传：项目 _inbox/ 里的文件（📎 和系统分享都落这儿）
            DetailSection("已上传", d?.uploads?.size) {
                if (d == null) LoadingRow()
                else if (d.uploads.isEmpty()) EmptyHint("还没有传过文件进这个项目")
                else d.uploads.forEach { u ->
                    TwoLineRow(u.name, humanBytes(u.size), relativeTime(u.ts))
                }
            }

            // ── 产物：会话里发布过的 Artifact
            DetailSection("产物", artifacts?.size) {
                val list = artifacts
                if (list == null) LoadingRow()
                else if (list.isEmpty()) EmptyHint("这个会话还没有发布产物")
                else list.forEach { a ->
                    TwoLineRow(
                        a.title.ifBlank { a.url.substringAfterLast('/').ifBlank { a.url } },
                        a.description.ifBlank { a.url },
                        artifactTimeLabel(a.ts),
                        onClick = { openUrl(context, a.url) },
                    )
                }
            }

            // ── 已使用技能：Skill 工具调用，按名字合并
            DetailSection("已使用技能", d?.skills?.size) {
                if (d == null) LoadingRow()
                else if (d.skills.isEmpty()) EmptyHint("这个会话还没用过技能")
                else d.skills.forEach { u ->
                    TwoLineRow(u.name, if (u.count > 1) "${u.count} 次" else "1 次", relativeTime(u.last_ts))
                }
            }

            // ── 通知：按项目静音（本机配置）
            DetailSection("通知", null) {
                Row(Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("静音此项目通知", color = Tok.Ink, fontSize = 14.sp, modifier = Modifier.weight(1f))
                    Switch(
                        s.project_path in settings.mutedProjects,
                        { v -> scope.launch { store.settings.setProjectMuted(s.project_path, v) } },
                    )
                }
            }

            // ── 更多：原来 ⋮ 里那些一年用一次的操作，收在最后（用户 2026-09-08：顶栏不该占着它们）。
            // 结束会话 / 删除**不在这里**：那归项目列表（长按项目行）
            DetailSection("更多", null) {
                SheetItem("✏️", "重命名会话", if (s.resume_id != null) "当前为 AI 命名" else null) { renameDialog = true }
                SheetItem("📋", "复制屏幕内容", "终端此刻的画面") {
                    scope.launch {
                        val text = runCatching { store.client?.screen(sessionId)?.text }.getOrNull()?.trim()
                        if (text.isNullOrBlank()) toast("屏幕为空") else { copyToClipboard(context, text); toast("已复制") }
                    }
                }
                SheetItem("🔗", "打开链接…", "从终端回放里找") {
                    scope.launch {
                        val urls = runCatching { store.client?.screen(sessionId)?.text }.getOrNull()
                            ?.let { findUrls(it).map { u -> u.url } }.orEmpty().distinct()
                        if (urls.isEmpty()) toast("回放里没有链接") else urlsDialog = urls
                    }
                }
                SheetItem("🔁", "重启 agent", "resume 同一会话") {
                    scope.launch {
                        try {
                            val api = store.client ?: return@launch
                            store.markUserKilled(sessionId)
                            runCatching { api.kill(sessionId) }
                            val fresh = api.createSession(s.project_path, s.agent, resume = true)
                            store.releaseAttachmentNow(sessionId)
                            nav.popBackStack()
                            openSession(fresh.id, "")
                        } catch (e: Exception) { toast("重启失败：${e.message}") }
                    }
                }
            }
            Spacer(Modifier.height(40.dp))
        }
    }

    if (renameDialog) {
        var title by remember { mutableStateOf(s?.title.orEmpty()) }
        AlertDialog(
            onDismissRequest = { renameDialog = false },
            containerColor = Tok.Raised,
            title = { Text("重命名会话", color = Tok.Ink) },
            text = { OutlinedTextField(title, { title = it }, singleLine = true) },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        runCatching { store.client?.rename(sessionId, title.trim()) }.onFailure { toast("重命名失败：${it.message}") }
                        store.refreshSessions()
                    }
                    renameDialog = false
                }) { Text("确定") }
            },
            dismissButton = { TextButton(onClick = { renameDialog = false }) { Text("取消", color = Tok.Dim) } },
        )
    }
    urlsDialog?.let { urls ->
        AlertDialog(
            onDismissRequest = { urlsDialog = null },
            containerColor = Tok.Raised,
            title = { Text("回放里的链接", color = Tok.Ink) },
            text = {
                Column(Modifier.verticalScroll(rememberScrollState())) {
                    urls.forEach { url ->
                        Text(
                            url, color = Tok.Accent, fontSize = 13.sp, fontFamily = FontFamily.Monospace,
                            maxLines = 2, overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.fillMaxWidth().clickable { openUrl(context, url); urlsDialog = null }.padding(vertical = 8.dp),
                        )
                    }
                }
            },
            confirmButton = { TextButton(onClick = { urlsDialog = null }) { Text("取消", color = Tok.Dim) } },
        )
    }
}

/**
 * 一节：`名字  N  ›`，点标题展开 / 收起（照 Antigravity 的辅助面板）。
 * `count` 为 null 时不显示计数（用量 / 通知 / 更多这种没有条目数的）。
 */
@Composable
private fun DetailSection(
    label: String,
    count: Int?,
    initiallyOpen: Boolean = false,
    note: String? = null,
    body: @Composable () -> Unit,
) {
    var open by rememberSaveable(label) { mutableStateOf(initiallyOpen) }
    HorizontalDivider(color = Tok.Edge, thickness = 1.dp)
    Row(
        Modifier.fillMaxWidth().clickable { open = !open }.padding(horizontal = 18.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Medium)
        if (count != null) {
            Spacer(Modifier.width(8.dp))
            Text("$count", color = Tok.Faint, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
        }
        Spacer(Modifier.weight(1f))
        note?.let { Text(it, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(end = 8.dp)) }
        Text(if (open) "▾" else "›", color = Tok.Dim, fontSize = 14.sp)
    }
    if (open) Column(Modifier.fillMaxWidth().padding(bottom = 8.dp)) { body() }
}

@Composable
private fun EmptyHint(text: String) {
    Text(text, color = Tok.Faint, fontSize = 12.5.sp, modifier = Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 10.dp))
}

@Composable
private fun LoadingRow() {
    Box(Modifier.fillMaxWidth().padding(18.dp), contentAlignment = Alignment.CenterStart) {
        CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = Tok.Accent)
    }
}

/** 标题 + 副标题 + 右边一个时间/尺寸；可点。 */
@Composable
private fun TwoLineRow(title: String, subtitle: String, trailing: String?, accent: Boolean = false, onClick: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth()
            .let { if (onClick != null) it.clickable(onClick = onClick) else it }
            .padding(horizontal = 18.dp, vertical = 8.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, color = if (accent) Tok.Accent else Tok.Ink, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle.isNotBlank()) {
                Text(subtitle, color = Tok.Dim, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        trailing?.takeIf { it.isNotBlank() }?.let {
            Spacer(Modifier.width(10.dp))
            Text(it, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
        }
    }
}

/** 一个子代理：类型 + 干什么 + 状态（还在跑的转圈） */
@Composable
private fun SubagentRow(a: Subagent) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 8.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Box(Modifier.width(20.dp), contentAlignment = Alignment.TopStart) {
            when (a.status) {
                "running" -> CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp, color = Tok.Accent)
                "err" -> Text("✗", color = Tok.Red, fontSize = 12.sp)
                else -> Text("✓", color = Tok.Green, fontSize = 12.sp)
            }
        }
        Column(Modifier.weight(1f)) {
            Text(a.kind.ifBlank { a.tool }, color = Tok.Ink, fontSize = 14.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (a.summary.isNotBlank()) Text(a.summary, color = Tok.Dim, fontSize = 12.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(10.dp))
        Text(relativeTime(a.ts), color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
    }
}

/** 用量块：模型 / 上下文（带条）/ 缓存命中 / 花费 / 时长 / 行数。没有的字段整行不占。 */
@Composable
private fun UsageBlock(u: SessionUsage?) {
    if (u == null) {
        EmptyHint("还没有用量数据（daemon 接管了 statusLine，跑一轮就有）")
        return
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 18.dp)) {
        u.model?.takeIf { it.isNotBlank() }?.let { KeyValue("模型", it) }
        u.context_pct?.let { pct ->
            Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("上下文", color = Tok.Dim, fontSize = 13.sp, modifier = Modifier.width(76.dp))
                Box(Modifier.weight(1f).height(4.dp).background(Tok.Edge, RoundedCornerShape(2.dp))) {
                    Box(
                        Modifier.fillMaxWidth((pct / 100.0).coerceIn(0.0, 1.0).toFloat()).height(4.dp)
                            .background(pctColor(pctColorLevel(pct), Tok.Accent), RoundedCornerShape(2.dp)),
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text(pctText(pct), color = pctColor(pctColorLevel(pct), Tok.Ink), fontSize = 12.sp, fontFamily = FontFamily.Monospace)
            }
        }
        u.cache_hit_pct?.let { KeyValue("缓存命中", String.format(java.util.Locale.US, "%.1f%%", it)) }
        u.cost_usd?.let { KeyValue("花费", String.format(java.util.Locale.US, "$%.2f", it)) }
        u.duration_ms?.takeIf { it > 0 }?.let { KeyValue("时长", humanDuration(it)) }
        if ((u.lines_added ?: 0) > 0 || (u.lines_removed ?: 0) > 0) {
            KeyValue("行数", "+${u.lines_added ?: 0} / -${u.lines_removed ?: 0}")
        }
        Spacer(Modifier.height(4.dp))
    }
}

@Composable
private fun KeyValue(k: String, v: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 5.dp), horizontalArrangement = Arrangement.Start) {
        Text(k, color = Tok.Dim, fontSize = 13.sp, modifier = Modifier.width(76.dp))
        Text(v, color = Tok.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** `1.2s` / `3m 20s` / `1h 04m` */
fun humanDuration(ms: Long): String {
    val s = ms / 1000
    return when {
        s < 60 -> String.format(java.util.Locale.US, "%.1fs", ms / 1000.0)
        s < 3600 -> "${s / 60}m ${s % 60}s"
        else -> String.format(java.util.Locale.US, "%dh %02dm", s / 3600, (s % 3600) / 60)
    }
}
