package cc.uoox.aaaui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 三套配色的纯数据约束。Palette 不碰 Compose 运行时（只用 Color 这个值类），单测直接跑。
 */
class PaletteTest {
    @Test fun threeThemesEachWithItsOwnBackgroundAndAccent() {
        assertEquals(listOf("dark", "light", "claude"), Palette.all.map { it.name })
        assertEquals(listOf("黑暗", "明亮", "Claude 橙"), Palette.all.map { it.label })
        assertEquals(3, Palette.all.map { it.bg }.distinct().size)
        assertEquals(3, Palette.all.map { it.accent }.distinct().size)
    }

    @Test fun forNameResolvesKnownNamesAndFallsBackToDark() {
        assertSame(Palette.Claude, Palette.forName("claude"))
        assertSame(Palette.Light, Palette.forName("light"))
        assertSame(Palette.Dark, Palette.forName("dark"))
        // 旧版本没写过这个键 / 手改成了垃圾：都回到黑暗，不能崩
        assertSame(Palette.Dark, Palette.forName(null))
        assertSame(Palette.Dark, Palette.forName(""))
        assertSame(Palette.Dark, Palette.forName("solarized"))
        assertSame(Palette.Dark, Palette.forName("Light"))
    }

    @Test fun darkKeepsThePrototypeTokens() {
        // 黑暗 = 原 prototype.html 的值，一个都不许改
        val d = Palette.Dark
        assertEquals(Color(0xFF0E1216), d.bg)
        assertEquals(Color(0xFF1A222B), d.surface)
        assertEquals(Color(0xFFE3EBF3), d.ink)
        assertEquals(Color(0xFF53C6DD), d.accent)
        assertEquals(Color(0xFF0A0E12), d.termBg)
        assertTrue(d.isDark)
    }

    @Test fun lightThemesAreLightAndClaudeKeepsAWarmDarkTerminal() {
        assertFalse(Palette.Light.isDark)
        assertFalse(Palette.Claude.isDark)
        assertEquals(Color(0xFFD97757), Palette.Claude.accent)
        assertEquals(Color(0xFF2A2825), Palette.Claude.termBg)
        assertEquals(Color(0xFFF0EEE6), Palette.Claude.termFg)
        assertEquals(Color(0xFFFFFFFF), Palette.Light.termBg)
        assertEquals(Palette.Light.ink, Palette.Light.termFg)
    }

    @Test fun terminalForegroundContrastsWithItsBackgroundOnEveryTheme() {
        // 前景/背景至少一暗一亮，否则终端一片糊
        Palette.all.forEach { p ->
            val bg = p.termBg.luminance(); val fg = p.termFg.luminance()
            assertTrue("${p.name}: bg=$bg fg=$fg", kotlin.math.abs(bg - fg) > 0.5f)
        }
    }
}
