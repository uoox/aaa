package cc.uoox.aaaui

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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController
import java.time.LocalDate
import java.time.YearMonth

/**
 * 历史（2026-09-07 用户拍板，照 todo 应用的样子）：两个 tab。
 * 「任务」以会话为任务：按 进行中 / 未完成 / 已完成 / 已删除 分组，行上是标题 + 没勾的项，
 * 点一行展开清单，「打开」进会话。「日历」是月历 + 那天 haiku 写的摘要 + 那天的会话。
 * 顶部搜索框两边共用。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    val sessions by store.sessions.collectAsState()
    var entries by remember { mutableStateOf<List<HistoryEntry>?>(null) }
    var days by remember { mutableStateOf<List<DayDigest>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var calendarTab by rememberSaveable { mutableStateOf(false) }
    var month by rememberSaveable { mutableStateOf(YearMonth.now().toString()) }
    var selectedDay by rememberSaveable { mutableStateOf<String?>(null) }
    var expanded by remember { mutableStateOf(setOf<String>()) }
    LaunchedEffect(Unit) {
        try { entries = store.client?.history(500).orEmpty(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持会话日志（需 v1.9）" else e.message }
        catch (e: Exception) { error = e.message }
        runCatching { days = store.client?.historyDays().orEmpty() }
    }
    val alive = remember(sessions) { sessions.map { it.id }.toSet() }
    val byDay = remember(days) { days.associateBy { it.date } }
    val matched = remember(entries, query) { entries.orEmpty().filter { historyMatches(it, query) } }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("历史", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.width(12.dp))
            Tab("任务", !calendarTab) { calendarTab = false }
            Spacer(Modifier.width(6.dp))
            Tab("日历", calendarTab) { calendarTab = true }
            Spacer(Modifier.weight(1f))
            entries?.let { Text("${it.size} 条", color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        Box(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            if (query.isEmpty()) Text("搜索：标题 / 项目 / 清单", color = Tok.Faint, fontSize = 14.sp)
            BasicTextField(query, { query = it }, singleLine = true, modifier = Modifier.fillMaxWidth(), textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp), cursorBrush = SolidColor(Tok.Accent))
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            if (entries == null && error == null) { item { Text("加载中…", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }; return@LazyColumn }
            if (!calendarTab) {
                if (matched.isEmpty()) item { Text(if (entries.isNullOrEmpty()) "还没有记录" else "没有匹配的会话", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }
                TaskGroup.entries.forEach { g ->
                    val rows = matched.filter { taskGroup(it, it.id in alive) == g }
                    if (rows.isEmpty()) return@forEach
                    item(key = "hdr-${g.name}") {
                        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Text(g.label, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
                            Text("${rows.size}", color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                        }
                    }
                    items(rows, key = { it.id }) { e ->
                        TaskRow(e, g, expanded = e.id in expanded, openable = e.id in alive,
                            onToggle = { expanded = if (e.id in expanded) expanded - e.id else expanded + e.id },
                            onOpen = { openSession(e.id, "") })
                    }
                }
            } else {
                item(key = "calendar") {
                    val ym = runCatching { YearMonth.parse(month) }.getOrDefault(YearMonth.now())
                    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text("‹", color = Tok.Dim, fontSize = 20.sp, modifier = Modifier.clickable { month = ym.minusMonths(1).toString() }.padding(horizontal = 10.dp))
                            Text(ym.toString(), color = Tok.Ink, fontSize = 13.sp, fontFamily = FontFamily.Monospace)
                            Text("›", color = Tok.Dim, fontSize = 20.sp, modifier = Modifier.clickable { month = ym.plusMonths(1).toString() }.padding(horizontal = 10.dp))
                        }
                        Row(Modifier.fillMaxWidth()) {
                            listOf("一", "二", "三", "四", "五", "六", "日").forEach { Text(it, color = Tok.Faint, fontSize = 10.sp, modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center) }
                        }
                        val lead = ym.atDay(1).dayOfWeek.value - 1
                        val total = lead + ym.lengthOfMonth()
                        val rows = (total + 6) / 7
                        val today = LocalDate.now().toString()
                        for (r in 0 until rows) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                            for (col in 0 until 7) {
                                val idx = r * 7 + col
                                val d = idx - lead + 1
                                if (d < 1 || d > ym.lengthOfMonth()) { Spacer(Modifier.weight(1f).height(40.dp)); continue }
                                val date = ym.atDay(d).toString()
                                val n = byDay[date]?.sessions ?: 0
                                val selected = selectedDay == date
                                Column(
                                    Modifier.weight(1f).height(40.dp)
                                        .then(if (selected) Modifier.background(Tok.Accent.copy(alpha = 0.18f), RoundedCornerShape(6.dp)).border(1.dp, Tok.Accent, RoundedCornerShape(6.dp)) else Modifier)
                                        .then(if (n > 0) Modifier.clickable { selectedDay = if (selected) null else date } else Modifier),
                                    horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.Center,
                                ) {
                                    Text("$d", color = if (date == today) Tok.Accent else if (n > 0) Tok.Ink else Tok.Faint, fontSize = 13.sp, fontWeight = if (date == today) FontWeight.Bold else FontWeight.Normal)
                                    if (n > 0) Text("$n", color = Tok.Green, fontSize = 9.sp, fontFamily = FontFamily.Monospace)
                                }
                            }
                        }
                    }
                }
                selectedDay?.let { d -> byDay[d] }?.let { dg ->
                    item(key = "digest") {
                        Column(
                            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
                                .background(Tok.Surface, RoundedCornerShape(10.dp)).border(1.dp, Tok.Edge, RoundedCornerShape(10.dp)).padding(12.dp),
                        ) {
                            Text("${dg.date} · ${dg.sessions} 个会话", color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                            Text(dg.text.ifBlank { "haiku 还没写这一天的摘要（daemon 每 5 分钟补一次）" }, color = Tok.Ink, fontSize = 13.5.sp, lineHeight = 20.sp, modifier = Modifier.padding(top = 4.dp))
                        }
                    }
                }
                val dayRows = matched.filter { e -> selectedDay?.let { localDayOf(e.created_at) == it } ?: true }
                if (dayRows.isEmpty()) item { Text(if (selectedDay != null) "这天没有匹配的会话" else "没有匹配的会话", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }
                items(dayRows, key = { it.id }) { e ->
                    TaskRow(e, taskGroup(e, e.id in alive), expanded = e.id in expanded, openable = e.id in alive,
                        onToggle = { expanded = if (e.id in expanded) expanded - e.id else expanded + e.id },
                        onOpen = { openSession(e.id, "") })
                }
            }
        }
    }
}

@Composable
private fun Tab(label: String, on: Boolean, onClick: () -> Unit) {
    Text(
        label, color = if (on) Tok.Accent else Tok.Dim, fontSize = 13.sp,
        modifier = Modifier
            .background(if (on) Tok.Accent.copy(alpha = 0.14f) else androidx.compose.ui.graphics.Color.Transparent, RoundedCornerShape(8.dp))
            .clickable(onClick = onClick).padding(horizontal = 10.dp, vertical = 4.dp),
    )
}

/** 一条任务 = 一个会话：状态格 + 标题 + 没勾的项；点开展开清单；「打开」进会话 */
@Composable
private fun TaskRow(e: HistoryEntry, g: TaskGroup, expanded: Boolean, openable: Boolean, onToggle: () -> Unit, onOpen: () -> Unit) {
    val items = parseChecklist(e.summary)
    val (glyph, color) = when (g) {
        TaskGroup.ACTIVE -> "◐" to Tok.Green
        TaskGroup.OPEN -> "☐" to Tok.Amber
        TaskGroup.DONE -> "☑" to Tok.Dim
        TaskGroup.DELETED -> "✕" to Tok.Faint
    }
    val sub = taskSubline(e)
    Column(Modifier.fillMaxWidth().clickable(onClick = onToggle).padding(horizontal = 16.dp, vertical = 9.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Text(glyph, color = color, fontSize = 17.sp, modifier = Modifier.width(26.dp).padding(top = 1.dp))
            Column(Modifier.weight(1f)) {
                Text(e.title.ifBlank { e.project_name }, color = if (g == TaskGroup.DELETED) Tok.Dim else Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Medium, maxLines = if (expanded) 3 else 1, overflow = TextOverflow.Ellipsis)
                if (sub.isNotEmpty()) Text(sub, color = Tok.Dim, fontSize = 12.5.sp, maxLines = if (expanded) 4 else 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 2.dp))
                Row(Modifier.fillMaxWidth().padding(top = 3.dp)) {
                    Text(artifactTimeLabel(e.created_at), color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
                    Text(e.project_name + if (e.agent == "shell") " · 终端" else "", color = Tok.Faint, fontSize = 11.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            if (openable) Text("打开", color = Tok.Accent, fontSize = 12.sp, modifier = Modifier.clickable(onClick = onOpen).padding(start = 10.dp, top = 2.dp, bottom = 6.dp))
        }
        if (expanded) Column(Modifier.padding(start = 26.dp, top = 6.dp)) {
            if (items.isEmpty()) Text("没有进度清单", color = Tok.Faint, fontSize = 12.sp)
            items.forEach { it ->
                Row(Modifier.padding(vertical = 2.dp), verticalAlignment = Alignment.Top) {
                    Text(if (it.done) "☑" else "☐", color = if (it.done) Tok.Green else Tok.Faint, fontSize = 14.sp, modifier = Modifier.width(22.dp))
                    Text(it.text, color = if (it.done) Tok.Dim else Tok.Ink, fontSize = 13.sp, lineHeight = 18.sp)
                }
            }
        }
    }
    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 42.dp))
}
