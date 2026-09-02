package cc.uoox.aaaui

import android.content.Context
import android.text.InputType
import android.view.KeyCharacterMap
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import androidx.compose.runtime.MutableState
import org.connectbot.terminal.VTermKey

// ============================================================
// 自己的键盘桥。不用 termlib 自带的 ImeInputView：它的普通模式把输入框声明成
// TYPE_NULL | PASSWORD，中文输入法在密码框里不组词；它的 compose 模式又把文字攒在
// 本地浮层里直到回车才发。这里照 termux 的做法：TYPE_NULL + fullEditor 的
// BaseInputConnection，拼音组合留在 Editable 里，选定候选（commitText）才整段发出。
// ============================================================

/** libvterm 的修饰键掩码：bit0 Shift、bit1 Alt、bit2 Ctrl。 */
internal const val VTERM_MOD_SHIFT = 1
internal const val VTERM_MOD_ALT = 2
internal const val VTERM_MOD_CTRL = 4

/**
 * 键盘字节的去处：termlib 的 emulator（libvterm 按当前 keypad/cursor 模式编码，经
 * onKeyboardInput 进 WS）。粘性 Ctrl 在这里消费：下一次按键带上 Ctrl 后自动松开。
 */
class TermInputSink(private val attachment: TerminalAttachment, private val ctrlSticky: MutableState<Boolean>) {
    private fun takeMods(extra: Int): Int {
        val sticky = ctrlSticky.value
        if (sticky) ctrlSticky.value = false
        return extra or (if (sticky) VTERM_MOD_CTRL else 0)
    }

    /** 输入法提交的一段文本。没有修饰键、没有换行时整段直写，省得一个字一帧。 */
    fun text(s: String) {
        if (s.isEmpty()) return
        val mods = takeMods(0)
        if (mods == 0 && s.none { it == '\n' || it == '\r' }) { attachment.write(s); return }
        var i = 0
        while (i < s.length) {
            when (s[i]) {
                '\n' -> { attachment.emulator.dispatchKey(mods, VTermKey.ENTER); i++ }
                '\r' -> { attachment.emulator.dispatchKey(mods, VTermKey.ENTER); i += if (i + 1 < s.length && s[i + 1] == '\n') 2 else 1 }
                else -> { val cp = s.codePointAt(i); attachment.emulator.dispatchCharacter(mods, cp); i += Character.charCount(cp) }
            }
        }
    }

    fun key(vtermKey: Int, extraMods: Int = 0) = attachment.emulator.dispatchKey(takeMods(extraMods), vtermKey)

    fun char(codepoint: Int, extraMods: Int = 0) = attachment.emulator.dispatchCharacter(takeMods(extraMods), codepoint)
}

/**
 * 隐形的 1dp View，只为承载软键盘的 InputConnection 与硬键盘按键。[sink] 是个 getter：
 * attach 换了（daemon 重连）时 View 不必重建。
 */
class TermInputView(context: Context, private val sink: () -> TermInputSink?) : View(context) {
    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    private val imm get() = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager

    fun showKeyboard() {
        if (requestFocus()) imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() {
        imm.hideSoftInputFromWindow(windowToken, 0)
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        outAttrs.inputType = InputType.TYPE_NULL
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_FLAG_NO_ENTER_ACTION or
            EditorInfo.IME_ACTION_NONE or EditorInfo.IME_FLAG_NO_FULLSCREEN
        return object : BaseInputConnection(this, true) {
            /** Editable 里已提交的文字整段发出并清空；组合中的拼音不在这里（setComposingText 走父类）。 */
            private fun flush() {
                val e = editable ?: return
                val s = e.toString()
                e.clear()
                if (s.isNotEmpty()) sink()?.text(s)
            }

            override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
                super.commitText(text, newCursorPosition)
                flush()
                return true
            }

            override fun finishComposingText(): Boolean {
                super.finishComposingText()
                flush()
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                // 正在组词：删的是 Editable 里的拼音，交给父类；否则就是真的退格
                val e = editable
                if (e != null && e.isNotEmpty()) return super.deleteSurroundingText(beforeLength, afterLength)
                val s = sink() ?: return true
                repeat(beforeLength.coerceAtLeast(1)) { s.key(VTermKey.BACKSPACE) }
                repeat(afterLength) { s.key(VTermKey.DEL) }
                return true
            }

            // 输入法把回车 / 退格当按键发过来（IME_FLAG_NO_ENTER_ACTION 下 Gboard 就是这样）
            override fun sendKeyEvent(event: KeyEvent): Boolean = dispatchKeyEvent(event)

            override fun performEditorAction(actionCode: Int): Boolean {
                sink()?.key(VTermKey.ENTER)
                return true
            }
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        handleKey(event) || super.onKeyDown(keyCode, event)

    /** 硬键盘 / 输入法按键：功能键走 libvterm 的键表，可打印字符带 Ctrl/Alt 掩码。返回 false 让系统处理（返回键、音量）。 */
    private fun handleKey(event: KeyEvent): Boolean {
        val s = sink() ?: return false
        if (event.action != KeyEvent.ACTION_DOWN) return false
        var mods = 0
        if (event.isShiftPressed) mods = mods or VTERM_MOD_SHIFT
        if (event.isAltPressed) mods = mods or VTERM_MOD_ALT
        if (event.isCtrlPressed) mods = mods or VTERM_MOD_CTRL
        vtermKeyFor(event.keyCode)?.let { s.key(it, mods); return true }
        if (event.keyCode in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12) {
            s.key(VTermKey.FUNCTION_1 + (event.keyCode - KeyEvent.KEYCODE_F1), mods)
            return true
        }
        // 字符本身已含 Shift 的效果；Ctrl/Alt 剥掉后取字符，再作为修饰键交给 libvterm 编码
        val meta = event.metaState and (KeyEvent.META_CTRL_MASK or KeyEvent.META_ALT_MASK).inv()
        val cp = event.getUnicodeChar(meta)
        if (cp == 0 || (cp and KeyCharacterMap.COMBINING_ACCENT) != 0) return false
        s.char(cp, mods and (VTERM_MOD_CTRL or VTERM_MOD_ALT))
        return true
    }
}

/**
 * 键位条的 Android 键码 → libvterm 键。不认识的返回 null，调用方就不发。
 * libvterm 会按当前 keypad / cursor 模式给出正确转义序列。
 */
internal fun vtermKeyFor(keyCode: Int): Int? = when (keyCode) {
    KeyEvent.KEYCODE_ESCAPE -> VTermKey.ESCAPE
    KeyEvent.KEYCODE_TAB -> VTermKey.TAB
    KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> VTermKey.ENTER
    KeyEvent.KEYCODE_DPAD_UP -> VTermKey.UP
    KeyEvent.KEYCODE_DPAD_DOWN -> VTermKey.DOWN
    KeyEvent.KEYCODE_DPAD_LEFT -> VTermKey.LEFT
    KeyEvent.KEYCODE_DPAD_RIGHT -> VTermKey.RIGHT
    KeyEvent.KEYCODE_MOVE_HOME -> VTermKey.HOME
    KeyEvent.KEYCODE_MOVE_END -> VTermKey.END
    KeyEvent.KEYCODE_DEL -> VTermKey.BACKSPACE
    KeyEvent.KEYCODE_FORWARD_DEL -> VTermKey.DEL
    KeyEvent.KEYCODE_INSERT -> VTermKey.INS
    KeyEvent.KEYCODE_PAGE_UP -> VTermKey.PAGEUP
    KeyEvent.KEYCODE_PAGE_DOWN -> VTermKey.PAGEDOWN
    else -> null
}
