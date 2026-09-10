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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.navigation.NavHostController

// ============================================================
// v1.35 读一份产物：详情屏「产物」里点开一份项目 Markdown 就到这一屏。
//
// 这是 v1.30 那个目录 explorer（会话的第三种看法「浏览」）的**剩下那一半**：
// 2026-09-10 用户拍板不要文件管理器，要的是「项目生成的 Markdown 排进产物里，
// 并且能读」。翻目录那半截整个删了，读文件这半截留下来。
//
// 只读，且只在**项目根底下**——守卫在 daemon 那一层（`files.rs`），这边不重判一遍。
// `.md` 按 CommonMark 渲染（与消息流同一个 [MarkdownBody]），`.html` 丢进 WebView
// 直接画出来（右上角可以切回源码），其余文本等宽 + 横向滚动，二进制只报大小。
//
// **html 为什么不是「交给系统默认程序打开」**：文件在 Mac 那台机器上，手机这边根本没有
// 这个文件。要交给别的 app 就得先把正文落到本地缓存再发 Intent，绕一圈还只能看没有图片
// 样式的半成品——而 WebView 本来就在系统里，`loadDataWithBaseURL` 一句就画出来了。
// 相对路径引的图片和 css 仍然拿不到（那要 daemon 当静态服务器），所以它是预览不是浏览器。
//
// mac 那边同源同构（`mac/src/ui/doc_view.rs`），两处的大小写法与标题口径一致。
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

/**
 * 产物阅读屏。`path` 是 daemon 给的绝对路径（详情屏那一行上就带着），`title` 是
 * 顶栏写的那几个字——一份 `docs/api.md` 在顶栏上写全 `docs/api.md` 比只写 `api.md`
 * 有用，因为同名的 README 可能有好几份。
 */
@Composable
fun DocScreen(store: AppStore, nav: NavHostController, path: String, title: String) {
    var file by remember(path) { mutableStateOf<FileBody?>(null) }
    var error by remember(path) { mutableStateOf<String?>(null) }
    var reloads by remember(path) { mutableStateOf(0) }

    LaunchedEffect(path, reloads) {
        try {
            file = store.client?.fileRead(path)
            error = null
        } catch (e: Exception) {
            error = e.message ?: "读取失败"
        }
    }

    Column(Modifier.fillMaxSize().background(Tok.Bg).navigationBarsPadding()) {
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            BackArrow(onClick = { nav.popBackStack() }, fontSize = 28.sp)
            Text(
                title.ifBlank { file?.name.orEmpty() },
                color = Tok.Ink, fontSize = 16.sp, fontWeight = FontWeight.Bold,
                maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f),
            )
            Text(
                "刷新", color = Tok.Faint, fontSize = 12.sp,
                modifier = Modifier.clickable { reloads++ }.padding(horizontal = 8.dp, vertical = 4.dp),
            )
        }
        HorizontalDivider(color = Tok.Edge)
        error?.let {
            Text(
                it, color = Tok.Red, fontSize = 11.sp,
                modifier = Modifier.fillMaxWidth().background(Tok.Red.copy(alpha = 0.08f)).padding(horizontal = 12.dp, vertical = 5.dp),
            )
        }
        val f = file
        if (f == null) {
            Text(
                if (error == null) "读取中…" else "没有内容",
                color = Tok.Faint, fontSize = 12.sp, modifier = Modifier.padding(14.dp),
            )
        } else {
            FileBodyView(f)
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
        // 报告是拿来抄走用的：整篇包一个选区容器，长按起选、跨段拖、走系统的复制条
        SelectionContainer {
            when {
                f.kind == "binary" -> Text("${f.name}：二进制文件，${humanSize(f.size)}", color = Tok.Faint, fontSize = 13.sp)
                f.kind == "markdown" -> MarkdownBody(f.text, 14.sp, Tok.Ink)
                else -> Box(Modifier.horizontalScroll(rememberScrollState())) {
                    Text(f.text, color = Tok.TermFg, fontSize = 12.sp, fontFamily = FontFamily.Monospace, softWrap = false)
                }
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
