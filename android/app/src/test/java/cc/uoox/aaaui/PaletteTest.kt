package cc.uoox.aaaui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 两套配色的纯数据约束。Palette 不碰 Compose 运行时（只用 Color 这个值类），单测直接跑。
 */
class PaletteTest {
    @Test fun twoThemesEachWithItsOwnBackgroundAndAccent() {
        assertEquals(listOf("dark", "claude"), Palette.all.map { it.name })
        assertEquals(listOf("黑暗", "Claude 橙"), Palette.all.map { it.label })
        assertEquals(2, Palette.all.map { it.bg }.distinct().size)
        assertEquals(2, Palette.all.map { it.accent }.distinct().size)
    }

    @Test fun forNameResolvesKnownNamesAndFallsBackToDark() {
        assertSame(Palette.Claude, Palette.forName("claude"))
        assertSame(Palette.Dark, Palette.forName("dark"))
        // 2026-09-08 删掉的那套：存量设置里的 "light" 静静回黑暗，不能崩
        assertSame(Palette.Dark, Palette.forName("light"))
        // 旧版本没写过这个键 / 手改成了垃圾：都回到黑暗，不能崩
        assertSame(Palette.Dark, Palette.forName(null))
        assertSame(Palette.Dark, Palette.forName(""))
        assertSame(Palette.Dark, Palette.forName("solarized"))
    }

    @Test fun darkKeepsThePrototypeTokens() {
        // 黑暗 = PROTOCOL.md 设计令牌表的原始值，一个都不许改
        val d = Palette.Dark
        assertEquals(Color(0xFF0E1216), d.bg)
        assertEquals(Color(0xFF1A222B), d.surface)
        assertEquals(Color(0xFFE3EBF3), d.ink)
        assertEquals(Color(0xFF53C6DD), d.accent)
        assertEquals(Color(0xFF0A0E12), d.termBg)
        assertTrue(d.isDark)
    }

    @Test fun claudeIsTheLightThemeAndItsTerminalIsLightToo() {
        assertFalse(Palette.Claude.isDark)
        assertEquals(Color(0xFFD97757), Palette.Claude.accent)
        // Claude 橙的终端底是暖白，字就是界面的墨
        assertTrue(Palette.Claude.termBg.luminance() > 0.9f)
        assertEquals(Palette.Claude.ink, Palette.Claude.termFg)
    }

    @Test fun everyThemeBringsItsOwnAnsiAndLightOnesStayReadable() {
        // v1.23：两套主题都自带 16 色（深色那套与 mac 逐色相同，见 TokensTest）——
        // 以前深色沿用 termux 出厂表，同一段输出两端颜色不一样。亮底那套还得没有一个色比底还亮。
        assertEquals(16, Palette.Dark.ansi?.size)
        listOf(Palette.Claude).forEach { p ->
            val ansi = p.ansi!!
            assertEquals(16, ansi.size)
            ansi.forEachIndexed { i, c ->
                assertTrue("${p.name}[$i] 在亮底上看不见", c.luminance() < 0.6f)
            }
            // 15 bright white = 墨色，7 white 是灰而不是白
            assertEquals(p.ink, ansi[15])
            assertTrue(ansi[7].luminance() > ansi[15].luminance())
        }
    }

    @Test fun terminalForegroundContrastsWithItsBackgroundOnEveryTheme() {
        // 前景/背景至少一暗一亮，否则终端一片糊
        Palette.all.forEach { p ->
            val bg = p.termBg.luminance(); val fg = p.termFg.luminance()
            assertTrue("${p.name}: bg=$bg fg=$fg", kotlin.math.abs(bg - fg) > 0.5f)
        }
    }
}
