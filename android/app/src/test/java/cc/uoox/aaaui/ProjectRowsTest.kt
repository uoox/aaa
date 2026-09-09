package cc.uoox.aaaui

import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.boolean
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 项目列表的行：顺序、底色、标题。数据一律来自三端共享向量 `fixtures/projects.json`——
 * mac 那边的测试读的是同一份、认的是同样几个 expect 键。此前两端各测各的一套，所以谁都没发现
 * 两边推出来的标题和状态不一样（同一个项目在 mac 和手机上显示的东西能对不上）。
 *
 * v1.22 起 `status` / `title` / `session_id` / `updated_at` 都是 daemon 算好的，客户端只剩两件事：
 * 按「黄底 > 状态 > 时间」排，和「这一行的底色画不画成蓝的 / 黄的」。所以这里不给 sessions——
 * 行不再依赖它。
 */
class ProjectRowsTest {
    private val json = ProtocolJson.instance
    private val fx = json.parseToJsonElement(java.io.File("../../fixtures/projects.json").readText()).jsonObject
    private val projects: List<Project> =
        json.decodeFromJsonElement(ListSerializer(Project.serializer()), fx["projects"]!!)
    private val unread: Set<String> = fx["unread"]!!.jsonArray.map { it.jsonPrimitive.content }.toSet()
    private val expect = fx["expect"]!!.jsonObject
    private val rows = projectRows(projects, emptyList(), unread)

    private fun expectPaths(k: String) = expect[k]!!.jsonArray.map { it.jsonPrimitive.content }

    /** 黄底 > 状态档 > 时间倒序 > 路径；档与时间都读 daemon 给的 status / updated_at */
    @Test fun `顺序是黄底 状态 时间`() {
        assertEquals(expectPaths("order"), rows.map { it.project.path })
    }

    /** 淡蓝底 = status ∈ {running, background}：它还在动，不用你管。asking 不蓝——那是在等你。 */
    @Test fun `在跑的两态才画淡蓝底`() {
        val blue = expectPaths("running").toSet()
        rows.forEach { r ->
            assertEquals(r.project.path, r.project.path in blue, statusRunning(r.status))
        }
    }

    /** 淡黄底 = 本机 unread 集合里有它（跨设备不同步：说的是「我这台还没看」） */
    @Test fun `淡黄底来自本机未读集合`() {
        assertEquals(expectPaths("unread_rows").toSet(), rows.filter { it.unread }.map { it.project.path }.toSet())
    }

    /**
     * 整行淡底的判定（2026-09-10 用户拍板「去掉竖线状态的设计，改为背景色，用浅色」）：
     * **黄盖过蓝**——黄的那个在等你；已读且不在跑 = 没有底色。
     */
    @Test fun `行底色是黄盖过蓝 已读的不画`() {
        assertEquals(Tok.RowUnread, rowBackground("running", unread = true))
        assertEquals(Tok.RowRunning, rowBackground("running", unread = false))
        assertEquals(Tok.RowRunning, rowBackground("background", unread = false))
        // asking 不蓝——那是在等你，蓝底说的是「不用你管」
        assertNull(rowBackground("asking", unread = false))
        assertNull(rowBackground("paused", unread = false))
        // 正在 POST /sessions 的那一行先按「在跑」画，别等 daemon 那一帧回来
        assertEquals(Tok.RowRunning, rowBackground("paused", unread = false, busy = true))
    }

    /** 标题回退链在 daemon 里走完，客户端直接用 `title`；空白才退到目录名 */
    @Test fun `标题直接用 daemon 给的那个`() {
        expect["titles"]!!.jsonObject.forEach { (path, want) ->
            assertEquals(path, want.jsonPrimitive.content, rows.first { it.project.path == path }.title)
        }
        // 只兜空白这一种情况：老 daemon 不给 title，退到目录名（文件夹名不是标题，它是地址）
        val bare = Project(path = "/p/x", name = "x", title = "  ")
        assertEquals("x", projectRows(listOf(bare), emptyList()).single().title)
    }

    /**
     * 未读 / 静音集合按路径存，`/p/a` 与 `/p/a/` 必须算同一个项目。以前这边是裸的字符串相等：
     * 黄点打在带尾斜杠的那份上、进会话时按不带斜杠的那份去清，点就消不掉了。
     * 只削尾斜杠、不做前缀匹配——`/p/b` 不是 `/p/bg`。
     */
    @Test fun `路径比对忽略尾斜杠但不认前缀`() {
        expect["unread_path_normalized"]!!.jsonObject.forEach { (path, want) ->
            assertEquals(path, want.jsonPrimitive.boolean, pathListContains(unread, path))
        }
        // 集合里存的是 /p/bg/，行上的路径是 /p/bg：黄点得打在那一行上
        assertTrue(unread.contains("/p/bg/"))
        assertTrue(rows.first { it.project.path == "/p/bg" }.unread)
        assertFalse(rows.first { it.project.path == "/p/bare" }.unread)
    }

    /** 不认识的状态（老 daemon 的空串）沉到最后，也不画淡蓝底：宁可排在底下，也不假装懂它 */
    @Test fun `未知状态排最后且不画淡蓝底`() {
        assertTrue(statusRank("") > statusRank("paused"))
        assertFalse(statusRunning(""))
        val unknown = Project(path = "/p/z", name = "z", status = "", updated_at = "2099-01-01T00:00:00Z")
        assertEquals("/p/z", projectRows(projects + unknown, emptyList(), emptySet()).last().project.path)
    }

    /** 代表会话是 daemon 指的那一条；池子里没有它（已被 daemon 清掉）也照样成行 */
    @Test fun `代表会话按 session_id 查`() {
        val sess = Session(id = "s_ask", project_path = "/p/ask", agent = "claude", state = "waiting")
        val withSession = projectRows(projects, listOf(sess), unread)
        assertEquals("s_ask", withSession.first { it.project.path == "/p/ask" }.primary?.id)
        assertEquals(null, withSession.first { it.project.path == "/p/run" }.primary)
        assertEquals(null, rows.first { it.project.path == "/p/bare" }.primary)
    }

    /**
     * agent 轮换：只在装了不止一个时才有「下一个」。少于两个返回 null，
     * 两端据此整个不画切换入口（老 daemon 不给 /agents，表是空的，同样不画）。
     */
    @Test
    fun next_agent_cycles_only_over_installed_ones() {
        val both = listOf(
            AgentInfo("claude", "Claude", true),
            AgentInfo("agy", "Antigravity", true),
        )
        assertEquals("agy", nextAgent(both, "claude"))
        assertEquals("claude", nextAgent(both, "agy"))
        assertEquals("当前这个没装就给条路回到装了的", "claude", nextAgent(both, "没见过的"))
        assertNull(nextAgent(listOf(AgentInfo("claude", "Claude", true)), "claude"))
        assertNull("老 daemon：空表不画切换", nextAgent(emptyList(), "claude"))
        val onlyClaude = both.map { it.copy(available = it.id == "claude") }
        assertNull("没装的不算", nextAgent(onlyClaude, "claude"))
        assertEquals("agy 卸载了，这一行还能换回 claude", "claude", nextAgent(onlyClaude, "agy"))
        assertEquals("Antigravity", agentLabel(both, "agy"))
        assertEquals("表里没有就报 id", "agy", agentLabel(emptyList(), "agy"))
    }
}
