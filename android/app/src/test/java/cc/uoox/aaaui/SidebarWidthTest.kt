package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * ☰ 左侧栏宽度（2026-09-08 用户拍板：「小屏情况下，直接铺满，大屏情况下，半屏」）。
 * 分支只有一条线，但两头的真实机型都要落在自己那侧——这里钉的就是机型，不是那个 600。
 */
class SidebarWidthTest {
    @Test
    fun 小屏铺满() {
        assertEquals(1f, sidebarFraction(343), 0f)  // 折叠机外屏 / 普通手机竖屏 1116px@520dpi
        assertEquals(1f, sidebarFraction(411), 0f)  // Pixel 竖屏
        assertEquals(1f, sidebarFraction(599), 0f)  // 门槛下沿
    }

    @Test
    fun 大屏半屏() {
        assertEquals(0.5f, sidebarFraction(600), 0f)   // 门槛本身算大屏
        assertEquals(0.5f, sidebarFraction(674), 0f)   // 折叠机内屏——用户抱怨「宽度太大了」的就是它
        assertEquals(0.5f, sidebarFraction(1280), 0f)  // 平板横屏
    }
}
