package cc.uoox.aaaui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

// ============================================================
// 共享构件
//
// 这个文件收的只有一类东西：**同一件东西在好几屏里各手画了一遍**。判定标准是视觉参数
// 逐个相同——颜色、圆角、间距、字号但凡有一处不一样，要么把它做成参数（且参数少到能一眼
// 看完），要么就干脆留在原地。宁可这里多几个小构件，也不要一个带一堆开关的万能函数：
// 开关一多，读的人还得回头去数每个调用点传了什么，反而比各画各的更难改。
//
// 已经有名字的东西不在这里重来一遍：配色走 `Tok.`（Ui.kt），`StateDot` / `DotWithText` /
// `KeyChip` / `MarkBar` 也各自留在原处，它们本来就是构件。
// ============================================================

// ---------- 圆角面板：两种「底 + 圆角」的固定搭配 ----------

/**
 * 「凹下去」的小面板：代码块、思考行、工具输出、表单里的自填框都是它——[Tok.Inset] 打底
 * 加一个圆角，圆角大小随块头走（大块 10、常规 8、行内小框 6），所以 [radius] 是参数而底色
 * 不是：换成别的底色它就不是这个东西了。padding 留给调用点——同一种底，代码块要 10/8、
 * 工具输出要 8、自填框要 10/7，各有各的呼吸。
 */
fun Modifier.insetPanel(radius: Dp = 8.dp): Modifier =
    background(Tok.Inset, RoundedCornerShape(radius))

/**
 * 「浮起来」的卡片：[Tok.Surface] 打底 + 一像素 [Tok.Edge] 描边，两者同一个圆角。设置页那
 * 张大卡（12dp）和看板的会话卡（10dp）只差圆角，所以只有 [radius] 一个参数。
 *
 * 顺序是先 background 后 border：border 的一像素描在底色之上，反过来底色会盖住描边内侧半格。
 */
fun Modifier.surfaceCard(radius: Dp): Modifier =
    background(Tok.Surface, RoundedCornerShape(radius))
        .border(1.dp, Tok.Edge, RoundedCornerShape(radius))

// ---------- 小构件 ----------

/**
 * 4dp 高的细进度条：[Tok.Edge] 的槽 + 按 [fraction]（0..1）填的条，两截同样 2dp 圆角。
 * 详情屏的上下文占比和看板卡片的清单进度是同一根条，只有填充色不同（占比按级别着色、
 * 清单做完了转绿），所以 [color] 是参数。
 *
 * 宽度由 [modifier] 从外面给（两处都是 Row 里的 `weight(1f)`）——条本身不知道自己该多宽。
 */
@Composable
fun ThinProgressBar(fraction: Float, color: Color, modifier: Modifier = Modifier) {
    Box(modifier.height(4.dp).background(Tok.Edge, RoundedCornerShape(2.dp))) {
        Box(
            Modifier.fillMaxWidth(fraction).height(4.dp)
                .background(color, RoundedCornerShape(2.dp)),
        )
    }
}

/**
 * 无边框的圆角输入框：[Tok.Raised] 一块圆角底，空的时候把 [placeholder] 垫在下面。会话页的
 * composer 和看板的搜索框是同一个东西，差别只在行数（一个最多 4 行，一个单行），所以
 * [singleLine] / [maxLines] 原样透出去——它俩在 `BasicTextField` 里语义不同（singleLine 还
 * 会把回车换成输入法的动作键），不能合并成一个数字。
 */
