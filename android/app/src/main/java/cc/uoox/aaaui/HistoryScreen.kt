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
 * 历史：daemon 的会话日志 + 日历。顶部搜索框；月历里有会话的日子带数字，点一天只看那天
 * （再点取消）；选中那天下面是 haiku 写的「这一天做了什么」；最后是会话列表。
 * 会话还在（活着或有回放）点一行就打开；已删除的只能看。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    val sessions by store.sessions.collectAsState()
    var entries by remember { mutableStateOf<List<HistoryEntry>?>(null) }
    var days by remember { mutableStateOf<List<DayDigest>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var month by rememberSaveable { mutableStateOf(YearMonth.now().toString()) }
    var selectedDay by rememberSaveable { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        try { entries = store.client?.history(500).orEmpty(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持会话日志（需 v1.9）" else e.message }
        catch (e: Exception) { error = e.message }
        runCatching { days = store.client?.historyDays().orEmpty() }
    }
    val alive = remember(sessions) { sessions.map { it.id }.toSet() }
    val byDay = remember(days) { days.associateBy { it.date } }
    val filtered = remember(entries, query, selectedDay) {
        entries.orEmpty().filter { historyMatches(it, query) }.filter { e -> selectedDay?.let { localDayOf(e.created_at) == it } ?: true }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("历史", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
            entries?.let { Text("${it.size} 条", color = Tok.Faint, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        // 搜索
        Box(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            if (query.isEmpty()) Text("搜索：标题 / 项目 / 清单", color = Tok.Faint, fontSize = 14.sp)
            BasicTextField(query, { query = it }, singleLine = true, modifier = Modifier.fillMaxWidth(), textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp), cursorBrush = SolidColor(Tok.Accent))
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }

        LazyColumn(Modifier.fillMaxSize()) {
            // 月历
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
                    val lead = ym.atDay(1).dayOfWeek.value - 1 // 周一 = 0
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
            // 选中那天的摘要
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
            val list = entries
            when {
                list == null && error == null -> item { Text("加载中…", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }
                list != null && filtered.isEmpty() -> item { Text(if (list.isEmpty()) "还没有记录" else "没有匹配的会话", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }
                else -> items(filtered, key = { it.id }) { e ->
                    val openable = e.id in alive
                    val (status, color) = when {
                        e.deleted_at != null -> "已删除" to Tok.Faint
                        e.last_state == "running" -> "执行中" to Tok.Green
                        e.last_state == "waiting" -> "已激活" to Tok.Accent
                        else -> "已退出" to Tok.Dim
                    }
                    val items = parseChecklist(e.summary)
                    val meta = buildString {
                        append(e.project_name)
                        if (e.agent == "shell") append(" · 终端")
                        append(" · "); append(artifactTimeLabel(e.created_at))
                        e.ended_at?.let { append(" → "); append(artifactTimeLabel(it)) }
                        if (items.isNotEmpty()) { append(" · "); append(checklistProgress(items)) }
                    }
                    Column(
                        Modifier.fillMaxWidth()
                            .then(if (openable) Modifier.clickable { openSession(e.id, "") } else Modifier)
                            .padding(horizontal = 16.dp, vertical = 9.dp),
                    ) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(status, color = color, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.width(48.dp))
                            Spacer(Modifier.width(6.dp))
                            Text(
                                e.title.ifBlank { e.project_name }, color = if (e.deleted_at != null) Tok.Dim else Tok.Ink,
                                fontSize = 14.5.sp, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                        }
                        Text(meta, color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(start = 54.dp, top = 2.dp))
                    }
                    HorizontalDivider(color = Tok.Edge, thickness = 1.dp, modifier = Modifier.padding(start = 70.dp))
                }
            }
        }
    }
}
