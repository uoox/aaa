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
        assertEquals("等你回答", projectSummary(p, asking, ProjectState.NEEDS_REPLY))
        // 表单刚弹出、屏幕还没静下来：asking 已 true 而 state 还是 running，一样算待回复
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(p, s("r", "running", "2026-09-02T10:00:00Z", asking = true)))
    }

    @Test fun waitingAtTheComposerIsNotAReply() {
        val idleWaiting = s("w", "waiting", "2026-09-02T09:00:00Z")
        assertEquals(ProjectState.DONE, projectStateOf(p, idleWaiting))
        assertEquals("", projectSummary(p, idleWaiting, ProjectState.DONE))
        // 没标题的待回复退到项目的对话名
        assertEquals("等你回答", projectSummary(p, s("a", "waiting", "2026-09-02T09:00:00Z", asking = true), ProjectState.NEEDS_REPLY))
    }

    @Test fun projectAgentSessionBeatsAShellInTheSameDir() {
        // 项目是 claude 的；同目录开了个终端正在敲命令，不能让它把项目行染成「执行中」
        val shell = s("sh", "running", "2026-09-02T12:00:00Z").copy(agent = "shell")
        val idle = s("i", "idle", "2026-09-02T08:00:00Z")
        assertSame(idle, primarySessionFor(p, listOf(shell, idle)))
        assertNull(primarySessionFor(p, listOf(shell)))
    }

    @Test fun shellNeverBecomesPrimaryEvenWhenNewestAndRealSessionExited() {
        val shell = s("sh", "running", "2026-09-02T12:00:00Z").copy(agent = "shell")
        val exited = s("x", "exited", "2026-09-02T08:00:00Z")
        assertSame(exited, primarySessionFor(p, listOf(exited, shell)))
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
        assertEquals("", projectSummary(p, null, ProjectState.DONE))
        assertEquals(ProjectState.NEVER, projectStateOf(bare, null))
        assertEquals("未开始", projectSummary(bare, null, ProjectState.NEVER))
    }

    @Test fun runningSummaryIgnoresScreenPreview() {
        // preview 是 TUI 的输入框/底栏，不能当摘要；执行中一律给标题
        val running = s("r", "running", "2026-09-02T10:00:00Z", preview = "⏵⏵ bypass permissions on (shift+tab to cycle)\n│ > │\n", title = "标题")
        assertEquals("执行中", projectSummary(p, running, ProjectState.RUNNING))
        val untitled = s("r2", "running", "2026-09-02T10:00:00Z", preview = "Reading foo.kt\n")
        assertEquals("执行中", projectSummary(p, untitled, ProjectState.RUNNING))
    }

    @Test fun twoGroupsAliveVersusRest() {
        val running = s("r", "running", "2026-09-02T10:00:00Z")
        val rows = projectRows(listOf(bare, p), listOf(running))
        val groups = groupProjectRows(rows)
        assertEquals(listOf(ProjectGroup.ACTIVE, ProjectGroup.INACTIVE), groups.map { it.first })
        assertEquals("a", groups[0].second.single().project.name)
        assertEquals("b", groups[1].second.single().project.name)
        // 会话退出了就不再「激活」：项目回到未激活栏（旧对话还在，点一行 resume）
        val exited = s("x", "exited", "2026-09-02T11:00:00Z")
        assertEquals(listOf(ProjectGroup.INACTIVE), groupProjectRows(projectRows(listOf(p), listOf(exited))).map { it.first })
        // waiting / asking 都还活着，都在激活栏
        val asking = s("q", "waiting", "2026-09-02T11:00:00Z", asking = true)
        assertEquals(ProjectGroup.ACTIVE, projectRows(listOf(p), listOf(asking)).single().group)
    }

    @Test fun activeOrderFollowsCreatedAtNotState() {
        val p2 = p.copy(path = "/r/c", name = "c")
        val p3 = p.copy(path = "/r/d", name = "d")
        // c 先开、a 后开、d 最后开；状态和最近输出故意反着来
        val late = s("a1", "running", "2026-09-02T12:00:00Z").copy(created_at = "2026-09-02T09:00:00Z")
        val early = s("c1", "waiting", "2026-09-02T08:00:00Z", path = p2.path).copy(created_at = "2026-09-02T07:00:00Z")
        val last = s("d1", "waiting", "2026-09-02T13:00:00Z", path = p3.path, asking = true).copy(created_at = "2026-09-02T10:00:00Z")
        val rows = projectRows(listOf(p, p2, p3), listOf(late, early, last))
        assertEquals(listOf("c", "a", "d"), groupProjectRows(rows).single().second.map { it.project.name })
        // 状态翻转顺序不变
        val flipped = listOf(late.copy(state = "waiting", asking = true), early.copy(state = "running"), last.copy(state = "running", asking = false))
        assertEquals(listOf("c", "a", "d"), groupProjectRows(projectRows(listOf(p, p2, p3), flipped)).single().second.map { it.project.name })
    }

    @Test fun inactiveOrderIsMostRecentFirst() {
        // 从没跑过的按目录 mtime：c 更新，排前面
        val pa = p.copy(mtime = "2026-09-01T00:00:00Z")
        val p2 = p.copy(path = "/r/c", name = "c", mtime = "2026-09-02T00:00:00Z")
        assertEquals(listOf("c", "a"), groupProjectRows(projectRows(listOf(pa, p2), emptyList())).single().second.map { it.project.name })
        // 刚关掉会话的项目排第一：a 的会话 09-03 退出，比 c 的 mtime 新
        val exited = s("x", "exited", "2026-09-03T08:00:00Z")
        assertEquals(listOf("a", "c"), groupProjectRows(projectRows(listOf(p2, pa), listOf(exited))).single().second.map { it.project.name })
    }

    @Test fun rowTitleIsTheSessionNameNotTheFolder() {
        // 与 mac 侧栏同口径：活着的会话用它的 title，退出后用 daemon 读出的 session_title，
        // 都没有才是文件夹名；第二行只说状态，文件夹名不再出现
        val live = s("l", "running", "2026-09-02T10:00:00Z", title = "改登录页")
        val liveRow = projectRows(listOf(p), listOf(live)).single()
        assertEquals("改登录页", liveRow.title)
        assertEquals("执行中", liveRow.subtitle)
        val idleRow = projectRows(listOf(p), emptyList()).single()
        assertEquals("旧对话", idleRow.title)
        assertEquals("", idleRow.subtitle)
        val neverRow = projectRows(listOf(bare), emptyList()).single()
        assertEquals("b", neverRow.title)
        assertEquals("未开始", neverRow.subtitle)
    }
}