@Composable
fun RoundedTextField(
    value: String,
    onValueChange: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    singleLine: Boolean = false,
    // 默认值抄的是 BasicTextField 自己的：单行时最多一行。写死 Int.MAX_VALUE 会让单行框少掉
    // 那条「按行数定高」的约束，空框的高度就跟原来差一点点
    maxLines: Int = if (singleLine) 1 else Int.MAX_VALUE,
) {
    Box(
        modifier.background(Tok.Raised, RoundedCornerShape(10.dp)).padding(horizontal = 12.dp, vertical = 8.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
        if (value.isEmpty()) Text(placeholder, color = Tok.Faint, fontSize = 14.sp)
        BasicTextField(
            value, onValueChange,
            modifier = Modifier.fillMaxWidth(),
            singleLine = singleLine,
            maxLines = maxLines,
            textStyle = TextStyle(color = Tok.Ink, fontSize = 14.sp),
            cursorBrush = SolidColor(Tok.Accent),
        )
    }
}

/**
 * 淡底 + 同色描边的小按钮（权限卡的「允许」/「拒绝」）：底 0.2、边 0.6，同一个 [tint] 出两档
 * 透明度，所以只要一个颜色参数。文字一律 [Tok.Ink]——两颗按钮是一对，字色得一样重。
 */
@Composable
fun TintPillButton(label: String, tint: Color, enabled: Boolean = true, onClick: () -> Unit) {
    Text(
        label, color = Tok.Ink, fontSize = 13.sp,
        modifier = Modifier
            .background(tint.copy(alpha = 0.2f), RoundedCornerShape(6.dp))
            .border(1.dp, tint.copy(alpha = 0.6f), RoundedCornerShape(6.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 6.dp),
    )
}

/**
 * 顶栏左上角那个返回箭头。三屏（设置 / 看板 / 详情）都是这一个字加同样的 8dp 触摸区，只有
 * 字号差一档（详情屏 28sp，另两屏 26sp），所以 [fontSize] 是参数。外面那行 Row 各屏的
 * padding 并不相同，就不一起收了——收进来反而要多一个参数去还原各自的行高。
 */
@Composable
fun BackArrow(onClick: () -> Unit, fontSize: TextUnit = 26.sp) {
    Text(
        "‹", color = Tok.Dim, fontSize = fontSize,
        modifier = Modifier.clickable(onClick = onClick).padding(horizontal = 8.dp),
    )
}

// ---------- 对话框 ----------

/**
 * 全 app 的对话框骨架：[Tok.Raised] 的底、[Tok.Ink] 的标题、右下角一个确认按钮、可选一个
 * 取消按钮。五处对话框（重启 daemon / 重命名 / 回放链接 / 套餐用量 / 删除报告）此前逐行
 * 写了五遍同样的三行样式，正文各不相同——所以正文是 [body] 插槽，其余都定死。
 *
 * [confirmColor] 默认 `Color.Unspecified`：那正是 `Text` 不写 color 时的取值，于是「按钮用
 * M3 主色」和「按钮特意染红/染灰」两种写法都能原样还原，不必为此分出两个构件。
 * [cancelLabel] 为 null 就没有第二颗按钮——只是「看完了关掉」的对话框不该有取消。
 */
@Composable
fun AaaDialog(
    title: String,
    onDismiss: () -> Unit,
    confirmLabel: String,
    onConfirm: () -> Unit = onDismiss,
    confirmColor: Color = Color.Unspecified,
    cancelLabel: String? = null,
    onCancel: () -> Unit = onDismiss,
    body: @Composable () -> Unit,
) {
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Tok.Raised,
        title = { Text(title, color = Tok.Ink) },
        text = body,
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirmLabel, color = confirmColor) } },
        dismissButton = if (cancelLabel == null) null else {
            { TextButton(onClick = onCancel) { Text(cancelLabel, color = Tok.Dim) } }
        },
    )
}

/**
 * 「做不做这件事」的确认框：正文一段淡字，确认按钮红（这类框问的都是会毁掉东西的事），
 * 取消固定叫「取消」。结束会话、删除项目、重启 daemon 都走它。
 */
@Composable
fun ConfirmDialog(title: String, body: String, confirmLabel: String, onConfirm: () -> Unit, onCancel: () -> Unit) {
    AaaDialog(
        title = title,
        onDismiss = onCancel,
        confirmLabel = confirmLabel,
        onConfirm = onConfirm,
        confirmColor = Tok.Red,
        cancelLabel = "取消",
        onCancel = onCancel,
    ) { Text(body, color = Tok.Dim) }
}
