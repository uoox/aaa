package cc.uoox.aaaui

import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient
import org.junit.Assert.assertEquals
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.charset.StandardCharsets

/**
 * ANSI 回路单测：远端字节喂入 [RemoteTerminalSession] → vendor 模拟器解析；
 * 用户输入/resize 从会话流出。JVM 上（无 Android Looper）分发是同步的。
 */
class RemoteTerminalSessionTest {

    private class Recorder {
        val sentBytes = mutableListOf<ByteArray>()
        val sentControls = mutableListOf<String>()
        var textChanges = 0
        var titleChanges = 0
        var finished = 0
        var copied: String? = null

        val client = object : TerminalSessionClient {
            override fun onTextChanged(changedSession: TerminalSession) { textChanges++ }
            override fun onTitleChanged(changedSession: TerminalSession) { titleChanges++ }
            override fun onSessionFinished(finishedSession: TerminalSession) { finished++ }
            override fun onCopyTextToClipboard(session: TerminalSession, text: String) { copied = text }
            override fun onPasteTextFromClipboard(session: TerminalSession?) {}
            override fun onBell(session: TerminalSession) {}
            override fun onColorsChanged(session: TerminalSession) {}
            override fun onTerminalCursorStateChange(state: Boolean) {}
            override fun setTerminalShellPid(session: TerminalSession, pid: Int) {}
            override fun getTerminalCursorStyle(): Int? = null
            override fun logError(tag: String?, message: String?) {}
            override fun logWarn(tag: String?, message: String?) {}
            override fun logInfo(tag: String?, message: String?) {}
            override fun logDebug(tag: String?, message: String?) {}
            override fun logVerbose(tag: String?, message: String?) {}
            override fun logStackTraceWithMessage(tag: String?, message: String?, e: Exception?) {}
            override fun logStackTrace(tag: String?, e: Exception?) {}
        }

        val session = RemoteTerminalSession({ sentBytes.add(it) }, { sentControls.add(it) }, client)
    }

    private fun screenLine(r: Recorder, row: Int): String {
        val emu = r.session.emulator!!
        return emu.getSelectedText(0, row, emu.mColumns - 1, row).trimEnd()
    }

    @Test fun ansiBytesRenderScreenAndMoveCursor() {
        val r = Recorder()
        r.session.updateSize(20, 5, 10, 20)
        assertNotNull(r.session.emulator)

        r.session.pushBytes("hello".toByteArray(StandardCharsets.UTF_8))
        assertEquals("hello", screenLine(r, 0))
        assertEquals(0, r.session.emulator.cursorRow)
        assertEquals(5, r.session.emulator.cursorCol)
        assertTrue(r.textChanges > 0)

        // CR LF + 颜色 + 第二行文本
        r.session.pushBytes("\r\n\u001b[31mred!\u001b[0m".toByteArray(StandardCharsets.UTF_8))
        assertEquals("red!", screenLine(r, 1))

        // 光标定位：CSI 4;3H → row 3 col 2（0-based）
        r.session.pushBytes("\u001b[4;3Hx".toByteArray(StandardCharsets.UTF_8))
        assertEquals(3, r.session.emulator.cursorRow)
        assertEquals(3, r.session.emulator.cursorCol) // x 写入后前进一列
        assertEquals("  x", screenLine(r, 3))

        // 整屏重绘（daemon replay 风格）：清屏 + 归位 + 新内容
        r.session.pushBytes("\u001b[2J\u001b[Hfresh".toByteArray(StandardCharsets.UTF_8))
        assertEquals("fresh", screenLine(r, 0))
        assertEquals("", screenLine(r, 1))
        assertEquals("", screenLine(r, 3))
    }

    @Test fun oscTitleReachesClient() {
        val r = Recorder()
        r.session.updateSize(20, 5, 10, 20)
        r.session.pushBytes("\u001b]0;my-title\u0007".toByteArray(StandardCharsets.UTF_8))
        assertEquals("my-title", r.session.title)
        assertTrue(r.titleChanges > 0)
    }

    @Test fun writeRespectsOffsetAndCount() {
        val r = Recorder()
        val data = "XXabcYY".toByteArray(StandardCharsets.UTF_8)
        r.session.write(data, 2, 3)
        assertArrayEquals("abc".toByteArray(), r.sentBytes.single())

        r.session.write(data, 0, 0) // 空写不出帧
        assertEquals(1, r.sentBytes.size)

        r.session.write("直发")
        assertArrayEquals("直发".toByteArray(StandardCharsets.UTF_8), r.sentBytes[1])
    }

    @Test fun writeCodePointEncodesUtf8WithOptionalEscape() {
        val r = Recorder()
        r.session.writeCodePoint(false, 'a'.code)
        assertArrayEquals(byteArrayOf(0x61), r.sentBytes[0])
        r.session.writeCodePoint(true, 'b'.code) // Alt+b → ESC b
        assertArrayEquals(byteArrayOf(27, 0x62), r.sentBytes[1])
        r.session.writeCodePoint(false, 0x4E2D) // 中 → 3 字节 UTF-8
        assertArrayEquals("中".toByteArray(StandardCharsets.UTF_8), r.sentBytes[2])
    }

    @Test fun resizeEmitsControlFrameOncePerGridChange() {
        val r = Recorder()
        r.session.updateSize(80, 24, 10, 20)
        r.session.updateSize(80, 24, 10, 20) // 同尺寸不重发
        r.session.updateSize(100, 40, 10, 20)
        assertEquals(
            listOf(
                """{"t":"resize","cols":80,"rows":24}""",
                """{"t":"resize","cols":100,"rows":40}""",
            ),
            r.sentControls,
        )
        r.session.resendSize() // 重连后重放当前尺寸
        assertEquals("""{"t":"resize","cols":100,"rows":40}""", r.sentControls.last())
        assertEquals(3, r.sentControls.size)
    }

    @Test fun finishReportsExitStatusOnce() {
        val r = Recorder()
        r.session.updateSize(20, 5, 10, 20)
        assertTrue(r.session.isRunning)
        r.session.finish(3)
        assertTrue(!r.session.isRunning)
        assertEquals(3, r.session.exitStatus)
        r.session.finish(4) // 幂等：不再回调
        assertEquals(1, r.finished)
    }

    @Test fun receiveBeforeUpdateSizeBootstrapsDefaultGrid() {
        val r = Recorder()
        r.session.pushBytes("early".toByteArray(StandardCharsets.UTF_8))
        assertNotNull(r.session.emulator)
        assertEquals("early", screenLine(r, 0))
    }
}
