package cc.uoox.aaaui

import androidx.compose.material3.windowsizeclass.ExperimentalMaterial3WindowSizeClassApi
import androidx.compose.material3.windowsizeclass.WindowSizeClass
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * 宽屏只换导航位置，内容始终单栏。
 *
 * 宽度是从目标机器上量出来的，不是估的——这里估错过一次：原先按「内屏 ≈1104dp」
 * 把断点放在 Expanded(840dp)，而 OnePlus Open 内屏 `dumpsys` 实报 **sw698dp**
 * （2268px ÷ density 3.25），于是宽屏布局在它唯一要服务的设备上永远不触发。
 * 改动这些数字前先在真机上 `adb shell dumpsys window | grep sw` 对一次。
 */
@OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
class PaneLayoutTest {

    private fun placementFor(widthDp: Int, heightDp: Int): NavPlacement =
        navPlacementFor(WindowSizeClass.calculateFromSize(DpSize(widthDp.dp, heightDp.dp)).widthSizeClass)

    @Test fun unfoldedOpenUsesTheLeftRail() {
        // 实测：2268×2440 px @520dpi → sw698dp w698dp h751dp
        assertEquals(NavPlacement.Rail, placementFor(698, 751))
        assertEquals(NavPlacement.Rail, placementFor(751, 698))  // 另一个方向
    }

    @Test fun phoneAndCoverDisplayKeepTheBottomBar() {
        assertEquals(NavPlacement.Bottom, placementFor(343, 764))  // Open 外屏，实测 1116px ÷ 3.25
        assertEquals(NavPlacement.Bottom, placementFor(393, 873))  // 9R 直板竖屏
    }

    @Test fun breakpointSitsAtMedium() {
        // material3 的 Medium 从 600dp 起；599 还是手机宽度
        assertEquals(NavPlacement.Bottom, placementFor(599, 900))
        assertEquals(NavPlacement.Rail, placementFor(600, 900))
    }
}
