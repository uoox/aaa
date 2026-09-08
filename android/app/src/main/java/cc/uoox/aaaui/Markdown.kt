package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.commonmark.ext.gfm.strikethrough.Strikethrough
import org.commonmark.ext.gfm.strikethrough.StrikethroughExtension
import org.commonmark.ext.gfm.tables.TableBlock
import org.commonmark.ext.gfm.tables.TableBody
import org.commonmark.ext.gfm.tables.TableCell
import org.commonmark.ext.gfm.tables.TableHead
import org.commonmark.ext.gfm.tables.TableRow
import org.commonmark.ext.gfm.tables.TablesExtension
import org.commonmark.node.BlockQuote
import org.commonmark.node.BulletList
import org.commonmark.node.Code
import org.commonmark.node.Emphasis
import org.commonmark.node.FencedCodeBlock
import org.commonmark.node.HardLineBreak
import org.commonmark.node.Heading
import org.commonmark.node.HtmlBlock
import org.commonmark.node.HtmlInline
import org.commonmark.node.Image
import org.commonmark.node.IndentedCodeBlock
import org.commonmark.node.Link
import org.commonmark.node.ListBlock
import org.commonmark.node.Node
import org.commonmark.node.OrderedList
import org.commonmark.node.Paragraph
import org.commonmark.node.SoftLineBreak
import org.commonmark.node.StrongEmphasis
import org.commonmark.node.Text as CommonText
import org.commonmark.node.ThematicBreak
import org.commonmark.parser.Parser
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.rememberTextMeasurer

data class MdSpan(val text: String, val bold: Boolean = false, val italic: Boolean = false, val code: Boolean = false, val strike: Boolean = false, val link: String? = null)

sealed class MdBlock {
    data class Heading(val level: Int, val spans: List<MdSpan>) : MdBlock()
    data class Paragraph(val spans: List<MdSpan>) : MdBlock()
    data class Code(val lang: String?, val text: String) : MdBlock()
    data class ListBlock(val ordered: Boolean, val start: Int, val items: List<List<MdBlock>>) : MdBlock()
    data class Quote(val blocks: List<MdBlock>) : MdBlock()
    object Rule : MdBlock()
    data class Table(val header: List<List<MdSpan>>, val rows: List<List<List<MdSpan>>>) : MdBlock()
}

private val mdParser = Parser.builder().extensions(listOf(TablesExtension.create(), StrikethroughExtension.create())).build()

fun parseMarkdown(text: String): List<MdBlock> = try { parseBlocks(mdParser.parse(text)) } catch (_: Exception) {
    listOf(MdBlock.Paragraph(listOf(MdSpan(text))))
}

private fun children(node: Node): List<Node> = buildList {
    var child = node.firstChild
    while (child != null) { add(child); child = child.next }
}

private fun parseBlocks(container: Node): List<MdBlock> = children(container).flatMap { node -> when (node) {
    is Heading -> listOf(MdBlock.Heading(node.level, parseSpans(node)))
    is Paragraph -> listOf(MdBlock.Paragraph(parseSpans(node)))
    is HtmlBlock -> listOf(MdBlock.Paragraph(listOf(MdSpan(node.literal))))
    is FencedCodeBlock -> listOf(MdBlock.Code(node.info.trim().ifBlank { null }, node.literal))
    is IndentedCodeBlock -> listOf(MdBlock.Code(null, node.literal))
    is BulletList -> listOf(parseList(node, false, 1))
    is OrderedList -> listOf(parseList(node, true, node.startNumber))
    is BlockQuote -> listOf(MdBlock.Quote(parseBlocks(node)))
    is ThematicBreak -> listOf(MdBlock.Rule)
    is TableBlock -> listOf(parseTable(node))
    else -> parseBlocks(node)
} }

private fun parseList(node: ListBlock, ordered: Boolean, start: Int) = MdBlock.ListBlock(ordered, start, children(node).map { parseBlocks(it) })

private fun parseTable(node: TableBlock): MdBlock.Table {
    fun row(row: TableRow) = children(row).filterIsInstance<TableCell>().map(::parseSpans)
    val head = children(node).filterIsInstance<TableHead>().firstOrNull()
    val body = children(node).filterIsInstance<TableBody>().firstOrNull()
    val header = head?.let { children(it).filterIsInstance<TableRow>().firstOrNull()?.let(::row) }.orEmpty()
    val rows = body?.let { children(it).filterIsInstance<TableRow>().map(::row) }.orEmpty()
    return MdBlock.Table(header, rows)
}

