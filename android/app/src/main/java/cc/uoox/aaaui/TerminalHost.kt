package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import com.termux.view.TerminalView
import com.termux.view.TerminalViewClient

// ============================================================
// 终端宿主：SessionScreen（agent 会话的终端视图）与 TerminalScreen（常驻终端面板）共用
// ============================================================

@Composable
internal fun TerminalHost(
    attachment: TerminalAttachment?,
    viewRef: androidx.compose.runtime.MutableState<TerminalView?>,
    fontSize: Int,
    viewClientFactory: (TerminalView) -> TerminalViewClient,
) {
    val density = LocalDensity.current
    if (attachment == null) {
        Box(Modifier.fillMaxSize().background(Tok.TermBg), contentAlignment = Alignment.Center) {
            Text("未连接 daemon", color = Tok.Faint)
        }
        return
    }
    AndroidView(
        factory = { ctx ->
            TerminalView(ctx, null).apply {
                setBackgroundColor(android.graphics.Color.parseColor("#0A0E12"))
                // 代码里 new 出来的 View 默认不可聚焦（termux 原本靠布局 XML 里的
                // focusable / focusableInTouchMode）。不设这两项 requestFocus() 直接
                // 返回 false，软键盘弹不出来，IME 输入也就永远到不了 PTY。
                isFocusable = true
                isFocusableInTouchMode = true
                setTerminalViewClient(viewClientFactory(this))
                // setTextSize must come first: it constructs the renderer (and
                // is null-safe), while setTypeface reads the existing one and
                // would NPE on a freshly built view.
                setTextSize(with(density) { fontSize.sp.toPx() }.toInt())
                // 随包的 JetBrains Mono，比系统等宽字体窄一截（见 Fonts）。后续
                // update 里的 setTextSize 会沿用现有渲染器的字体，不用再设。
                setTypeface(Fonts.terminal(ctx))
                attachSession(attachment.session)
                keepScreenOn = true
                viewRef.value = this
            }
        },
        update = { view ->
            val px = with(density) { fontSize.sp.toPx() }.toInt()
            if (view.tag != px) { view.tag = px; view.setTextSize(px) }
            if (view.currentSession !== attachment.session) view.attachSession(attachment.session)
        },
        modifier = Modifier.fillMaxSize().background(Tok.TermBg),
    )
}


@Composable
internal fun KeyChip(label: String, active: Boolean = false, onClick: () -> Unit) {
    Text(
        label,
        color = if (active) Tok.Bg else Tok.Ink,
        fontSize = 13.sp,
        fontFamily = FontFamily.Monospace,
        modifier = Modifier
            .background(if (active) Tok.Cyan else Tok.Raised, RoundedCornerShape(7.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
    )
}
