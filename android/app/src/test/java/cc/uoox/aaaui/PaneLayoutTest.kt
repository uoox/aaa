package cc.uoox.aaaui

import androidx.compose.material3.windowsizeclass.ExperimentalMaterial3WindowSizeClassApi
import androidx.compose.material3.windowsizeclass.WindowSizeClass
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * 折叠屏断点与右栏选中项的取舍。
 *
 * 宽度是从目标机器上量出来的，不是估的——这里估错过一次：原先按「内屏 ≈1104dp」
 * 把门槛放在 Expanded(840dp)，而 OnePlus Open 内屏 `dumpsys` 实报 **sw698dp**
 * （2268px ÷ density 3.25），于是双栏在它唯一要服务的设备上永远不触发。
 * 改动这些数字前先在真机上 `adb shell dumpsys window | grep sw` 对一次。
 */
@OptIn(ExperimentalMaterial3WindowSizeClassApi::class)
class PaneLayoutTest {

    private fun paneFor(widthDp: Int, heightDp: Int): PaneLayout =
        paneLayoutFor(WindowSizeClass.calculateFromSize(DpSize(widthDp.dp, heightDp.dp)).widthSizeClass)

    @Test fun unfoldedOpenGoesDualPane() {
        // 实测：2268×2440 px @520dpi → sw698dp w698dp h751dp
        assertEquals(PaneLayout.Dual, paneFor(698, 751))
        assertEquals(PaneLayout.Dual, paneFor(751, 698))  // 另一个方向
    }

    @Test fun phoneAndCoverDisplayStaySinglePane() {
        assertEquals(PaneLayout.Single, paneFor(343, 764))  // Open 外屏，实测 1116px ÷ 3.25
        assertEquals(PaneLayout.Single, paneFor(393, 873))  // 9R 直板竖屏
    }

    @Test fun breakpointSitsAtMedium() {
        // material3 的 Medium 从 600dp 起；598 还是手机宽度，600 已经放得下两栏
        assertEquals(PaneLayout.Single, paneFor(599, 900))
        assertEquals(PaneLayout.Dual, paneFor(600, 900))
    }

    @Test fun theTerminalKeepsTheLargerPane() {
        // 双栏的前提：终端那栏永远比列表宽。最窄的双栏场景是 600dp。
        val terminal = 600 - ListPaneWidth.value.toInt()
        assert(terminal > ListPaneWidth.value.toInt()) { "终端栏 ${terminal}dp 不该窄于列表栏" }
    }

    @Test fun openTargetFollowsPaneLayout() {
        assertEquals(OpenTarget.Select("s1", "hi"), openTargetFor(PaneLayout.Dual, "s1", "hi"))
        assertEquals(OpenTarget.Push("s1", "hi"), openTargetFor(PaneLayout.Single, "s1", "hi"))
    }

    @Test fun selectionSurvivesListReshuffleButNotDeletion() {
        val a = session("a")
        val b = session("b")
        // 单栏↔两栏来回切会重排列表，选中项不能因为顺序变了就丢
        assertEquals("a", retainSelection("a", listOf(b, a)))
        // 会话被删掉后右栏必须回空态，否则挂着一个已经不存在的会话
        assertNull(retainSelection("a", listOf(b)))
        assertNull(retainSelection(null, listOf(a, b)))
    }

    private fun session(id: String) = Session(id = id, project_path = "/p/$id", project_name = id, agent = "claude", state = "idle")
}
