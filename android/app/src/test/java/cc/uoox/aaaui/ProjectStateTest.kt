package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 首页的项目 ↔ 会话拼接、行首记号（转圈 / 黄点）与排序。全在客户端算，daemon 只给两张表。
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
        // 在问 = 待回复；表单刚弹出、屏幕还没静下来（state 还是 running）也一样
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(asking))
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(s("r", "running", "2026-09-02T10:00:00Z", asking = true)))
    }

    @Test fun stateAndSpinning() {
        assertEquals(ProjectState.RUNNING, projectStateOf(s("r", "running", "2026-09-02T10:00:00Z")))
        assertEquals(ProjectState.ACTIVE, projectStateOf(s("w", "waiting", "2026-09-02T09:00:00Z")))
        assertEquals(ProjectState.INACTIVE, projectStateOf(s("x", "exited", "2026-09-02T09:00:00Z")))
        assertEquals(ProjectState.INACTIVE, projectStateOf(null))
        // 2026-09-08：列表上不再写状态字，枚举只管「转不转圈」和排序
        assertTrue(ProjectState.RUNNING.spinning)
        assertTrue(ProjectState.BACKGROUND.spinning)
        assertFalse(ProjectState.NEEDS_REPLY.spinning)
        assertFalse(ProjectState.ACTIVE.spinning)
        assertFalse(ProjectState.INACTIVE.spinning)
        // 后台：waiting 且 background；在问的仍是待回复
        assertEquals(ProjectState.BACKGROUND, projectStateOf(s("w", "waiting", "2026-09-02T09:00:00Z").copy(background = true)))
        assertEquals(ProjectState.NEEDS_REPLY, projectStateOf(s("w", "waiting", "2026-09-02T09:00:00Z", asking = true).copy(background = true)))
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

    @Test fun statusDecidesTheOrderBeforeTime() {
        // 2026-09-07 用户拍板的从上往下：待回复 > 运行 > 后台 > 激活 > 暂停
        val pAsk = p.copy(path = "/r/ask", name = "ask", mtime = "2026-09-01T00:00:00Z")
        val pRun = p.copy(path = "/r/run", name = "run", mtime = "2026-09-02T00:00:00Z")
        val pBg = p.copy(path = "/r/bg", name = "bg", mtime = "2026-09-02T12:00:00Z")
        val pAct = p.copy(path = "/r/act", name = "act", mtime = "2026-09-03T00:00:00Z")
        val pIdle = p.copy(path = "/r/idle", name = "idle", mtime = "2026-09-04T00:00:00Z")
        // 时间故意与状态反着来：只按时间排的话顺序正好倒过来
        val ask = s("s1", "waiting", "2026-09-01T00:00:00Z", path = pAsk.path, asking = true, updated = "2026-09-01T00:00:00Z")
        val run = s("s2", "running", "2026-09-02T00:00:00Z", path = pRun.path, updated = "2026-09-02T00:00:00Z")
        val bg = s("s4", "waiting", "2026-09-02T12:00:00Z", path = pBg.path, updated = "2026-09-02T12:00:00Z").copy(background = true)
        val act = s("s3", "waiting", "2026-09-03T00:00:00Z", path = pAct.path, updated = "2026-09-03T00:00:00Z")
        val rows = projectRows(listOf(pAsk, pRun, pBg, pAct, pIdle), listOf(ask, run, bg, act))
        assertEquals(listOf("ask", "run", "bg", "act", "idle"), rows.map { it.project.name })
        assertEquals(
            listOf(ProjectState.NEEDS_REPLY, ProjectState.RUNNING, ProjectState.BACKGROUND, ProjectState.ACTIVE, ProjectState.INACTIVE),
            rows.map { it.state },
        )
        // 同状态里仍是最近更新的在前
        val r1 = s("r1", "running", "2026-09-08T00:00:00Z", path = pAsk.path, updated = "2026-09-08T00:00:00Z")
        val r2 = s("r2", "running", "2026-09-09T00:00:00Z", path = pRun.path, updated = "2026-09-09T00:00:00Z")
        assertEquals(listOf("run", "ask"), projectRows(listOf(pAsk, pRun), listOf(r1, r2)).map { it.project.name })
    }

    @Test fun withinTheSameStatusTheLatestUpdatedSessionWins() {
        val p2 = p.copy(path = "/r/c", name = "c")
        val p3 = p.copy(path = "/r/d", name = "d")
        // 最近输出、状态故意和 updated_at 反着来：只看 updated_at
        val a1 = s("a1", "running", "2026-09-02T13:00:00Z", updated = "2026-09-02T09:00:00Z")
        val c1 = s("c1", "waiting", "2026-09-02T08:00:00Z", path = p2.path, updated = "2026-09-02T11:00:00Z")
        val d1 = s("d1", "waiting", "2026-09-02T12:00:00Z", path = p3.path, asking = true, updated = "2026-09-02T10:00:00Z")
        val rows = projectRows(listOf(p, p2, p3), listOf(a1, c1, d1))
        // 状态先分组（待回复 d > 执行中 a > 已激活 c），时间只在同组里比
        assertEquals(listOf("d", "a", "c"), rows.map { it.project.name })
        assertEquals(listOf(ProjectState.NEEDS_REPLY, ProjectState.RUNNING, ProjectState.ACTIVE), rows.map { it.state })
        // 输出再多，updated_at 不变、状态不变，顺序就不变
        val churn = listOf(a1.copy(last_output_at = "2026-09-09T00:00:00Z", state = "waiting"), c1.copy(state = "running"), d1)
        assertEquals(listOf("d", "c", "a"), projectRows(listOf(p, p2, p3), churn).map { it.project.name })
        // 退出的会话也算「更新」，但它是未激活，排在还活着的后面
        val x = s("x", "exited", "2026-09-02T00:00:00Z", updated = "2026-09-03T00:00:00Z")
        val rows2 = projectRows(listOf(p, p2), listOf(x, c1))
        assertEquals(listOf("c", "a"), rows2.map { it.project.name })
        assertEquals(ProjectState.INACTIVE, rows2[1].state)
    }

    @Test fun unreadRowsFloatAboveRunningOnesButNotAbovePinned() {
        // 2026-09-08 用户拍板：置顶 > 有黄点 > 在跑 > 其余
        val pRun = p.copy(path = "/r/run", name = "run")
        val pUnread = p.copy(path = "/r/unread", name = "unread")
        val pTop = p.copy(path = "/r/top", name = "top", pinned = true)
        val run = s("s1", "running", "2026-09-09T00:00:00Z", path = pRun.path, updated = "2026-09-09T00:00:00Z")
        val done = s("s2", "waiting", "2026-09-01T00:00:00Z", path = pUnread.path, updated = "2026-09-01T00:00:00Z")
        val rows = projectRows(listOf(pRun, pUnread, pTop), listOf(run, done), setOf(pUnread.path))
        assertEquals(listOf("top", "unread", "run"), rows.map { it.project.name })
        assertTrue(rows[1].unread)
        assertFalse(rows[2].unread)
        // 没给 unread 集合时谁都不是黄点，顺序回到「在跑的在前」
        assertEquals(listOf("top", "run", "unread"), projectRows(listOf(pRun, pUnread, pTop), listOf(run, done)).map { it.project.name })
    }

    @Test fun pinnedProjectsComeFirst() {
        val old = p.copy(path = "/r/old", name = "old", mtime = "2026-01-01T00:00:00Z", pinned = true)
        val fresh = p.copy(path = "/r/new", name = "new", mtime = "2026-09-01T00:00:00Z")
        assertEquals(listOf("old", "new"), projectRows(listOf(fresh, old), emptyList()).map { it.project.name })
        // 置顶是自己按的，待回复也挤不掉它
        val ask = s("n1", "waiting", "2026-09-09T00:00:00Z", path = fresh.path, asking = true, updated = "2026-09-09T00:00:00Z")
        val rows = projectRows(listOf(fresh, old), listOf(ask))
        assertEquals(listOf("old", "new"), rows.map { it.project.name })
        assertEquals(ProjectState.NEEDS_REPLY, rows[1].state)
    }

    @Test fun oldDaemonWithoutUpdatedAtFallsBackToCreatedAt() {
        val p2 = p.copy(path = "/r/c", name = "c", mtime = "2026-09-05T00:00:00Z")
        // 两边都是未激活，时间才说了算：a 的会话已退出，只贡献 created_at
        val early = s("a1", "exited", "2026-09-02T00:00:00Z").copy(created_at = "2026-09-02T00:00:00Z")
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
