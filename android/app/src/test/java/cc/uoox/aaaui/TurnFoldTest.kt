package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 消息流按轮折叠的纯逻辑。消息形状照 PROTOCOL.md：tool_use / tool_result 的 role
 * 是 "tool"（daemon 实际如此），用户消息 role=user kind=text，问题 kind=question。
 */
class TurnFoldTest {
    private fun msg(seq: Long, role: String, kind: String, text: String = "", tool: ToolInfo? = null) =
        ChatMessage(seq = seq, ts = "", role = role, kind = kind, text = text, tool = tool)
    private fun user(seq: Long, text: String = "做") = msg(seq, "user", "text", text)
    private fun say(seq: Long, text: String = "好") = msg(seq, "assistant", "text", text)
    private fun think(seq: Long) = msg(seq, "assistant", "thinking", "hmm")
    private fun use(seq: Long, name: String, summary: String = "") = msg(seq, "tool", "tool_use", tool = ToolInfo(name, summary, "ok"))
    private fun result(seq: Long) = msg(seq, "tool", "tool_result", "out", ToolInfo("", "", "ok"))
    private fun question(seq: Long) = msg(seq, "assistant", "question", "which?")
    private fun answer(seq: Long) = msg(seq, "user", "answer", "that one", ToolInfo("AskUserQuestion", "", "ok"))

    // 1. 两轮基本切分
    @Test fun splitsIntoTurnsOnUserText() {
        val turns = foldTurns(listOf(user(1), use(2, "Bash"), result(3), say(4, "done"), user(5), say(6, "ok")), live = false)
        assertEquals(2, turns.size)
        assertEquals(1L, turns[0].user?.seq)
        assertEquals(listOf(2L, 3L), turns[0].steps.map { it.seq })
        assertEquals(listOf(4L), turns[0].replies.map { it.seq })
        assertEquals(1L, turns[0].key)
        assertEquals(5L, turns[1].user?.seq)
        assertTrue(turns[1].steps.isEmpty())
        assertEquals(listOf(6L), turns[1].replies.map { it.seq })
    }

    // 2. 轮内每条 assistant text 都是回答，过程按位置切成多段
    //    （2026-09-07 修：以前只留最后一条，中间几段真回答被折进「过程」，看着像思考）
    @Test fun everyAssistantTextIsAReplyAndProcessSplitsAroundThem() {
        val t = foldTurns(listOf(user(1), say(2, "先看看"), use(3, "Read"), result(4), say(5, "再改"), use(6, "Edit"), say(7, "改好了")), live = false).single()
        assertEquals(listOf(2L, 5L, 7L), t.replies.map { it.seq })
        assertEquals(listOf(3L, 4L, 6L), t.steps.map { it.seq })
        assertEquals(listOf("u1", "r2", "f3", "r5", "f6", "r7"), flattenForList(listOf(t), emptySet()).map { it.key })
        // 展开只展开被点的那一段
        assertEquals(
            listOf("u1", "r2", "f3", "s3", "s4", "r5", "f6", "r7"),
            flattenForList(listOf(t), expanded = setOf(3L)).map { it.key },
        )
    }

    // 3. question 永不折叠，也不吃掉回答
    @Test fun questionsStayOutOfTheFold() {
        val t = foldTurns(listOf(user(1), use(2, "Bash"), question(3)), live = false).single()
        assertTrue(t.replies.isEmpty())
        assertEquals(listOf(2L), t.steps.map { it.seq })
        assertEquals(listOf("u1", "f2", "q3"), flattenForList(listOf(t), emptySet()).map { it.key })

        // 问题排在回复之后也不改变它独立成项
        val t2 = foldTurns(listOf(user(1), say(2), question(3)), live = false).single()
        assertEquals(listOf("u1", "r2", "q3"), flattenForList(listOf(t2), emptySet()).map { it.key })
    }

    // 3b. answer 与 question 一样独立成项、不折叠；键前缀 a
    @Test fun answersAreStandaloneItemsToo() {
        val t = foldTurns(listOf(user(1), question(2), answer(3), use(4, "Edit"), say(5)), live = false).single()
        assertEquals(listOf(4L), t.steps.map { it.seq })
        assertEquals(listOf("u1", "q2", "a3", "f4", "r5"), flattenForList(listOf(t), emptySet()).map { it.key })
    }

    // 3c. 待答 = 会话活着且最新 question 后没有 answer
    @Test fun pendingQuestionIsTheNewestUnanswered() {
        val asked = listOf(user(1), question(2))
        assertEquals(2L, pendingQuestionSeq(asked, alive = true))
        assertNull(pendingQuestionSeq(asked, alive = false))
        assertNull(pendingQuestionSeq(asked + answer(3), alive = true))
        // 新问题顶掉旧问题：只有最新的待答
        assertEquals(4L, pendingQuestionSeq(asked + answer(3) + question(4), alive = true))
        assertEquals(4L, pendingQuestionSeq(asked + question(4), alive = true))
        assertNull(pendingQuestionSeq(listOf(user(1), say(2)), alive = true))
        // resume 带进来的旧问题：早于本进程 created_at 的不算待答
        val old = msg(2, "assistant", "question", "old?").copy(ts = "2026-09-02T09:59:59.900Z")
        assertNull(pendingQuestionSeq(listOf(user(1), old), alive = true, since = "2026-09-02T10:00:00Z"))
        val fresh = old.copy(ts = "2026-09-02T10:00:00.500Z")
        assertEquals(2L, pendingQuestionSeq(listOf(user(1), fresh), alive = true, since = "2026-09-02T10:00:00Z"))
        // 已回答集合：answer 归到前面最近的 question
        assertEquals(setOf(2L), answeredQuestionSeqs(asked + answer(3) + question(4)))
        assertEquals(emptySet<Long>(), answeredQuestionSeqs(asked))
    }

