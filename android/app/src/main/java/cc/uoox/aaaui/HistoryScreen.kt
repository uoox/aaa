package cc.uoox.aaaui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.staggeredgrid.LazyStaggeredGridState
import androidx.compose.foundation.lazy.staggeredgrid.LazyVerticalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridItemSpan
import androidx.compose.foundation.lazy.staggeredgrid.items
import androidx.compose.foundation.lazy.staggeredgrid.rememberLazyStaggeredGridState
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.navigation.NavHostController

/**
 * 看板的两节（2026-09-08 用户拍板「看板里面东西太多了，要做一下分割，即在 AAA 里面的对话，
 * 和不在里面的对话」）：
 * - [inAaa]（[cardInAaa]）此刻还活着的会话——在跑，或者停在输入框等你说话，就是项目列表上那几行；
 * - [gone] 其余：退出了的（哪怕还在池子里点得开）、只剩一条记录的。
 *
 * [searching] = 搜索框里有字。这时第二节强制展开：搜到的东西藏在折叠节里等于没搜到。
 */
data class DashboardSections(
    val inAaa: List<SessionCard>,
    val gone: List<SessionCard>,
    val searching: Boolean,
) {
    val total: Int get() = inAaa.size + gone.size
}

/**
 * 先过滤（搜索词 + 已删除开关）再按 `alive` 切两节。分节只在客户端做：daemon 给的
 * `sessions` 已经排好序了，节内原样沿用它，客户端不再自作主张排一遍。
 */
fun dashboardSections(cards: List<SessionCard>, query: String, showDeleted: Boolean): DashboardSections {
    val kept = cards.filter { cardMatches(it, query) && (showDeleted || !it.deleted) }
    return DashboardSections(
        inAaa = kept.filter { cardInAaa(it) },
        gone = kept.filter { !cardInAaa(it) },
        searching = query.isNotBlank(),
    )
}

/**
 * 看板（2026-09-07 用户拍板，第二版）：**所有会话的进度**，没有时间维度。
 * 数据是 daemon 一次算好的 `GET /history/dashboard`。
 *
 * 顶上是未完成条目数 + 搜索（2026-09-08 用户拍板：五态计数条和状态字一起去掉，看板和项目
 * 列表说同一套话）。主体一会话一张卡：在跑就一根蓝竖线、否则什么都没有 + 标题 + 项目，一根进度条 done/total，下面直接列
 * 没勾的项，做完的折成一行「已做 N」点开看。已完成的（暂停且全勾完）默认收进「已完成 N」；
 * 已删除的默认不显示，一个开关切出来。会话还在就能点开，已退出的只能看。
 *
 * 2026-09-08 用户拍板「看板里面东西太多了，要做一下分割」：卡片按 `alive` 分成两节，
 * 「不在 AAA 里」那节默认折起来（见 [dashboardSections]）。右缘还有一根滚动指示条
 * （见 [ScrollHint]）——瀑布流全展开之后一屏根本装不下，得知道自己在哪儿。
 */