private fun parseSpans(container: Node): List<MdSpan> = buildList {
    fun visit(node: Node, bold: Boolean = false, italic: Boolean = false, code: Boolean = false, strike: Boolean = false, link: String? = null) {
        when (node) {
            is CommonText -> add(MdSpan(node.literal, bold, italic, code, strike, link))
            is HtmlInline -> add(MdSpan(node.literal, bold, italic, code, strike, link))
            is SoftLineBreak -> add(MdSpan(" ", bold, italic, code, strike, link))
            is HardLineBreak -> add(MdSpan("\n", bold, italic, code, strike, link))
            is Code -> add(MdSpan(node.literal, bold, italic, true, strike, link))
            is Emphasis -> children(node).forEach { visit(it, bold, true, code, strike, link) }
            is StrongEmphasis -> children(node).forEach { visit(it, true, italic, code, strike, link) }
            is Strikethrough -> children(node).forEach { visit(it, bold, italic, code, true, link) }
            is Link -> children(node).forEach { visit(it, bold, italic, code, strike, node.destination) }
            is Image -> add(MdSpan(flatten(node).ifBlank { node.title.orEmpty().ifBlank { node.destination } }, bold, italic, code, strike, node.destination))
            else -> children(node).forEach { visit(it, bold, italic, code, strike, link) }
        }
    }
    children(container).forEach { visit(it) }
}

private fun flatten(node: Node): String = buildString {
    fun visit(n: Node) { when (n) {
        is CommonText -> append(n.literal)
        is SoftLineBreak -> append(' ')
        is HardLineBreak -> append('\n')
        else -> children(n).forEach(::visit)
    } }
    visit(node)
}

/** 随包的 JetBrains Mono 组成 Compose 字体族（读一次缓存）；读不到就退系统等宽。 */
@Composable
private fun monoFamily(): FontFamily {
    val assets = LocalContext.current.assets
    return remember { runCatching { FontFamily(Font("fonts/JetBrainsMonoNL-Regular.ttf", assets)) }.getOrDefault(FontFamily.Monospace) }
}

private fun TextUnit.plus(delta: Float): TextUnit = (value + delta).sp
private fun TextUnit.times(k: Float): TextUnit = (value * k).sp

/**
 * assistant 文本的 Markdown 正文：块列（标题 / 段落 / 代码 / 列表 / 引用 / 分隔线 / 表格）。
 * 解析结果按 text 记忆；解析为空（纯空白等）退回今天的纯文本 + 链接高亮。
 */
@Composable
fun MarkdownBody(text: String, baseSize: TextUnit, color: Color, modifier: Modifier = Modifier) {
    val blocks = remember(text) { parseMarkdown(text) }
    val mono = monoFamily()
    val context = LocalContext.current
    val onLink: (String) -> Unit = { openUrl(context, it) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (blocks.isEmpty()) {
            Text(linkified(text, onLink), color = color, fontSize = baseSize, lineHeight = baseSize.times(1.45f))
        } else {
            blocks.forEach { MarkdownBlock(it, baseSize, color, mono, onLink) }
        }
    }
}

@Composable
private fun MarkdownBlock(block: MdBlock, size: TextUnit, color: Color, mono: FontFamily, onLink: (String) -> Unit) {
    when (block) {
        is MdBlock.Heading -> Text(
            styled(block.spans, mono, onLink), color = Tok.Ink, fontWeight = FontWeight.Bold,
            fontSize = size.plus(when (block.level) { 1 -> 3f; 2 -> 1.5f; else -> 0.5f }),
            lineHeight = size.plus(when (block.level) { 1 -> 3f; 2 -> 1.5f; else -> 0.5f }).times(1.35f),
        )
        is MdBlock.Paragraph -> Text(styled(block.spans, mono, onLink), color = color, fontSize = size, lineHeight = size.times(1.45f))
        is MdBlock.Code -> Box(Modifier.fillMaxWidth().insetPanel().padding(horizontal = 10.dp, vertical = 8.dp)) {
            Row(Modifier.horizontalScroll(rememberScrollState())) {
                Text(block.text, color = Tok.Ink, fontFamily = mono, fontSize = size.plus(-1.5f), lineHeight = size.plus(-1.5f).times(1.4f), softWrap = false)
            }
            if (!block.lang.isNullOrBlank()) Text(block.lang, color = Tok.Faint, fontSize = 10.sp, modifier = Modifier.align(Alignment.TopEnd))
        }
        is MdBlock.ListBlock -> Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
            block.items.forEachIndexed { i, item ->
                Row(Modifier.fillMaxWidth()) {
                    Text(
                        if (block.ordered) "${block.start + i}." else "•",
                        color = Tok.Dim, fontSize = size, lineHeight = size.times(1.45f), modifier = Modifier.width(22.dp),
                    )
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) { item.forEach { MarkdownBlock(it, size, color, mono, onLink) } }
                }
            }
        }
        is MdBlock.Quote -> Row(Modifier.fillMaxWidth().height(IntrinsicSize.Min).padding(vertical = 2.dp)) {
            Box(Modifier.width(3.dp).fillMaxHeight().background(Tok.Faint))
            Column(Modifier.padding(start = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) { block.blocks.forEach { MarkdownBlock(it, size, Tok.Dim, mono, onLink) } }
        }
        MdBlock.Rule -> Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).height(1.dp).background(Tok.Edge2))
        is MdBlock.Table -> MarkdownTable(block, size, mono, onLink)
    }
}

