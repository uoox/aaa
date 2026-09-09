package cc.uoox.aaaui

import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 看板分节（2026-09-08 用户拍板「看板里面东西太多了，要做一下分割，即在 AAA 里面的对话，
 * 和不在里面的对话」）。数据一律来自三端共享向量 `fixtures/dashboard.json`——mac 那边的分节
 * 测试认的是同样三个 expect 键（`in_aaa` / `not_in_aaa` / `not_in_aaa_with_deleted`），两端口
 * 径必须一致，所以这里不自己造 SessionCard。
 *
 * 盯三件事：过滤（搜索词 + 已删除开关）在分节之前发生；daemon 给的 `sessions` 顺序在节内一字
 * 不动（客户端只切，不重排）；搜索框里有字时 `searching` 为真，UI 据此不许「不在 AAA 里」那
 * 节折着（藏起来 = 没搜到）。
 */
class DashboardSectionsTest {
    private val json = ProtocolJson.instance
    private val fx = json.parseToJsonElement(java.io.File("../../fixtures/dashboard.json").readText()).jsonObject
    private val cards: List<SessionCard> =
        json.decodeFromString<Dashboard>("""{"sessions":${fx["sessions"]}}""").sessions

    private fun expect(k: String) = fx["expect"]!!.jsonObject[k]!!.jsonArray.map { it.jsonPrimitive.content }

    /** 默认：已删除的两节都不收；节内还是 daemon 那个先后 */
    @Test fun `按还活着与否切两节且节内保序`() {
        val s = dashboardSections(cards, "", showDeleted = false)
        assertEquals(expect("in_aaa"), s.inAaa.map { it.id })
        assertEquals(expect("not_in_aaa"), s.gone.map { it.id })
        assertEquals(expect("in_aaa").size + expect("not_in_aaa").size, s.total)
    }

    /** 开关打开：已删除的那条接回它本来那一节的原处，第一节不受影响 */
    @Test fun `已删除开关只加不排`() {
        val s = dashboardSections(cards, "", showDeleted = true)
        assertEquals(expect("in_aaa"), s.inAaa.map { it.id })
        assertEquals(expect("not_in_aaa_with_deleted"), s.gone.map { it.id })
    }

    /** 分水岭：还在池子里（点得开）但进程已经退出的那条，算「不在 AAA 里」 */
    @Test fun `退出了还在池子里的不算在 AAA 里`() {
        val pooled = cards.first { it.id == "poolpau" }
        assertTrue(pooled.alive)
        assertFalse(cardInAaa(pooled))
        assertTrue(dashboardSections(cards, "", showDeleted = false).gone.any { it.id == "poolpau" })
    }

    /** 搜索走 cardMatches（标题 / 项目 / 清单项），命中的再分别落进两节 */
    @Test fun `搜索先过滤再分节`() {
        val s = dashboardSections(cards, "测试", showDeleted = false)
        assertEquals(listOf("run"), s.inAaa.map { it.id })
        assertEquals(listOf("old"), s.gone.map { it.id })
        // 共享向量里 match_测试 是这两条，只是分属两节
        assertEquals(expect("match_测试").toSet(), (s.inAaa + s.gone).map { it.id }.toSet())
    }

    @Test fun `搜索框有字就不许折起来`() {
        assertTrue(dashboardSections(cards, "测试", showDeleted = false).searching)
        // 空串 / 全空白都不算在搜索：cardMatches 本来就把它们当全匹配，这时该由用户决定折不折
        assertFalse(dashboardSections(cards, "", showDeleted = false).searching)
        val blank = dashboardSections(cards, "   ", showDeleted = false)
        assertFalse(blank.searching)
        assertEquals(expect("visible_default").size, blank.total)
    }

    @Test fun `一条都不剩时两节都是空的`() {
        val none = dashboardSections(cards, "没有这种东西的关键字", showDeleted = true)
        assertEquals(0, none.total)
        assertTrue(none.inAaa.isEmpty() && none.gone.isEmpty())
        assertEquals(0, dashboardSections(emptyList(), "", showDeleted = false).total)
    }

    /**
     * 待决策：只收「还在池子里、没删、asking」的，等得久的在前。
     * mac 那边同一口径（`pending_cards`），两端都盯着这几条。
     */
    @Test fun `待决策只收此刻卡在你身上的会话`() {
        val cards = listOf(
            SessionCard(id = "run", status = "running", alive = true, updated_at = "2026-09-10T01:00:00Z"),
            SessionCard(id = "new", status = "asking", alive = true, updated_at = "2026-09-10T03:00:00Z"),
            SessionCard(id = "old", status = "asking", alive = true, updated_at = "2026-09-10T02:00:00Z"),
            SessionCard(id = "gone", status = "asking", alive = false, updated_at = "2026-09-10T02:30:00Z"),
            SessionCard(id = "del", status = "asking", alive = true, deleted = true, updated_at = "2026-09-10T02:40:00Z"),
        )
        assertEquals(listOf("old", "new"), pendingCards(cards).map { it.id })
        assertTrue(pendingCards(emptyList()).isEmpty())
    }
}