@Composable
fun HistoryScreen(store: AppStore, nav: NavHostController) {
    val openSession = LocalOpenSession.current
    val scope = rememberCoroutineScope()
    var dash by remember { mutableStateOf<Dashboard?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var query by rememberSaveable { mutableStateOf("") }
    var showDeleted by rememberSaveable { mutableStateOf(false) }
    // 「不在 AAA 里」那节默认折起来，只留一行表头；折叠状态跨旋转 / 重建活着
    var goneOpen by rememberSaveable { mutableStateOf(false) }
    val gridState = rememberLazyStaggeredGridState()
    LaunchedEffect(Unit) {
        try { dash = store.client?.historyDashboard(); error = null }
        catch (e: DaemonHttpException) { error = if (e.code == 404) "daemon 版本不支持看板（需 v1.13）" else e.message }
        catch (e: kotlinx.serialization.SerializationException) { error = "daemon 太旧，看板形状不对（需 v1.13）：${e.message?.take(80)}" }
        catch (e: Exception) { error = e.message }
    }
    val d = dash
    // 瀑布流、节内全展开；分节这一步是纯函数，单测盯着它（见 DashboardSectionsTest）
    val sections = remember(d, query, showDeleted) { dashboardSections(d?.sessions.orEmpty(), query, showDeleted) }
    val deletedN = d?.sessions.orEmpty().count { it.deleted }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(10.dp), verticalAlignment = Alignment.CenterVertically) {
            BackArrow(onClick = { nav.popBackStack() })
            Text("看板", color = Tok.Ink, fontSize = 17.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            d?.let { Text("未完成 ${it.counts.open_items} 条", color = Tok.Amber, fontSize = 12.sp, fontFamily = FontFamily.Monospace) }
        }
        RoundedTextField(
            query, { query = it }, "搜索：标题 / 项目 / 条目",
            modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp),
            singleLine = true,
        )
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
        } else if (sections.total == 0) {
            Text(
                if (d.sessions.isEmpty()) "还没有记录（daemon 每秒把会话同步进日志）" else "没有匹配的会话",
                color = Tok.Faint, fontSize = 13.sp, modifier = Modifier.padding(16.dp),
            )
        } else {
            val goneShown = sections.searching || goneOpen
            Box(Modifier.fillMaxSize()) {
                // 瀑布流：卡片高矮不一（清单长短不同），交错网格才不浪费竖向空间；宽屏 / 折叠屏内屏自动多列
                LazyVerticalStaggeredGrid(
                    columns = StaggeredGridCells.Adaptive(300.dp),
                    state = gridState,
                    modifier = Modifier.fillMaxSize(),
                    contentPadding = PaddingValues(start = 8.dp, end = 8.dp, bottom = 24.dp),
                    verticalItemSpacing = 8.dp,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    val card: @Composable (SessionCard) -> Unit = { c ->
                        SessionCardView(
                            c, onOpen = { if (c.alive) openSession(c.id, "") },
                            onToggle = { idx, item ->
                                if (c.alive) scope.launch {
                                    runCatching { store.client?.checklist(c.id, idx, item.text, !item.done); dash = store.client?.historyDashboard() }
                                        .onFailure { e -> error = e.message }
                                }
                            },
                        )
                    }
                    // 空的那一节整节不画，表头也不画：没有「在 AAA 里 0」这种话。
                    // 只有这一节时也不写表头——它只在「和下面那节相对」时才有意义
                    if (sections.inAaa.isNotEmpty()) {
                        if (sections.gone.isNotEmpty()) item(key = "hdr-alive", span = StaggeredGridItemSpan.FullLine) {
                            SectionHeader("在 AAA 里 ${sections.inAaa.size}")
                        }
                        items(sections.inAaa, key = { "alive-${it.id}" }) { card(it) }
                    }
                    if (sections.gone.isNotEmpty()) {
                        item(key = "hdr-gone", span = StaggeredGridItemSpan.FullLine) {
                            SectionHeader(
                                "${if (goneShown) "▾" else "▸"} 不在 AAA 里 ${sections.gone.size}",
                                // 搜索时这节由不得折：点表头也就没有意义，索性不给它点
                                onClick = if (sections.searching) null else ({ goneOpen = !goneOpen }),
                            )
                        }
                        if (goneShown) items(sections.gone, key = { "gone-${it.id}" }) { card(it) }
                    }
                }
                ScrollHint(gridState, Modifier.align(Alignment.TopEnd).padding(top = 4.dp, bottom = 24.dp, end = 1.dp))
            }
        }
    }
}

/** 分节表头：口径跟着「已删除 N」那个小标题走（Tok.Dim / 11.5sp / Monospace），占满网格一行 */
@Composable
private fun SectionHeader(text: String, onClick: (() -> Unit)? = null) {
    Text(
        text, color = Tok.Dim, fontSize = 11.5.sp, fontFamily = FontFamily.Monospace,
        modifier = Modifier.fillMaxWidth()
            .then(if (onClick == null) Modifier else Modifier.clickable(onClick = onClick))
            .padding(start = 4.dp, end = 4.dp, top = 10.dp, bottom = 2.dp),
    )
}

