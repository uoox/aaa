package cc.uoox.aaaui

import android.view.KeyEvent
import androidx.compose.ui.graphics.toArgb
import org.connectbot.terminal.VTermKey
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TerminalInputTest {
    private val esc = "\u001b"

    @Test fun keyBarCodesMapToVtermKeys() {
        assertEquals(VTermKey.ESCAPE, vtermKeyFor(KeyEvent.KEYCODE_ESCAPE))
        assertEquals(VTermKey.ENTER, vtermKeyFor(KeyEvent.KEYCODE_ENTER))
        assertEquals(VTermKey.BACKSPACE, vtermKeyFor(KeyEvent.KEYCODE_DEL))
        assertEquals(VTermKey.UP, vtermKeyFor(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals(VTermKey.HOME, vtermKeyFor(KeyEvent.KEYCODE_MOVE_HOME))
        assertNull(vtermKeyFor(KeyEvent.KEYCODE_A)) // 字面键不走这条路
    }

    /** Claude Code 启动时的一串：备用屏 + 任意移动鼠标 + SGR 坐标 + 括号粘贴 */
    @Test fun modeTrackerFollowsClaudeCodeStartup() {
        val (m, rest) = ModeTracker.scan(TermModes(), "$esc[?1049h$esc[?1003h$esc[?1006h$esc[?2004h$esc[?25l".toByteArray())
        assertEquals(TermModes(mouse = 1003, sgrMouse = true, altScreen = true), m)
        assertEquals(0, rest.size)
        // 退出：任一鼠标模式的 l 都关掉上报；1049l 回主屏
        val (off, _) = ModeTracker.scan(m, "$esc[?1003l$esc[?1006l$esc[?1049l".toByteArray())
        assertFalse(off.mouseOn)
        assertFalse(off.altScreen)
    }

    @Test fun modeTrackerCarriesTruncatedSequenceAcrossFrames() {
        val t = ModeTracker()
        t.feed("hello $esc[?10".toByteArray())
        assertFalse(t.modes.value.mouseOn)
        t.feed("03h more".toByteArray())
        assertEquals(1003, t.modes.value.mouse)
        // 多参数一次开几个
        t.feed("$esc[?1000;1006h".toByteArray())
        assertEquals(TermModes(mouse = 1000, sgrMouse = true, altScreen = false), t.modes.value)
        t.reset()
        assertEquals(TermModes(), t.modes.value)
    }

    @Test fun modeTrackerIgnoresOtherCsiAndGarbage() {
        val (m, rest) = ModeTracker.scan(TermModes(), "$esc[31mred$esc[2J$esc[?x$esc]0;title".toByteArray())
        assertEquals(TermModes(), m)
        assertEquals(0, rest.size)
    }

    @Test fun mouseReportsEncodeSgrAndX10() {
        assertArrayEquals("$esc[<64;12;3M".toByteArray(), Mouse.report(Mouse.WHEEL_UP, 12, 3, press = true, sgr = true))
        assertArrayEquals("$esc[<0;1;1m".toByteArray(), Mouse.report(Mouse.LEFT, 1, 1, press = false, sgr = true))
        // X10：ESC [ M, 按钮+32, 列+32, 行+32；松开是按钮 3，坐标封顶 223
        assertArrayEquals(
            byteArrayOf(0x1b, '['.code.toByte(), 'M'.code.toByte(), (32 + 65).toByte(), (32 + 5).toByte(), (32 + 7).toByte()),
            Mouse.report(Mouse.WHEEL_DOWN, 5, 7, press = true, sgr = false),
        )
        assertArrayEquals(
            byteArrayOf(0x1b, '['.code.toByte(), 'M'.code.toByte(), (32 + 3).toByte(), (32 + 223).toByte(), (32 + 1).toByte()),
            Mouse.report(Mouse.LEFT, 300, 1, press = false, sgr = false),
        )
    }

    @Test fun urlAtCellUsesDisplayColumnsAndOneBasedCoordinates() {
        val screen = "第一行\n看这里 https://example.com/a?b=1 结束\n无链接"
        // 「看这里 」= 3 个全角 + 1 空格 = 7 列，URL 从第 8 列开始
        assertEquals("https://example.com/a?b=1", urlAtCell(screen, 2, 8))
        assertEquals("https://example.com/a?b=1", urlAtCell(screen, 2, 8 + "https://example.com/a?b=1".length - 1))
        assertNull(urlAtCell(screen, 2, 7))
        assertNull(urlAtCell(screen, 2, 8 + "https://example.com/a?b=1".length))
        assertNull(urlAtCell(screen, 3, 1))
        assertNull(urlAtCell(screen, 9, 1)) // 行不存在
        assertEquals(2, displayWidth('中')); assertEquals(1, displayWidth('a'))
    }

    @Test fun darkThemeKeepsXtermAnsiAndLightThemesBringTheirOwn() {
        assertArrayEquals(XTERM_ANSI, terminalAnsi(Palette.Dark))
        assertTrue(terminalAnsi(Palette.Claude).contentEquals(Palette.Claude.ansi!!.map { it.toArgb() }.toIntArray()))
    }
}
