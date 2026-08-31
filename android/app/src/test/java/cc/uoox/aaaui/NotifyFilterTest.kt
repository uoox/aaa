package cc.uoox.aaaui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NotifyFilterTest {
    @Test fun mutedProjectSuppressesAllKinds() {
        val settings = NotifySettings(mutedProjects = setOf("/p/muted"))
        for (kind in NotifyKind.entries) {
            assertFalse(NotifyFilter.shouldNotify(kind, "/p/muted", settings))
            assertTrue(NotifyFilter.shouldNotify(kind, "/p/other", settings))
        }
    }

    @Test fun perKindTogglesApply() {
        val s = NotifySettings(waitingEnabled = false, exitedEnabled = true, stalledEnabled = false)
        assertFalse(NotifyFilter.shouldNotify(NotifyKind.WAITING, "/p", s))
        assertTrue(NotifyFilter.shouldNotify(NotifyKind.EXITED, "/p", s))
        assertFalse(NotifyFilter.shouldNotify(NotifyKind.STALLED, "/p", s))
    }

    @Test fun waitingDeduperDedupesSameQuestionAndCoolsDown() {
        val d = WaitingDeduper(cooldownMs = 1000)
        val t0 = 1_000_000L
        assertTrue(d.offer("s_1", "问题A", t0))
        // 同会话同 question：永远只推一次
        assertFalse(d.offer("s_1", "问题A", t0 + 10_000))
        // 同会话不同 question 但在冷却期内：不推
        assertFalse(d.offer("s_1", "问题B", t0 + 500))
        // 冷却期过后的新 question：推
        assertTrue(d.offer("s_1", "问题B", t0 + 20_000))
        // 其它会话不受影响
        assertTrue(d.offer("s_2", "问题A", t0))
        // clear 后同 question 可再推（如会话退出重开）
        d.clear("s_1")
        assertTrue(d.offer("s_1", "问题B", t0 + 40_000))
    }
}
