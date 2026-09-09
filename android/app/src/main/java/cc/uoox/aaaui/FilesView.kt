package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.launch

// ============================================================
// v1.30 目录浏览：会话的第三种看法（消息流 / 终端 / 浏览，顶栏点一下轮换）。
//
// 只读，且只在**项目根底下**——守卫在 daemon 那一层（`files.rs`），这边不重判一遍。
// 点目录进去，点文件打开：`.md` 直接按 CommonMark 渲染（与消息流同一个 [MarkdownBody]），
// `.html` 丢进 WebView 直接画出来（右上角可以切回源码），其余文本等宽 + 横向滚动，
// 二进制只报大小。
//
// **html 为什么不是「交给系统默认程序打开」**：文件在 Mac 那台机器上，手机这边根本没有
// 这个文件。要交给别的 app 就得先把正文落到本地缓存再发 Intent，绕一圈还只能看没有图片
// 样式的半成品——而 WebView 本来就在系统里，`loadDataWithBaseURL` 一句就画出来了。
// 相对路径引的图片和 css 仍然拿不到（那要 daemon 当静态服务器），所以它是预览不是浏览器。
//
// mac 那边同源同构（`mac/src/ui/files_view.rs`），两处的排序、大小写法、面包屑口径一致。
// ============================================================

/** 文件大小写成人话，与 mac 的 `human_size` 同一口径 */
fun humanSize(n: Long): String {
    if (n < 1024) return "$n B"
    var v = n.toDouble() / 1024
    for (unit in listOf("KB", "MB", "GB")) {
        if (v < 1024 || unit == "GB") {
            return if (v < 10) String.format("%.1f %s", v, unit) else String.format("%.0f %s", v, unit)
        }
        v /= 1024
    }
    return "$n B"
}

/** 面包屑：项目目录显示成它的名字，底下的接在后面（与 mac 的 `crumb` 同口径） */
fun fileCrumb(root: String, dir: String): String {
    val base = root.substringAfterLast('/')
    if (dir == root) return base
    val rest = dir.removePrefix(root)
    return if (rest != dir) "$base$rest" else dir
}

/**
 * 目录浏览视图。`root` = 会话的项目目录，也是「上一级」的下界。
 *
 * 状态（当前目录、打开的文件）留在本 composable 里而不是提到会话屏：切到终端再切回来
 * 时希望回到原处，而会话屏本身不关心你翻到了哪一层。
 */
@Composable
fun FilesView(store: AppStore, root: String) {
    var dir by rememberSaveable(root) { mutableStateOf(root) }
    var parent by rememberSaveable(root) { mutableStateOf<String?>(null) }
    var entries by remember(root) { mutableStateOf<List<FileEntry>>(emptyList()) }
    var truncated by remember(root) { mutableStateOf(false) }
    var viewing by remember(root) { mutableStateOf<FileBody?>(null) }
    var error by remember(root) { mutableStateOf<String?>(null) }
    var reloads by remember(root) { mutableStateOf(0) }

    // 目录变了（或按了刷新）就重列。打开的文件由点击那一刻直接拉，不走这里
    LaunchedEffect(dir, reloads) {
        try {
            val r = store.client?.files(dir) ?: return@LaunchedEffect
            error = null
            parent = r.parent
            entries = r.entries
            truncated = r.truncated
        } catch (e: Exception) {
            error = e.message ?: "列目录失败"
        }
    }

    val scope = rememberCoroutineScope()
    val openFile: (String) -> Unit = { path ->
        scope.launch {
            try {
                viewing = store.client?.fileRead(path)
                error = null
            } catch (e: Exception) {
                error = e.message ?: "读取失败"
            }
        }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg)) {
        FilesBar(
            label = viewing?.let { "${fileCrumb(root, dir)} · ${it.name}" } ?: fileCrumb(root, dir),
            canUp = viewing != null || parent != null,
            onUp = {
                if (viewing != null) viewing = null else parent?.let { dir = it }
            },
            onReload = { viewing?.let { openFile(it.path) } ?: run { reloads++ } },
        )
        error?.let {
            Text(
                it, color = Tok.Red, fontSize = 11.sp,
                modifier = Modifier.fillMaxWidth().background(Tok.Red.copy(alpha = 0.08f)).padding(horizontal = 12.dp, vertical = 5.dp),
            )
        }
        val file = viewing
        if (file != null) {
            FileBodyView(file)
        } else {
            LazyColumn(Modifier.fillMaxSize()) {
                items(entries, key = { it.path }) { e ->
                    FileRow(e) { if (e.dir) { viewing = null; dir = e.path } else openFile(e.path) }
                }
                if (entries.isEmpty()) {
                    item { Text("空目录", color = Tok.Faint, fontSize = 12.sp, modifier = Modifier.padding(12.dp)) }
                }
                if (truncated) {
                    item { Text("条目太多，只列了前面一部分", color = Tok.Amber, fontSize = 10.5.sp, modifier = Modifier.padding(12.dp)) }
                }
            }
        }
    }
}

