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
 * 「两端 UI 必须一致」这句话此前只写在 PROTOCOL 里，没有任何东西盯着：深色终端前景一边
 * `#c9d4de` 一边纯白、Claude 主题的 magenta 一边哑紫一边直接等于 accent——两端各测各的
 * 一套，所以谁都没发现。
 */
class TokensTest {
    private val fx = Json.parseToJsonElement(java.io.File("../../fixtures/tokens.json").readText()).jsonObject
    private fun theme(name: String) = fx["themes"]!!.jsonObject[name]!!.jsonObject
    private fun hex(c: Color) = "#%06x".format(c.value.toString().let { _ -> (c.value shr 32).toLong() and 0xFFFFFF })

    private fun assertRoles(name: String, p: Palette) {
        val roles = theme(name)["roles"]!!.jsonObject
        val actual = mapOf(
            "bg" to p.bg, "surface" to p.surface, "surface_raised" to p.raised,
            "edge" to p.edge, "edge_light" to p.edge2,
            "ink" to p.ink, "dim" to p.dim, "faint" to p.faint,
            "term_bg" to p.termBg, "term_fg" to p.termFg,
            "accent" to p.accent, "magenta" to p.magenta,
            "green" to p.green, "amber" to p.amber, "red" to p.red, "blue" to p.blue,
            "inset" to p.inset,
        )
        for ((role, color) in actual) {
            assertEquals("$name.$role", roles[role]!!.jsonPrimitive.content, hex(color))
        }
    }

    @Test fun 黑暗主题逐色对上共享向量() = assertRoles("dark", Palette.Dark)

    @Test fun claude主题逐色对上共享向量() = assertRoles("claude", Palette.Claude)

    /** v1.23：**两套**主题的终端 16 色都两端逐色相同（深色那套以前 Android 用的是 xterm 默认）。 */
    @Test fun 两套主题的终端16色两端逐色相同() {
        for (name in listOf("dark", "claude")) {
            val want = theme(name)["ansi"]!!.jsonArray.map { it.jsonPrimitive.content }
            val p = Palette.all.first { it.name == name }
            val got = terminalAnsi(p).map { "#%06x".format(it and 0xFFFFFF) }
            assertEquals(name, want, got)
        }
    }
}
