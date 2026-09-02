package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MarkdownTest {
    private fun text(spans: List<MdSpan>) = spans.joinToString("") { it.text }
    @Test fun headingsKeepLevelsAndText() {
        val blocks = parseMarkdown("# one\n## two\n### three")
        assertEquals(listOf(1, 2, 3), blocks.map { (it as MdBlock.Heading).level })
        assertEquals(listOf("one", "two", "three"), blocks.map { text((it as MdBlock.Heading).spans) })
    }
    @Test fun inlineStylesProduceFlagsAndLinks() {
        val spans = (parseMarkdown("**bold** *italic* \u0060code\u0060 [link](https://a.dev) ~~gone~~").single() as MdBlock.Paragraph).spans
        assertTrue(spans.any { it.text == "bold" && it.bold }); assertTrue(spans.any { it.text == "italic" && it.italic })
        assertTrue(spans.any { it.text == "code" && it.code }); assertTrue(spans.any { it.text == "link" && it.link == "https://a.dev" })
        assertTrue(spans.any { it.text == "gone" && it.strike })
    }
    @Test fun fencedCodeHasLanguageAndLiteralText() {
        val code = parseMarkdown("\u0060\u0060\u0060kotlin\nval x = 1\n\u0060\u0060\u0060").single() as MdBlock.Code
        assertEquals("kotlin", code.lang); assertEquals("val x = 1\n", code.text)
    }
    @Test fun nestedBulletListRetainsOrderedStart() {
        val outer = parseMarkdown("3. foo\n   - bar").single() as MdBlock.ListBlock
        assertTrue(outer.ordered); assertEquals(3, outer.start)
        assertTrue(outer.items.single().any { it is MdBlock.ListBlock && !(it as MdBlock.ListBlock).ordered })
    }
    @Test fun quoteAndRuleAreBlocks() {
        assertTrue(parseMarkdown("> quoted").single() is MdBlock.Quote); assertTrue(parseMarkdown("\n---\n").single() is MdBlock.Rule)
    }
    @Test fun gfmTableHasHeaderAndTwoRows() {
        val table = parseMarkdown("| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |").single() as MdBlock.Table
        assertEquals(2, table.header.size); assertEquals(2, table.rows.size); assertEquals("2", text(table.rows[0][1]))
    }
    @Test fun bareUrlIsOneParagraph() { assertEquals(1, parseMarkdown("See https://a.dev/x now").size) }
    @Test fun adversarialInputNeverThrows() { assertNotNull(parseMarkdown("\u0060\u0060\u0060\n** [unterminated ( [ [\n* _ ] ]")) }
}