/** 行内片段 → AnnotatedString：粗 / 斜 / 行内代码（等宽 + 底色）/ 删除线 / 链接（强调色下划线，可点）。 */
private fun styled(spans: List<MdSpan>, mono: FontFamily, onLink: (String) -> Unit): AnnotatedString = buildAnnotatedString {
    spans.forEach { s ->
        val style = SpanStyle(
            fontWeight = if (s.bold) FontWeight.Bold else null,
            fontStyle = if (s.italic) FontStyle.Italic else null,
            fontFamily = if (s.code) mono else null,
            background = if (s.code) Tok.Inset else Color.Unspecified,
            color = if (s.code || s.link != null) Tok.Accent else Color.Unspecified,
            textDecoration = when {
                s.strike && s.link != null -> TextDecoration.combine(listOf(TextDecoration.LineThrough, TextDecoration.Underline))
                s.strike -> TextDecoration.LineThrough
                s.link != null -> TextDecoration.Underline
                else -> null
            },
        )
        val url = s.link
        if (url != null) {
            withLink(LinkAnnotation.Url(url, TextLinkStyles(style)) { onLink(url) }) { withStyle(style) { append(s.text) } }
        } else {
            withStyle(style) { append(s.text) }
        }
    }
}

/**
 * 表格：真正的网格，列宽 = 该列最宽单元格**按实际排版测出来的像素**（+ 内边距）。
 * 以前是拼成等宽文本靠空格对齐——中文落到备用字体时并不是等宽字体的两倍宽，列就漂了
 * （2026-09-07 用户反馈「表格不太整齐」）。单元格里的粗体 / 行内代码 / 链接照常渲染；
 * 表头加粗、下加一条线；整体比消息宽时横向滚动。
 */
@Composable
private fun MarkdownTable(table: MdBlock.Table, size: TextUnit, mono: FontFamily, onLink: (String) -> Unit) {
    val measurer = rememberTextMeasurer()
    val density = LocalDensity.current
    val cellSize = size.plus(-1.5f)
    val style = TextStyle(fontSize = cellSize, fontFamily = mono)
    val all = listOf(table.header) + table.rows
    val cols = all.maxOfOrNull { it.size } ?: 0
    if (cols == 0) return
    val pad = 8.dp
    val widths = remember(table, size) {
        (0 until cols).map { c ->
            val maxPx = all.maxOf { row -> row.getOrNull(c)?.let { measurer.measure(styled(it, mono, onLink), style).size.width } ?: 0 }
            with(density) { maxPx.toDp() } + pad * 2
        }
    }
    @Composable
    fun line(row: List<List<MdSpan>>, head: Boolean, zebra: Boolean) {
        Row(Modifier.background(if (zebra) Tok.Edge.copy(alpha = 0.25f) else Color.Transparent)) {
            widths.forEachIndexed { i, w ->
                Box(Modifier.width(w).padding(horizontal = pad, vertical = 4.dp)) {
                    Text(
                        styled(row.getOrNull(i).orEmpty(), mono, onLink), color = Tok.Ink, fontFamily = mono,
                        fontSize = cellSize, lineHeight = cellSize.times(1.4f), softWrap = false,
                        fontWeight = if (head) FontWeight.Bold else null,
                    )
                }
            }
        }
    }
    Column(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).insetPanel().padding(vertical = 4.dp)) {
        line(table.header, head = true, zebra = false)
        Box(Modifier.width(widths.fold(0.dp) { a, b -> a + b }).height(1.dp).background(Tok.Edge2))
        table.rows.forEachIndexed { i, r -> line(r, head = false, zebra = i % 2 == 1) }
    }
}
