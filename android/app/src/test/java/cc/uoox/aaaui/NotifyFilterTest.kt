package cc.uoox.aaaui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** 通知只剩一种「完成」：按项目静音 + 总开关，没有分级、没有去重。 */
class NotifyFilterTest {
    @Test fun mutedProjectSuppresses() {
        val settings = NotifySettings(mutedProjects = setOf("/p/muted"))
        assertFalse(NotifyFilter.shouldNotify("/p/muted", settings))
        assertTrue(NotifyFilter.shouldNotify("/p/other", settings))
    }

    @Test fun doneToggleApplies() {
        assertFalse(NotifyFilter.shouldNotify("/p", NotifySettings(doneEnabled = false)))
        assertTrue(NotifyFilter.shouldNotify("/p", NotifySettings(doneEnabled = true)))
    }
}
