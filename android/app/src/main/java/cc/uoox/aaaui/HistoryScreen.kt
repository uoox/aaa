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
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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

/**
 * 看板（2026-09-07 用户拍板，取代原来的历史两 tab）：一眼看出**最近做了什么**、
 * **还有什么没做**。数据是 daemon 一次算好的 `GET /history/dashboard`。
 *
 * 手机上一栏从上往下：数字块（今天 / 近 7 天 / 此刻）+ 近 8 周活动条 → 搜索 →
 * 「还没做」（所有会话里没勾的清单项，按项目归并）→「最近做了什么」（按天倒序，
 * haiku 日摘要 + 当天会话）。会话还在就能点开，已删除的只能看。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    var dash by remember { mutableStateOf<Dashboard?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var expandedDays by remember { mutableStateOf(setOf<String>()) }
    LaunchedEffect(Unit) {
        try { dash = store.client?.historyDashboard(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持看板（需 v1.11）" else e.message }
        catch (e: Exception) { error = e.message }
    }
    val d = dash
    val openItems = remember(d, query) { d?.open.orEmpty().filter { openItemMatches(it, query) } }
    val groups = remember(openItems) { groupOpenByProject(openItems) }
    val days = remember(d, query) { d?.days.orEmpty().filter { dayCardMatches(it, query) } }
    val today = remember { LocalDate.now().toString() }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("看板", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            d?.let { Text("待办 ${it.open.size}", color = Tok.Amber, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        Box(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            if (query.isEmpty()) Text("搜索：待办 / 标题 / 项目", color = Tok.Faint, fontSize = 14.sp)
            BasicTextField(query, { query = it }, singleLine = true, modifier = Modifier.fillMaxWidth(), textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp), cursorBrush = SolidColor(Tok.Accent))
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }

        LazyColumn(Modifier.fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 24.dp)) {
            if (d == null) {
                if (error == null) item { Text("加载中…", color = Tok.Faint, modifier = Modifier.padding(16.dp)) }
                return@LazyColumn
            }
            item(key = "stats") {
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    StatTile(Modifier.weight(1f), "今天", "${d.today.sessions} 会话", "做完 ${d.today.done} · 没做 ${d.today.open}")
                    StatTile(Modifier.weight(1f), "近 7 天", "${d.week.sessions} 会话", "做完 ${d.week.done} · 没做 ${d.week.open}")
                    StatTile(Modifier.weight(1f), "此刻", "${d.active} 在跑", "待办共 ${d.open.size}")
                }
            }
            item(key = "spark") { SparkStrip(d.spark) }

            item(key = "todo-hdr") { SectionHeader("还没做", "${openItems.size}", Tok.Amber) }
            if (groups.isEmpty()) {
                item(key = "todo-empty") {
                    Text(
                        if (d.open.isEmpty()) "没有待办——每个会话的进度清单都勾完了" else "没有匹配的待办",
                        color = Tok.Faint, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                    )
                }
            }
            groups.forEach { (project, items) ->
                item(key = "proj-$project") {
                    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 10.dp, bottom = 2.dp)) {
                        Text(project, color = Tok.Dim, fontSize = 12.5.sp, modifier = Modifier.weight(1f))
                        Text("${items.size}", color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace)
                    }
                }
                items(items, key = { "todo-${it.session_id}-${it.text}" }) { it2 ->
                    TodoRow(it2) { if (it2.alive) openSession(it2.session_id, "") }
                }
            }

            item(key = "days-hdr") { SectionHeader("最近做了什么", "${days.size} 天", Tok.Faint) }
            if (days.isEmpty()) {
                item(key = "days-empty") {
                    Text(
                        if (d.days.isEmpty()) "还没有记录（daemon 每秒把会话同步进日志）" else "没有匹配的日子",
                        color = Tok.Faint, fontSize = 13.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                    )
                }
            }
            items(days, key = { "day-${it.date}" }) { day ->
                DayBlock(
                    day, today, expanded = day.date in expandedDays,
                    onToggle = { expandedDays = if (day.date in expandedDays) expandedDays - day.date else expandedDays + day.date },
                    onOpen = { id -> openSession(id, "") },
                )
            }
        }
    }
}

/** 数字块：小标题 + 大字 + 副行 */
@Composable
private fun StatTile(modifier: Modifier, label: String, big: String, sub: String) {
    Column(
        modifier.background(Tok.Surface, RoundedCornerShape(10.dp)).border(1.dp, Tok.Edge, RoundedCornerShape(10.dp))
            .padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Text(label, color = Tok.Faint, fontSize = 11.sp)
        Text(big, color = Tok.Ink, fontSize = 17.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(sub, color = Tok.Dim, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** 近 8 周活动条：一格一天，最旧在左，深浅按当天会话数 */
@Composable
private fun SparkStrip(spark: List<Int>) {
    if (spark.isEmpty()) return
    val max = (spark.maxOrNull() ?: 0).coerceAtLeast(1)
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text("8 周", color = Tok.Faint, fontSize = 10.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.padding(end = 4.dp))
        spark.forEach { n ->
            val alpha = if (n == 0) 0.10f else 0.25f + 0.75f * (n.toFloat() / max)
            Box(Modifier.weight(1f).height(14.dp).background(Tok.Accent.copy(alpha = alpha), RoundedCornerShape(2.dp)))
        }
    }
}

@Composable
private fun SectionHeader(title: String, tail: String, tailColor: androidx.compose.ui.graphics.Color) {
    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, color = Tok.Ink, fontSize = 15.sp, fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f))
        Text(tail, color = tailColor, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
    }
}

/** 一条待办：☐ + 条目本身；副行是它属于哪个会话、什么时候开的。会话还活着才可点开 */
@Composable
private fun TodoRow(item: OpenItem, onOpen: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(enabled = item.alive, onClick = onOpen).padding(horizontal = 16.dp, vertical = 7.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Text("☐", color = Tok.Amber, fontSize = 15.sp, modifier = Modifier.width(24.dp))
        Column(Modifier.weight(1f)) {
            Text(item.text, color = Tok.Ink, fontSize = 14.sp, lineHeight = 19.sp)
            Text(
                item.title + " · " + artifactTimeLabel(item.created_at),
                color = Tok.Faint, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 2.dp),
            )
        }
        if (item.running) Text("● 在跑", color = Tok.Green, fontSize = 10.5.sp, modifier = Modifier.padding(start = 8.dp, top = 2.dp))
    }
}

