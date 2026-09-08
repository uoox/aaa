package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 终端输出 / 消息流里的链接识别。样本刻意取真实终端里会出现的形状：
 * dev server 地址、编译报错里的文档链接、中文句子夹着的地址、以及一堆
 * 长得像域名但不是链接的路径与包名。
 */
class LinksTest {
    /** 单个词是不是链接（终端点击的判定形状）。生产代码走 `findUrls`，这个壳只有测试用。 */
    private fun urlInWord(word: String): String? = findUrls(word).firstOrNull()?.url


    private fun urls(text: String) = findUrls(text).map { it.url }

    @Test fun findsSchemeUrls() {
        assertEquals(
            listOf("http://127.0.0.1:5173/", "https://docs.rs/tokio/latest"),
            urls("Local: http://127.0.0.1:5173/ 文档见 https://docs.rs/tokio/latest"),
        )
    }

    @Test fun ignoresBareDomainsAndPaths() {
        // 终端里满屏都是这些，认成链接比不认还难用
        assertEquals(emptyList<String>(), urls("main.rs Cargo.toml src/app.kt example.com www.foo.cn"))
    }

    @Test fun trimsSentencePunctuation() {
        assertEquals(listOf("https://a.dev/x"), urls("见 https://a.dev/x。"))
        assertEquals(listOf("https://a.dev/x"), urls("see https://a.dev/x, then"))
        assertEquals(listOf("https://a.dev/x"), urls("https://a.dev/x!"))
    }

    @Test fun keepsBalancedParensButDropsWrappingOnes() {
        assertEquals(
            listOf("https://en.wikipedia.org/wiki/Curl_(math)"),
            urls("https://en.wikipedia.org/wiki/Curl_(math)"),
        )
        assertEquals(listOf("https://a.dev/x"), urls("(见 https://a.dev/x)"))
    }

    @Test fun spansPointAtTheOriginalText() {
        val text = "启动于 https://a.dev/x 之后"
        val span = findUrls(text).single()
        assertEquals("https://a.dev/x", text.substring(span.start, span.end))
    }

    @Test fun stopsAtWhitespaceAndQuotes() {
        assertEquals(listOf("https://a.dev/x"), urls("""curl "https://a.dev/x" -v"""))
        assertEquals(listOf("https://a.dev/x"), urls("<https://a.dev/x>"))
    }

    @Test fun urlInWordMatchesTerminalTapTargets() {
        // TerminalBuffer.getWordAtLocation 给出的是空白分隔的一个词
        assertEquals("https://a.dev/x", urlInWord("https://a.dev/x"))
        assertEquals("file:///tmp/report.html", urlInWord("file:///tmp/report.html"))
        assertNull(urlInWord("src/main.rs:42"))
        assertNull(urlInWord(""))
    }

    @Test fun handlesMultipleUrlsAndNoUrls() {
        assertEquals(
            listOf("http://a.dev", "http://b.dev"),
            urls("http://a.dev 和 http://b.dev"),
        )
        assertEquals(emptyList<String>(), urls("完全没有链接的一行输出"))
    }

    /** scheme 前必须是分隔符：没有左边界，`xhttps://a.com` 会从第二个字符起被认成链接。 */
    @Test
    fun 半截词里的scheme不算链接() {
        assertEquals(emptyList<String>(), findUrls("xhttps://a.com").map { it.url })
        assertEquals(emptyList<String>(), findUrls("a.https://a.com").map { it.url })
        assertEquals(listOf("https://a.com"), findUrls("见 https://a.com").map { it.url })
        assertEquals(listOf("https://a.com"), findUrls("(https://a.com)").map { it.url })
    }
}