@Composable
private fun FilesBar(label: String, canUp: Boolean, onUp: () -> Unit, onReload: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            "↰ 上一级", color = if (canUp) Tok.Accent else Tok.Faint, fontSize = 12.sp,
            modifier = Modifier.let { if (canUp) it.clickable(onClick = onUp) else it }.padding(horizontal = 6.dp, vertical = 4.dp),
        )
        Spacer(Modifier.width(6.dp))
        Text(
            label, color = Tok.Dim, fontSize = 11.sp, fontFamily = FontFamily.Monospace,
            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
        )
        Text(
            "刷新", color = Tok.Faint, fontSize = 11.sp,
            modifier = Modifier.clickable(onClick = onReload).padding(horizontal = 8.dp, vertical = 4.dp),
        )
    }
    HorizontalDivider(color = Tok.Edge)
}

@Composable
private fun FileRow(e: FileEntry, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(if (e.dir) "▸" else "·", color = Tok.Faint, fontSize = 12.sp, modifier = Modifier.width(18.dp))
        Text(
            e.name,
            color = if (e.dir) Tok.Ink else if (e.kind == "binary") Tok.Faint else Tok.Dim,
            fontSize = 14.sp,
            fontWeight = if (e.dir) FontWeight.Medium else FontWeight.Normal,
            maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
        )
        if (!e.dir) {
            Text(humanSize(e.size), color = Tok.Faint, fontSize = 10.sp, fontFamily = FontFamily.Monospace)
        }
    }
}

@Composable
private fun FileBodyView(f: FileBody) {
    // html 默认画出来；右上角切「源码」看原文。切换状态按文件路径记，翻到别的文件回到默认
    var asSource by rememberSaveable(f.path) { mutableStateOf(false) }
    if (f.kind == "html" && !asSource) {
        Column(Modifier.fillMaxSize()) {
            SourceToggle(asSource) { asSource = it }
            HtmlPreview(f.text, Modifier.weight(1f))
        }
        return
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 12.dp)) {
        if (f.kind == "html") SourceToggle(asSource) { asSource = it }
        when {
            f.kind == "binary" -> Text("${f.name}：二进制文件，${humanSize(f.size)}", color = Tok.Faint, fontSize = 13.sp)
            f.kind == "markdown" -> MarkdownBody(f.text, 14.sp, Tok.Ink)
            else -> Box(Modifier.horizontalScroll(rememberScrollState())) {
                Text(f.text, color = Tok.TermFg, fontSize = 12.sp, fontFamily = FontFamily.Monospace, softWrap = false)
            }
        }
        if (f.truncated) {
            Spacer(Modifier.width(8.dp))
            Text("文件太大，只读了前面一段（共 ${humanSize(f.size)}）", color = Tok.Amber, fontSize = 10.5.sp, modifier = Modifier.padding(top = 10.dp))
        }
    }
}

/** html 的「画出来 ⇄ 看源码」一行开关 */
@Composable
private fun SourceToggle(asSource: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp), horizontalArrangement = Arrangement.End) {
        Text(
            if (asSource) "预览" else "源码",
            color = Tok.Accent, fontSize = 12.sp, fontFamily = FontFamily.Monospace,
            modifier = Modifier.clickable { onChange(!asSource) }.padding(horizontal = 8.dp, vertical = 4.dp),
        )
    }
}

/**
 * html 预览：系统自带的 WebView，正文直接喂进去（不联网、不执行外部脚本——
 * `baseUrl` 给 null，相对路径的图片和 css 本来也拿不到）。
 */
@Composable
private fun HtmlPreview(html: String, modifier: Modifier = Modifier) {
    AndroidView(
        modifier = modifier.fillMaxWidth(),
        factory = { ctx ->
            android.webkit.WebView(ctx).apply {
                settings.javaScriptEnabled = false
                settings.loadsImagesAutomatically = true
                // 项目里的 html 是别人写的：不给它读手机上的本地文件
                settings.allowFileAccess = false
                settings.allowContentAccess = false
                isVerticalScrollBarEnabled = true
            }
        },
        // **只在正文真的换了的时候重载**：`update` 每次重组都会跑，无脑 load 一次
        // 页面就重画一次、滚动位置回到顶上（换主题、开合键盘都会重组）
        update = { web ->
            if (web.tag != html) {
                web.tag = html
                web.loadDataWithBaseURL(null, html, "text/html", "utf-8", null)
            }
        },
    )
}
