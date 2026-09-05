package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 首页的项目 ↔ 会话拼接、三个状态字与排序。全在客户端算，daemon 只给两张表。
 */
class ProjectStateTest {
    private val p = Project(path = "/r/a", name = "a", mtime = "2026-09-01T00:00:00Z", agent = "claude", session_title = "旧对话")
    private val bare = Project(path = "/r/b", name = "b", mtime = "2026-09-01T00:00:00Z", agent = "claude", session_title = null)

    private fun s(id: String, state: String, at: String, asking: Boolean = false, path: String = p.path, preview: String = "", title: String = "", updated: String = "") =
        Session(id = id, project_path = path, agent = "claude", state = state, asking = asking, last_output_at = at, preview = preview, title = title, updated_at = updated)

    @Test fun askingBeatsRunningRegardlessOfRecency() {
        val running = s("run", "running", "2026-09-02T10:00:00Z")
        val asking = s("ask", "waiting", "2026-09-02T09:00:00Z", asking = true, title = "选部署方式")
        assertSame(asking, primarySessionFor(p, listOf(running, asking)))
        // 在问 = 轮到你 = 已激活；表单刚弹出、屏幕还没静下来（state 还是 running）也一样
        assertEquals(ProjectState.ACTIVE, projectStateOf(asking))
        assertEquals(ProjectState.ACTIVE, projectStateOf(s("r", "running", "2026-09-02T10:00:00Z", asking = true)))
    }

    @Test fun threeWords() {
        assertEquals(ProjectState.RUNNING, projectStateOf(s("r", "running", "2026-09-02T10:00:00Z")))
        assertEquals(ProjectState.ACTIVE, projectStateOf(s("w", "waiting", "2026-09-02T09:00:00Z")))
        assertEquals(ProjectState.INACTIVE, projectStateOf(s("x", "exited", "2026-09-02T09:00:00Z")))
        assertEquals(ProjectState.INACTIVE, projectStateOf(null))
        assertEquals("执行中", ProjectState.RUNNING.label)
        assertEquals("已激活", ProjectState.ACTIVE.label)
        assertEquals("未激活", ProjectState.INACTIVE.label)
    }

    @Test fun projectAgentSessionBeatsAShellInTheSameDir() {
        // 项目是 claude 的；同目录开了个终端正在敲命令，不能让它把项目行染成「执行中」
        val shell = s("sh", "running", "2026-09-02T12:00:00Z").copy(agent = "shell")
        val idle = s("i", "idle", "2026-09-02T08:00:00Z")
        assertSame(idle, primarySessionFor(p, listOf(shell, idle)))
        assertNull(primarySessionFor(p, listOf(shell)))
        // 终端也不参与排序时间：只有终端的项目按目录 mtime
        val row = projectRows(listOf(p), listOf(shell)).single()
        assertEquals(p.mtime, row.updatedIso)
        assertEquals(ProjectState.INACTIVE, row.state)
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
        assertEquals(ProjectState.INACTIVE, projectStateOf(newer))
    }

    @Test fun otherProjectsSessionsAreIgnored() {
        assertNull(primarySessionFor(p, listOf(s("z", "running", "2026-09-02T12:00:00Z", path = "/r/other"))))
    }

    @Test fun orderFollowsLatestUpdatedSessionNotOutputOrState() {
        val p2 = p.copy(path = "/r/c", name = "c")
        val p3 = p.copy(path = "/r/d", name = "d")
        // 最近输出、状态故意和 updated_at 反着来：只看 updated_at
        val a1 = s("a1", "running", "2026-09-02T13:00:00Z", updated = "2026-09-02T09:00:00Z")
        val c1 = s("c1", "waiting", "2026-09-02T08:00:00Z", path = p2.path, updated = "2026-09-02T11:00:00Z")
        val d1 = s("d1", "waiting", "2026-09-02T12:00:00Z", path = p3.path, asking = true, updated = "2026-09-02T10:00:00Z")
        val rows = projectRows(listOf(p, p2, p3), listOf(a1, c1, d1))
        assertEquals(listOf("c", "d", "a"), rows.map { it.project.name })
        assertEquals(listOf(ProjectState.ACTIVE, ProjectState.ACTIVE, ProjectState.RUNNING), rows.map { it.state })
        // 输出再多、状态再翻，updated_at 不变顺序就不变
        val churn = listOf(a1.copy(last_output_at = "2026-09-09T00:00:00Z", state = "waiting"), c1.copy(state = "running"), d1)
        assertEquals(listOf("c", "d", "a"), projectRows(listOf(p, p2, p3), churn).map { it.project.name })
        // 退出的会话也算「更新」：刚关掉会话的项目排第一，但行是未激活
        val x = s("x", "exited", "2026-09-02T00:00:00Z", updated = "2026-09-03T00:00:00Z")
        val rows2 = projectRows(listOf(p, p2), listOf(x, c1))
        assertEquals(listOf("a", "c"), rows2.map { it.project.name })
        assertEquals(ProjectState.INACTIVE, rows2[0].state)
    }

    @Test fun oldDaemonWithoutUpdatedAtFallsBackToCreatedAt() {
        val p2 = p.copy(path = "/r/c", name = "c", mtime = "2026-09-05T00:00:00Z")
        val early = s("a1", "waiting", "2026-09-02T00:00:00Z").copy(created_at = "2026-09-02T00:00:00Z")
        // c 没有会话：按目录 mtime（09-05）排在 a（09-02）前面
        assertEquals(listOf("c", "a"), projectRows(listOf(p, p2), listOf(early)).map { it.project.name })
        // 都没有时间就按路径稳住
        val na = p.copy(mtime = ""); val nb = bare.copy(mtime = "")
        assertEquals(listOf("/r/a", "/r/b"), projectRows(listOf(nb, na), emptyList()).map { it.project.path })
    }

    @Test fun rowTitleIsTheSessionNameNotTheFolder() {
        // 与 mac 侧栏同口径：活着的会话用它的 title，退出后用 daemon 读出的 session_title，
        // 都没有才是文件夹名
        val live = s("l", "running", "2026-09-02T10:00:00Z", title = "改登录页")
        assertEquals("改登录页", projectRows(listOf(p), listOf(live)).single().title)
        // 退出的会话哪怕有 title 也不代表项目：用 session_title
        val gone = s("g", "exited", "2026-09-02T10:00:00Z", title = "已经关了的")
        assertEquals("旧对话", projectRows(listOf(p), listOf(gone)).single().title)
        assertEquals("旧对话", projectRows(listOf(p), emptyList()).single().title)
        assertEquals("b", projectRows(listOf(bare), emptyList()).single().title)
        assertFalse(projectRows(listOf(bare), emptyList()).single().alive)
        assertTrue(projectRows(listOf(p), listOf(live)).single().alive)
    }

    @Test fun composerQueuesWhileAgentIsBusyOrAsking() {
        // 在跑：排成待发送，跑完自动发出（和终端里先敲好等它一样）
        assertTrue(queueInsteadOfSend("running", asking = false))
        // 弹着问题：自由文本会替你按下高亮项，也排队
        assertTrue(queueInsteadOfSend("waiting", asking = true))
        assertTrue(queueInsteadOfSend("running", asking = true))
        // 空着：直接写进去
        assertFalse(queueInsteadOfSend("waiting", asking = false))
        // 还没拿到会话 / 已退出：直接发（退出的发送按钮本来就是禁用的）
        assertFalse(queueInsteadOfSend(null, asking = false))
        assertFalse(queueInsteadOfSend("exited", asking = false))
    }
}
