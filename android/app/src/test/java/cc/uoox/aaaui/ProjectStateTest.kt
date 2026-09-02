package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Test

/**
 * 首页的项目 ↔ 会话拼接与三态归类。全在客户端算，daemon 只给两张表。
 */
class ProjectStateTest {
    private val p = Project(path = "/r/a", name = "a", mtime = "2026-09-01T00:00:00Z", agent = "claude", session_title = "旧对话")
    private val bare = Project(path = "/r/b", name = "b", mtime = "2026-09-01T00:00:00Z", agent = "claude", session_title = null)

    private fun s(id: String, state: String, at: String, asking: Boolean = false, path: String = p.path, preview: String = "", title: String = "") =
        Session(id = id, project_path = path, agent = "claude", state = state, asking = asking, last_output_at = at, preview = preview, title = title)

    @Test fun askingBeatsRunningRegardlessOfRecency() {
        val running = s("run", "running", "2026-09-02T10:00:00Z")
        val asking = s("ask", "waiting", "2026-09-02T09:00:00Z", asking = true, title = "选部署方式")
        assertSame(asking, primarySessionFor(p, listOf(running, asking)))
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(p, asking))
        assertEquals("选部署方式", projectSummary(p, asking, ProjectState.NEEDS_REPLY))
        // 表单刚弹出、屏幕还没静下来：asking 已 true 而 state 还是 running，一样算待回复
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(p, s("r", "running", "2026-09-02T10:00:00Z", asking = true)))
    }

    @Test fun waitingAtTheComposerIsNotAReply() {
        val idleWaiting = s("w", "waiting", "2026-09-02T09:00:00Z")
        assertEquals(ProjectState.DONE, projectStateOf(p, idleWaiting))
        assertEquals("旧对话", projectSummary(p, idleWaiting, ProjectState.DONE))
        // 没标题的待回复退到项目的对话名
        assertEquals("旧对话", projectSummary(p, s("a", "waiting", "2026-09-02T09:00:00Z", asking = true), ProjectState.NEEDS_REPLY))
    }

    @Test fun projectAgentSessionBeatsAShellInTheSameDir() {
        // 项目是 claude 的；同目录开了个终端正在敲命令，不能让它把项目行染成「执行中」
        val shell = s("sh", "running", "2026-09-02T12:00:00Z").copy(agent = "shell")
        val idle = s("i", "idle", "2026-09-02T08:00:00Z")
        assertSame(idle, primarySessionFor(p, listOf(shell, idle)))
        // claude 会话一个都没有时才退到终端
        assertSame(shell, primarySessionFor(p, listOf(shell)))
    }

    @Test fun liveSessionWinsOverANewerExitedOne() {
        val exited = s("x", "exited", "2026-09-02T12:00:00Z")
        val idle = s("i", "idle", "2026-09-02T08:00:00Z")
        assertSame(idle, primarySessionFor(p, listOf(exited, idle)))
    }

    @Test fun allExitedPicksTheMostRecentOne() {
        val older = s("o", "exited", "2026-09-01T12:00:00Z")
        val newer = s("n", "exited", "2026-09-02T12:00:00Z")
        assertSame(newer, primarySessionFor(p, listOf(older, newer)))
        assertEquals(ProjectState.DONE, projectStateOf(p, newer))
    }

    @Test fun otherProjectsSessionsAreIgnored() {
        assertNull(primarySessionFor(p, listOf(s("z", "running", "2026-09-02T12:00:00Z", path = "/r/other"))))
    }

    @Test fun noSessionButAStoredConversationIsDoneNotNever() {
        assertEquals(ProjectState.DONE, projectStateOf(p, null))
        assertEquals("旧对话", projectSummary(p, null, ProjectState.DONE))
        assertEquals(ProjectState.NEVER, projectStateOf(bare, null))
        assertEquals("未开始 · 点击启动", projectSummary(bare, null, ProjectState.NEVER))
    }

    @Test fun runningSummaryIgnoresScreenPreview() {
        // preview 是 TUI 的输入框/底栏，不能当摘要；执行中一律给标题
        val running = s("r", "running", "2026-09-02T10:00:00Z", preview = "⏵⏵ bypass permissions on (shift+tab to cycle)\n│ > │\n", title = "标题")
        assertEquals("标题", projectSummary(p, running, ProjectState.RUNNING))
        val untitled = s("r2", "running", "2026-09-02T10:00:00Z", preview = "Reading foo.kt\n")
        assertEquals(p.session_title, projectSummary(p, untitled, ProjectState.RUNNING))
    }

    @Test fun groupsFollowEnumOrderAndDropEmptyOnes() {
        val running = s("r", "running", "2026-09-02T10:00:00Z")
        val rows = projectRows(listOf(bare, p), listOf(running))
        val groups = groupProjectRows(rows)
        assertEquals(listOf(ProjectState.RUNNING, ProjectState.NEVER), groups.map { it.first })
        assertEquals("a", groups[0].second.single().project.name)
    }

    @Test fun withinAGroupNewestFirst() {
        val p2 = p.copy(path = "/r/c", name = "c", mtime = "2026-09-02T00:00:00Z")
        val rows = projectRows(listOf(p, p2), emptyList())
        assertEquals(listOf("c", "a"), groupProjectRows(rows).single().second.map { it.project.name })
    }
}
