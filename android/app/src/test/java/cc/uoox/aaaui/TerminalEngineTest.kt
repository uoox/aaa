package cc.uoox.aaaui

import android.view.KeyEvent
import org.connectbot.terminal.VTermKey
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class TerminalEngineTest {
    @Test fun unknownEngineNameFallsBackToTermux() {
        assertEquals(TerminalEngine.Termux, TerminalEngine.forName(null))
        assertEquals(TerminalEngine.Termux, TerminalEngine.forName("garbage"))
        assertEquals(TerminalEngine.Termlib, TerminalEngine.forName("termlib"))
    }

    @Test fun viewToggleCyclesThroughAllThreeAndSkipsMessagesWhenUnsupported() {
        assertEquals("termux", nextUiMode("messages", true))
        assertEquals("termlib", nextUiMode("termux", true))
        assertEquals("messages", nextUiMode("termlib", true))
        // v1 daemon 没有消息流：两套终端来回
        assertEquals("termux", nextUiMode("termlib", false))
        assertEquals("termlib", nextUiMode("termux", false))
    }

    @Test fun keyBarCodesMapToVtermKeys() {
        assertEquals(VTermKey.ESCAPE, vtermKeyFor(KeyEvent.KEYCODE_ESCAPE))
        assertEquals(VTermKey.ENTER, vtermKeyFor(KeyEvent.KEYCODE_ENTER))
        assertEquals(VTermKey.UP, vtermKeyFor(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals(VTermKey.HOME, vtermKeyFor(KeyEvent.KEYCODE_MOVE_HOME))
        assertNull(vtermKeyFor(KeyEvent.KEYCODE_A)) // 字面键不走这条路
    }
}
