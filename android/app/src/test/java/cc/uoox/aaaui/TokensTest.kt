package cc.uoox.aaaui

import androidx.compose.ui.graphics.Color
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 设计令牌的三端共享向量 `fixtures/tokens.json`（mac 那边有一份对着同一个文件的测试）。
 *
 * 「两端 UI 必须一致」这句话此前只写在 PROTOCOL 里，没有任何东西盯着：终端前景一边
 * `#c9d4de` 一边纯白、magenta 一边哑紫一边直接等于 accent——两端各测各的一套，所以谁都没发现。
 *
 * 2026-09-10 只剩一套主题，向量因此拍平：顶层一个 `roles` 一个 `ansi`，没有 `themes` 这一层了。
 */
class TokensTest {
    private val fx = Json.parseToJsonElement(java.io.File("../../fixtures/tokens.json").readText()).jsonObject
    private fun hex(c: Color) = "#%06x".format((c.value shr 32).toLong() and 0xFFFFFF)

    @Test fun 十九个角色逐色对上共享向量() {
        val roles = fx["roles"]!!.jsonObject
        val actual = mapOf(
            "bg" to Tok.Bg, "surface" to Tok.Surface, "surface_raised" to Tok.Raised,
            "edge" to Tok.Edge, "edge_light" to Tok.Edge2,
            "ink" to Tok.Ink, "dim" to Tok.Dim, "faint" to Tok.Faint,
            "term_bg" to Tok.TermBg, "term_fg" to Tok.TermFg,
            "accent" to Tok.Accent, "on_accent" to Tok.OnAccent, "magenta" to Tok.Magenta,
            "green" to Tok.Green, "amber" to Tok.Amber, "red" to Tok.Red,
            "inset" to Tok.Inset,
            "row_running" to Tok.RowRunning, "row_unread" to Tok.RowUnread,
        )
        // 一个都不许漏：向量里有几个角色，这边就得画出几个
        assertEquals(roles.keys.sorted(), actual.keys.sorted())
        for ((role, color) in actual) {
            assertEquals(role, roles[role]!!.jsonPrimitive.content, hex(color))
        }
    }

    /** 终端 16 色两端逐色相同（gruvbox-light），没有「不带就用 xterm 默认表」那条回退。 */
    @Test fun 终端16色两端逐色相同() {
        val want = fx["ansi"]!!.jsonArray.map { it.jsonPrimitive.content }
        val got = Tok.TerminalAnsi.map { "#%06x".format(it and 0xFFFFFF) }
        assertEquals(want, got)
    }
}