    // 4. 首条非 user 的消息归入 user==null 首轮
    @Test fun leadingNonUserMessagesFormAHeadlessTurn() {
        val turns = foldTurns(listOf(use(7, "Bash"), result(8), say(9), user(10), say(11)), live = false)
        assertEquals(2, turns.size)
        assertNull(turns[0].user)
        assertEquals(7L, turns[0].key)
        assertEquals(listOf(7L, 8L), turns[0].steps.map { it.seq })
        assertEquals(listOf(9L), turns[0].replies.map { it.seq })
        assertEquals(10L, turns[1].user?.seq)
    }

    // 5. 没有过程 → 不出 Fold 项
    @Test fun emptyProcessEmitsNoFold() {
        val turns = foldTurns(listOf(user(1), say(2)), live = true)
        val items = flattenForList(turns, expanded = setOf(1L))
        assertEquals(listOf("u1", "r2"), items.map { it.key })
        assertFalse(items.any { it is StreamItem.Fold })
    }

    // 6. Fold 标签：计数与排序，thinking 不计
    @Test fun foldLabelCountsToolUsesAndRanksNames() {
        val steps = buildList {
            add(think(1))
            (2L..8L).forEach { add(use(it, "Bash")) }
            (9L..11L).forEach { add(use(it, "Read")) }
            add(result(12))
        }
        assertEquals(10, stepCount(steps))
        assertEquals("过程 · 10 步 · Bash ×7 · Read ×3", foldLabel(steps))

        // 前三之外的工具不列；同次数按首次出现
        val many = listOf(use(1, "Edit"), use(2, "Grep"), use(3, "Bash"), use(4, "Bash"), use(5, "Read"), use(6, "Write"))
        assertEquals("过程 · 6 步 · Bash ×2 · Edit ×1 · Grep ×1", foldLabel(many))
        assertEquals("过程 · 0 步", foldLabel(listOf(think(1))))
    }

    // 7. 只有贴在轮尾的那一段是 live，liveTail = 段内最后一条 tool_use
    @Test fun onlyTheTrailingSegmentOfTheLiveTurnIsLive() {
        val msgs = listOf(user(1), use(2, "Read", "a.kt"), result(3), say(4, "看完了"), use(5, "Bash", "cargo test"), result(6))
        val live = flattenForList(foldTurns(msgs, live = true), emptySet())
        assertEquals(listOf("u1", "f2", "r4", "f5"), live.map { it.key })
        val folds = live.filterIsInstance<StreamItem.Fold>()
        assertEquals(listOf(false, true), folds.map { it.live })
        assertEquals(5L, folds[1].liveTail?.seq)
        assertEquals("进行中 · 1 步 · 最近：Bash cargo test", liveLabel(folds[1].steps, folds[1].liveTail))

        // 会话不在 running：没有 live 段
        val idle = flattenForList(foldTurns(msgs, live = false), emptySet()).filterIsInstance<StreamItem.Fold>()
        assertTrue(idle.none { it.live } && idle.all { it.liveTail == null })

        // 只有最后一轮算 live
        val two = flattenForList(foldTurns(msgs + listOf(user(7), use(8, "Edit")), live = true), emptySet())
            .filterIsInstance<StreamItem.Fold>()
        assertEquals(listOf(false, false, true), two.map { it.live })
        assertEquals(8L, two[2].liveTail?.seq)

        // 尾段后面又出了回答 → 那段不再是「进行中」
        val done = flattenForList(foldTurns(listOf(user(1), use(2, "Bash"), say(3)), live = true), emptySet())
        assertTrue(done.filterIsInstance<StreamItem.Fold>().none { it.live })
    }

    @Test fun expandedFoldIsFollowedByItsSteps() {
        val turns = foldTurns(listOf(user(1), think(2), use(3, "Bash"), result(4), say(5), user(6), use(7, "Read"), say(8)), live = false)
        // 展开状态按段键（段内第一条 seq）记
        val items = flattenForList(turns, expanded = setOf(2L))
        assertEquals(listOf("u1", "f2", "s2", "s3", "s4", "r5", "u6", "f7", "r8"), items.map { it.key })
        val collapsed = flattenForList(turns, expanded = emptySet())
        assertEquals(listOf("u1", "f2", "r5", "u6", "f7", "r8"), collapsed.map { it.key })
    }

    @Test fun emptyInputFoldsToNothing() {
        assertTrue(foldTurns(emptyList(), live = true).isEmpty())
        assertTrue(flattenForList(emptyList(), emptySet()).isEmpty())
    }

    // 滚动判定：首批数据到底；之后只有变化前在底部才跟
    @Test fun followsTailOnlyWhenAlreadyAtBottomOrOnFirstData() {
        assertTrue(shouldFollowTail(wasAtBottom = false, hadMessages = false, hasMessages = true))
        assertTrue(shouldFollowTail(wasAtBottom = true, hadMessages = true, hasMessages = true))
        assertFalse(shouldFollowTail(wasAtBottom = false, hadMessages = true, hasMessages = true))
        assertFalse(shouldFollowTail(wasAtBottom = true, hadMessages = true, hasMessages = false))
    }
}
