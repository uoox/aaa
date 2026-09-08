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
import androidx.compose.foundation.layout.fillMaxWidth as fillMaxWidthFrac
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.items
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import kotlinx.coroutines.launch
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController

/**
 * 看板（2026-09-07 用户拍板，第二版）：**所有会话的进度**，没有时间维度。
 * 数据是 daemon 一次算好的 `GET /history/dashboard`。
 *
 * 顶上是未完成条目数 + 搜索（2026-09-08 用户拍板：五态计数条和状态字一起去掉，看板和项目
 * 列表说同一套话）。主体一会话一张卡：在跑就一个蓝点、否则什么都没有 + 标题 + 项目，一根进度条 done/total，下面直接列
 * 没勾的项，做完的折成一行「已做 N」点开看。已完成的（暂停且全勾完）默认收进「已完成 N」；
 * 已删除的默认不显示，一个开关切出来。会话还在就能点开，已退出的只能看。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    val scope = rememberCoroutineScope()
    var dash by remember { mutableStateOf<Dashboard?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var showDeleted by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        try { dash = store.client?.historyDashboard(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持看板（需 v1.13）" else e.message }
        catch (e: kotlinx.serialization.SerializationException) { error = "daemon 太旧，看板形状不对（需 v1.13）：${e.message?.take(80)}" }
        catch (e: Exception) { error = e.message }
    }
    val d = dash
    // 2026-09-07 第三版：瀑布流、全展开——一眼看全，不折叠、不分组
    val matched = remember(d, query, showDeleted) {
        d?.sessions.orEmpty()
            .filter { cardMatches(it, query) }
            .filter { showDeleted || !it.deleted }
    }
    val deletedN = d?.sessions.orEmpty().count { it.deleted }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("‹", color = Tok.Dim, fontSize = 26.sp, modifier = Modifier.clickable { nav.popBackStack() }.padding(horizontal = 8.dp))
            Text("看板", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            d?.let { Text("未完成 ${it.counts.open_items} 条", color = Tok.Amber, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        Box(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp).background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            if (query.isEmpty()) Text("搜索：标题 / 项目 / 条目", color = Tok.Faint, fontSize = 14.sp)
            BasicTextField(query, { query = it }, singleLine = true, modifier = Modifier.fillMaxWidth(), textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp), cursorBrush = SolidColor(Tok.Accent))
        }
        error?.let { Text(it, color = Tok.Red, fontSize = 13.sp, modifier = Modifier.padding(16.dp)) }

        if (d != null && deletedN > 0) {
            // 五态计数条去掉了；只剩「已删除 N」这个开关（它不是状态，是一个筛子）
            Text(
                "已删除 $deletedN", color = if (showDeleted) Tok.Accent else Tok.Dim, fontSize = 11.5.sp,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 8.dp)
                    .background(if (showDeleted) Tok.Accent.copy(alpha = 0.14f) else Color.Transparent, RoundedCornerShape(8.dp))
                    .clickable { showDeleted = !showDeleted }.padding(horizontal = 10.dp, vertical = 6.dp),
            )
        }
        if (d == null) {
            if (error == null) Text("加载中…", color = Tok.Faint, modifier = Modifier.padding(16.dp))
        } else if (matched.isEmpty()) {
            Text(
                if (d.sessions.isEmpty()) "还没有记录（daemon 每秒把会话同步进日志）" else "没有匹配的会话",
                color = Tok.Faint, fontSize = 13.sp, modifier = Modifier.padding(16.dp),
            )
        } else {
            // 瀑布流：卡片高矮不一（清单长短不同），交错网格才不浪费竖向空间；宽屏 / 折叠屏内屏自动多列
            LazyVerticalStaggeredGrid(
                columns = StaggeredGridCells.Adaptive(300.dp),
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(start = 8.dp, end = 8.dp, bottom = 24.dp),
                verticalItemSpacing = 8.dp,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                val card: @Composable (SessionCard) -> Unit = { c ->
                    SessionCardView(
                        c, onOpen = { if (c.alive) openSession(c.id, "") },
                        onToggle = { it ->
                            if (c.alive) scope.launch {
                                runCatching { store.client?.checklist(c.id, it.text, !it.done); dash = store.client?.historyDashboard() }
                                    .onFailure { e -> error = e.message }
                            }
                        },
                    )
                }
                items(matched, key = { "card-${it.id}" }) { card(it) }
            }
        }
    }
}

/** 一张卡：状态字 + 标题 + 项目 → 进度条 → 全部清单项（没勾的在前、做完的灰掉）。全展开，一眼看全。会话还在才可点开 */
@Composable
private fun SessionCardView(c: SessionCard, onOpen: () -> Unit, onToggle: (ChecklistItem) -> Unit = {}) {
    val total = c.done + c.open
    Column(
        Modifier.fillMaxWidth()
            .background(Tok.Surface, RoundedCornerShape(10.dp)).border(1.dp, Tok.Edge, RoundedCornerShape(10.dp))
            .clickable(enabled = c.alive, onClick = onOpen).padding(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            // 蓝点 = 还在跑；已删除的写一个字；其余什么都不画（和项目列表同一套话）
            if (cardRunning(c)) {
                Box(Modifier.size(7.dp).background(Tok.Blue, CircleShape))
                Spacer(Modifier.width(8.dp))
            } else if (c.deleted) {
                Text("已删除", color = Tok.Faint, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace)
                Spacer(Modifier.width(8.dp))
            }
            Text(
                c.title, color = if (c.deleted) Tok.Dim else Tok.Ink, fontSize = 14.sp, fontWeight = FontWeight.Medium,
                maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            if (c.alive) Text("打开", color = Tok.Accent, fontSize = 12.sp, modifier = Modifier.padding(start = 8.dp))
        }
        Text(c.project_name, color = Tok.Faint, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.padding(top = 2.dp))
        if (total > 0) {
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.weight(1f).height(4.dp).background(Tok.Edge, RoundedCornerShape(2.dp))) {
                    Box(
                        Modifier.fillMaxWidthFrac(c.done.toFloat() / total).height(4.dp)
                            .background(if (c.open == 0) Tok.Green else Tok.Accent, RoundedCornerShape(2.dp)),
                    )
                }
                Spacer(Modifier.width(8.dp))
                Text("${c.done}/$total", color = Tok.Dim, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace)
            }
        } else {
            Text("没有进度清单", color = Tok.Faint, fontSize = 11.5.sp, modifier = Modifier.padding(top = 6.dp))
        }
        (c.items.filter { !it.done } + c.items.filter { it.done }).forEach { it ->
            // 会话还在池子里就能点着勾 / 取消勾（POST /sessions/:id/checklist）
            Row(Modifier.fillMaxWidth().clickable(enabled = c.alive) { onToggle(it) }.padding(top = 5.dp), verticalAlignment = Alignment.Top) {
                Text(if (it.done) "☑" else "☐", color = if (it.done) Tok.Green else Tok.Amber, fontSize = 13.sp, modifier = Modifier.width(20.dp))
                Text(it.text, color = if (it.done) Tok.Dim else Tok.Ink, fontSize = 13.sp, lineHeight = 18.sp)
            }
        }
    }
}