/**
 * 右缘那根滚动指示条（2026-09-08 用户拍板「看板这边要加个滑动条」）：卡片全展开之后一屏
 * 根本装不下，得有个东西告诉你在哪儿。
 *
 * 瀑布流的卡片高矮不一、每列还各走各的，**做不到像素精确**——这里按「可见项索引区间 /
 * 总项数」估：条长 = 可见项数 / 总项数，位置 = 已划过的项数在「总数 − 可见数」里的占比。
 * 它说的是「大概在哪儿」，不是滚动条，所以也不接拖拽。一屏装得下（可见项 ≥ 全部）就不画。
 * 停手 0.6 秒后淡出：常显的话它会在每张卡右边留一道无关的竖线，看板本来就够满了。
 */
@Composable
private fun ScrollHint(state: LazyStaggeredGridState, modifier: Modifier = Modifier) {
    val span by remember(state) {
        derivedStateOf {
            val vis = state.layoutInfo.visibleItemsInfo
            if (vis.isEmpty()) null else Triple(vis.minOf { it.index }, vis.maxOf { it.index } - vis.minOf { it.index } + 1, state.layoutInfo.totalItemsCount)
        }
    }
    val (first, shown, total) = span ?: return
    if (total <= 0 || shown >= total) return
    val scrolling = state.isScrollInProgress
    val alpha by animateFloatAsState(
        if (scrolling) 0.45f else 0f,
        tween(durationMillis = if (scrolling) 120 else 400, delayMillis = if (scrolling) 0 else 600),
        label = "scroll-hint",
    )
    if (alpha < 0.02f) return
    BoxWithConstraints(modifier.fillMaxHeight().width(3.dp)) {
        val thumb = maxHeight * (shown.toFloat() / total).coerceIn(0.06f, 1f)
        val pos = (first.toFloat() / (total - shown)).coerceIn(0f, 1f)
        Box(
            Modifier.offset(y = (maxHeight - thumb) * pos).width(3.dp).height(thumb)
                .background(Tok.Dim.copy(alpha = alpha), RoundedCornerShape(1.5.dp)),
        )
    }
}

/** 一张卡：状态字 + 标题 + 项目 → 进度条 → 全部清单项（没勾的在前、做完的灰掉）。全展开，一眼看全。会话还在才可点开 */
@Composable
private fun SessionCardView(c: SessionCard, onOpen: () -> Unit, onToggle: (Int, ChecklistItem) -> Unit = { _, _ -> }) {
    val total = c.done + c.open
    Column(
        Modifier.fillMaxWidth()
            .surfaceCard(10.dp)
            .clickable(enabled = c.alive, onClick = onOpen).padding(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            // 蓝竖线 = 还在跑；已删除的写一个字；其余什么都不画（和项目列表同一套话）
            if (cardRunning(c)) {
                MarkBar(Tok.Blue)
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
                ThinProgressBar(
                    c.done.toFloat() / total,
                    if (c.open == 0) Tok.Green else Tok.Accent,
                    Modifier.weight(1f),
                )
                Spacer(Modifier.width(8.dp))
                Text("${c.done}/$total", color = Tok.Dim, fontSize = 10.5.sp, fontFamily = FontFamily.Monospace)
            }
        } else {
            Text("没有进度清单", color = Tok.Faint, fontSize = 11.5.sp, modifier = Modifier.padding(top = 6.dp))
        }
        // 带着**原下标**一起走（重排之前先 withIndex）：POST 回去要说清是第几条，
        // 光给文字的话，清单里有两条一样的就会一起被翻过去。
        (c.items.withIndex().filter { !it.value.done } + c.items.withIndex().filter { it.value.done }).forEach { (idx, it) ->
            // 会话还在池子里就能点着勾 / 取消勾（POST /sessions/:id/checklist）
            Row(Modifier.fillMaxWidth().clickable(enabled = c.alive) { onToggle(idx, it) }.padding(top = 5.dp), verticalAlignment = Alignment.Top) {
                Text(if (it.done) "☑" else "☐", color = if (it.done) Tok.Green else Tok.Amber, fontSize = 13.sp, modifier = Modifier.width(20.dp))
                Text(it.text, color = if (it.done) Tok.Dim else Tok.Ink, fontSize = 13.sp, lineHeight = 18.sp)
            }
        }
    }
}