/** 一天：日期 + 计数 + haiku 要点；点「这天的会话」展开当天会话 */
@Composable
private fun DayBlock(day: DayCard, today: String, expanded: Boolean, onToggle: () -> Unit, onOpen: (String) -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
            .background(Tok.Surface, RoundedCornerShape(10.dp)).border(1.dp, Tok.Edge, RoundedCornerShape(10.dp)).padding(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                day.date + if (day.date == today) "（今天）" else "",
                color = if (day.date == today) Tok.Accent else Tok.Ink,
                fontSize = 13.sp, fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f),
            )
            Text("${day.sessions} 会话 · 做完 ${day.done} · 没做 ${day.open}", color = Tok.Faint, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace)
        }
        if (day.text.isBlank()) {
            Text("haiku 还没写这一天的摘要（daemon 每 5 分钟补一次）", color = Tok.Faint, fontSize = 12.sp, modifier = Modifier.padding(top = 6.dp))
        } else {
            day.text.lineSequence().map { it.trim().trimStart('-', '*', '•').trim() }.filter { it.isNotEmpty() }.forEach { line ->
                Row(Modifier.padding(top = 5.dp), verticalAlignment = Alignment.Top) {
                    Text("▪", color = Tok.Green, fontSize = 12.sp, modifier = Modifier.width(16.dp))
                    Text(line, color = Tok.Ink, fontSize = 13.5.sp, lineHeight = 19.sp)
                }
            }
        }
        Text(
            (if (expanded) "▾" else "▸") + " 这天的会话 ${day.sessions}",
            color = Tok.Dim, fontSize = 11.5.sp, fontFamily = FontFamily.Monospace,
            modifier = Modifier.clickable(onClick = onToggle).padding(top = 8.dp, bottom = 2.dp),
        )
        if (expanded) day.entries.forEach { e ->
            val (glyph, color) = when {
                e.deleted -> "✕" to Tok.Faint
                e.running -> "◐" to Tok.Green
                e.open > 0 -> "☐" to Tok.Amber
                else -> "☑" to Tok.Dim
            }
            Row(
                Modifier.fillMaxWidth().clickable(enabled = e.alive) { onOpen(e.id) }.padding(vertical = 5.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(glyph, color = color, fontSize = 14.sp, modifier = Modifier.width(22.dp))
                Text(
                    e.title, color = if (e.deleted) Tok.Dim else Tok.Ink, fontSize = 13.sp,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
                )
                Spacer(Modifier.size(6.dp))
                Text(
                    e.project_name + if (e.done + e.open > 0) " ${e.done}/${e.done + e.open}" else "",
                    color = Tok.Faint, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}
