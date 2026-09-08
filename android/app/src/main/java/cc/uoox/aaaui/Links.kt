package cc.uoox.aaaui

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.widget.Toast
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink

// ============================================================
// 终端输出与消息流里的链接识别 / 打开
// ============================================================

/** 一段文本里的一个链接：[start] 起、[end] 止（不含），[url] 已去掉尾部标点。 */
data class UrlSpan(val start: Int, val end: Int, val url: String)

/**
 * 只认带 scheme 的绝对地址。刻意比 android.util.Patterns.WEB_URL 保守：终端里
 * 满屏都是 main.rs、Cargo.toml、a.b.c 这种路径与包名，裸域名规则会把它们统统
 * 变成点不开的假链接，比不识别还难用。
 */
// scheme 前必须是分隔符（v1.22 补）：没有这个左边界，`xhttps://a.com` 会从第二个字符起
// 被认成链接。mac 侧 `term.rs::is_boundary` 一直是这么判的，两端此前对不上。
private val URL_REGEX =
    Regex("""(?<![A-Za-z0-9.\-_+])(?:https?|ftp|file)://[^\s<>"'`\\^{}|]+""", RegexOption.IGNORE_CASE)

/** 句末标点：URL 出现在中英文句子里时它们几乎不可能是地址的一部分。 */
private const val TRAILING_PUNCT = ".,;:!?'\"“”‘’、。，；：！？…"

/**
 * 扫出 [text] 里的所有链接。尾部标点会被剥掉；成对的括号只在不配平时才剥，
 * 这样 Wikipedia 那种 `.../Foo_(bar)` 的地址不会被砍掉半截。
 */
fun findUrls(text: String): List<UrlSpan> =
    URL_REGEX.findAll(text).mapNotNull { m ->
        val trimmed = trimUrlTail(m.value)
        if (trimmed.isEmpty()) null
        else UrlSpan(m.range.first, m.range.first + trimmed.length, trimmed)
    }.toList()


private fun trimUrlTail(raw: String): String {
    var end = raw.length
    while (end > 0) {
        val c = raw[end - 1]
        when {
            TRAILING_PUNCT.indexOf(c) >= 0 -> end--
            c == ')' || c == ']' || c == '}' -> {
                val open = when (c) { ')' -> '('; ']' -> '['; else -> '{' }
                val slice = raw.substring(0, end)
                if (slice.count { it == open } < slice.count { it == c }) end-- else return slice
            }
            else -> return raw.substring(0, end)
        }
    }
    return ""
}

/**
 * 把纯文本包成带可点链接的 [AnnotatedString]。消息流是 Compose 文本，链接走
 * LinkAnnotation.Url + [onClick]，不像终端那样靠命中测试。
 */
fun linkified(text: String, onClick: (String) -> Unit): AnnotatedString {
    val spans = findUrls(text)
    if (spans.isEmpty()) return AnnotatedString(text)
    return buildAnnotatedString {
        var cursor = 0
        spans.forEach { span ->
            if (span.start > cursor) append(text.substring(cursor, span.start))
            val link = LinkAnnotation.Url(
                span.url,
                TextLinkStyles(SpanStyle(color = Tok.Accent, textDecoration = TextDecoration.Underline)),
            ) { onClick(span.url) }
            withLink(link) { append(text.substring(span.start, span.end)) }
            cursor = span.end
        }
        if (cursor < text.length) append(text.substring(cursor))
    }
}

/** 交给系统浏览器。没有 app 接得住就吐个 toast，别让点击静默失败。 */
fun openUrl(context: Context, url: String) {
    try {
        context.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    } catch (e: Exception) {
        Toast.makeText(context, "无法打开链接：$url", Toast.LENGTH_SHORT).show()
    }
}
