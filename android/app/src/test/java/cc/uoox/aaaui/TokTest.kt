package cc.uoox.aaaui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 唯一那套配色的纯数据约束（2026-09-10 用户拍板只留一个主题，配色类连同黑暗模式一起删了）。
 * Tok 只用 Color 这个值类，不碰 Compose 运行时，单测直接跑。
 */
class TokTest {
    @Test fun 强调色是陶土橙() {
        assertEquals(Color(0xFFD97757), Tok.Accent)
    }

    @Test fun 终端是暖白底墨字() {
        // 终端底是暖白纸面，字就是界面的墨——前景/背景一暗一亮，否则终端一片糊
        assertTrue(Tok.TermBg.luminance() > 0.9f)
        assertEquals(Tok.Ink, Tok.TermFg)
        assertTrue(kotlin.math.abs(Tok.TermBg.luminance() - Tok.TermFg.luminance()) > 0.5f)
    }

    @Test fun 终端16色在亮底上都看得见() {
        // gruvbox-light：浅底上没有一个色比纸还亮（xterm 的亮黄 / 亮青落在白纸上根本看不见）
        Tok.TerminalAnsi.forEachIndexed { i, argb ->
            assertTrue("ansi[$i] 在亮底上看不见", Color(argb).luminance() < 0.6f)
        }
        // 15 bright white = 墨色，7 white 是暖灰而不是白：浅底上 8–15 比 0–7 更沉
        assertEquals(Tok.Ink, Color(Tok.TerminalAnsi[15]))
        assertTrue(Color(Tok.TerminalAnsi[7]).luminance() > Color(Tok.TerminalAnsi[15]).luminance())
    }

    @Test fun 项目行的两种状态底都是淡的且互相分得开() {
        // 整行淡底（2026-09-10）：底色要淡到标题照常读得出来，蓝黄之间、以及和面板底之间都得分得开
        assertTrue(Tok.RowRunning.luminance() > 0.7f)
        assertTrue(Tok.RowUnread.luminance() > 0.7f)
        assertNotEquals(Tok.RowRunning, Tok.RowUnread)
        assertNotEquals(Tok.RowRunning, Tok.Surface)
        assertNotEquals(Tok.RowUnread, Tok.Surface)
    }
}
