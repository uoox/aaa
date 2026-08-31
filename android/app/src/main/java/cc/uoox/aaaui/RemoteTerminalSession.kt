package cc.uoox.aaaui

import com.termux.terminal.TerminalSession
import com.termux.terminal.TerminalSessionClient

/**
 * Terminal session backed by a remote PTY (aaa-daemon attach WebSocket).
 *
 * - Remote PTY output bytes are fed in through [pushBytes] → vendor emulator.
 * - User input bytes (keys, IME, paste) leave through [sendBytes] as binary WS frames.
 * - Size changes (from TerminalView layout or explicit font changes) leave through
 *   [sendControl] as the `{"t":"resize","cols":C,"rows":R}` text frame of the protocol.
 */
class RemoteTerminalSession(
    private val sendBytes: (ByteArray) -> Unit,
    private val sendControl: (String) -> Unit,
    client: TerminalSessionClient? = null,
) : TerminalSession("", "", emptyArray(), emptyArray(), TRANSCRIPT_ROWS, client) {

    private var lastSentCols = -1
    private var lastSentRows = -1

    fun pushBytes(bytes: ByteArray) = receive(bytes)

    override fun write(data: ByteArray, offset: Int, count: Int) {
        if (count <= 0) return
        sendBytes(data.copyOfRange(offset, offset + count))
    }

    /** TerminalView calls this on layout/font changes; propagate the new grid to the daemon. */
    override fun updateSize(columns: Int, rows: Int, cellWidthPixels: Int, cellHeightPixels: Int) {
        super.updateSize(columns, rows, cellWidthPixels, cellHeightPixels)
        if (columns != lastSentCols || rows != lastSentRows) {
            lastSentCols = columns
            lastSentRows = rows
            sendControl("""{"t":"resize","cols":$columns,"rows":$rows}""")
        }
    }

    /** Re-announce the current grid (used right after a WS reconnect). */
    fun resendSize() {
        if (lastSentCols > 0 && lastSentRows > 0) {
            sendControl("""{"t":"resize","cols":$lastSentCols,"rows":$lastSentRows}""")
        }
    }

    private companion object { const val TRANSCRIPT_ROWS = 2000 }
}
