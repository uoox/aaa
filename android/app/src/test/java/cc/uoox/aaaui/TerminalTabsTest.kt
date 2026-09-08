package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Test

class TerminalTabsTest {
    @Test fun tabLabelOmitsRootAndShowsSubdirectory() {
        val root = "/Volumes/SSD/project"
        assertEquals("终端 1", terminalTabLabel(0, Session(id = "root", project_path = root), root))
        assertEquals("终端 2 · aaa-ui", terminalTabLabel(1, Session(id = "sub", project_path = "$root/aaa-ui"), root))
    }

    @Test fun terminalSessionsKeepsOnlyLiveShellsOldestFirst() {
        val sessions = listOf(
            Session(id = "new", agent = "shell", state = "running", created_at = "2026-09-02T12:00:00Z"),
            Session(id = "agent", agent = "claude", state = "running", created_at = "2026-09-02T08:00:00Z"),
            Session(id = "dead", agent = "shell", state = "exited", created_at = "2026-09-02T07:00:00Z"),
            Session(id = "old", agent = "shell", state = "waiting", created_at = "2026-09-02T09:00:00Z"),
        )
        assertEquals(listOf("old", "new"), terminalSessions(sessions).map { it.id })
    }

    /** 尾斜杠：`/a/b/` 与 `/a/b` 是同一个目录，而取叶子名会得到空串——手机上就多出一截「终端 1 · 」。 */
    @Test fun tabLabelTrimsTrailingSlashes() {
        val root = "/Volumes/SSD/project"
        assertEquals("终端 1", terminalTabLabel(0, Session(id = "a", project_path = "$root/"), root))
        assertEquals("终端 1", terminalTabLabel(0, Session(id = "b", project_path = root), "$root/"))
        assertEquals("终端 2 · aaa-ui", terminalTabLabel(1, Session(id = "c", project_path = "$root/aaa-ui/"), root))
        assertEquals("path 为空时不该拼出尾巴", "终端 1", terminalTabLabel(0, Session(id = "d", project_path = ""), root))
    }
}
