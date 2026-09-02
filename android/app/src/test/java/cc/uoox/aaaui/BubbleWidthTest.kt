package cc.uoox.aaaui

import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Test

/** 消息气泡宽度按可用宽度的比例走，不再写死 300/320dp。 */
class BubbleWidthTest {
    @Test fun scalesWithAvailableWidth() {
        // Open 内屏（≈698dp 减去 rail 与边距后 ≈600dp 可用）：比例生效
        assertEquals(492.dp, bubbleMaxWidth(600.dp))
        // 直板机 393dp：82% ≈ 322dp，和旧的手感相当
        assertEquals((393 * 0.82f).dp, bubbleMaxWidth(393.dp))
    }

    @Test fun clampedAtBothEnds() {
        // 300dp 可用：82% = 246dp，被下限抬到 260dp（仍不超过可用宽度）
        assertEquals(260.dp, bubbleMaxWidth(300.dp), "窄屏下限：别挤成一列字")
        assertEquals(720.dp, bubbleMaxWidth(2000.dp), "超宽上限：一行拉满难读")
    }

    @Test fun neverWiderThanAvailable() {
        // 可用宽度比下限还窄（分屏/极窄机）：下限让位，气泡不能溢出
        assertEquals(200.dp, bubbleMaxWidth(200.dp), "气泡不得超过可用宽度")
    }

    private fun assertEquals(expected: androidx.compose.ui.unit.Dp, actual: androidx.compose.ui.unit.Dp, msg: String) {
        org.junit.Assert.assertEquals(msg, expected.value, actual.value, 0.5f)
    }
}
