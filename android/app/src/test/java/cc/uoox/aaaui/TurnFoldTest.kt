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

    // 1. 两轮基本切分
    @Test fun splitsIntoTurnsOnUserText() {
        val turns = foldTurns(listOf(user(1), use(2, "Bash"), result(3), say(4, "done"), user(5), say(6, "ok")), live = false)
        assertEquals(2, turns.size)
        assertEquals(1L, turns[0].user?.seq)
        assertEquals(listOf(2L, 3L), turns[0].process.map { it.seq })
        assertEquals(4L, turns[0].reply?.seq)
        assertEquals(1L, turns[0].key)
        assertEquals(5L, turns[1].user?.seq)
        assertTrue(turns[1].process.isEmpty())
        assertEquals(6L, turns[1].reply?.seq)
    }

    // 2. 轮内多条 assistant text：只有最后一条是 reply
    @Test fun onlyTheLastAssistantTextIsTheReply() {
        val t = foldTurns(listOf(user(1), say(2, "先看看"), use(3, "Read"), say(4, "再改"), use(5, "Edit"), say(6, "改好了")), live = false).single()
        assertEquals(6L, t.reply?.seq)
        assertEquals(listOf(2L, 3L, 4L, 5L), t.process.map { it.seq })
    }

    // 3. question 永不折叠且不占 reply 位
    @Test fun questionsStayOutOfProcessAndReply() {
        val t = foldTurns(listOf(user(1), use(2, "Bash"), question(3)), live = false).single()
        assertEquals(listOf(3L), t.questions.map { it.seq })
        assertNull(t.reply)
        assertEquals(listOf(2L), t.process.map { it.seq })
        val items = flattenForList(listOf(t), emptySet())
        assertEquals(listOf("u1", "f1", "q3"), items.map { it.key })

        // 问题排在回复之后也不改变它独立成项
        val t2 = foldTurns(listOf(user(1), say(2), question(3)), live = false).single()
        assertEquals(2L, t2.reply?.seq)
        assertEquals(listOf("u1", "r2", "q3"), flattenForList(listOf(t2), emptySet()).map { it.key })
    }

    // 4. 首条非 user 的消息归入 user==null 首轮
    @Test fun leadingNonUserMessagesFormAHeadlessTurn() {
        val turns = foldTurns(listOf(use(7, "Bash"), result(8), say(9), user(10), say(11)), live = false)
        assertEquals(2, turns.size)
        assertNull(turns[0].user)
        assertEquals(7L, turns[0].key)
        assertEquals(listOf(7L, 8L), turns[0].process.map { it.seq })
        assertEquals(9L, turns[0].reply?.seq)
        assertEquals(10L, turns[1].user?.seq)
    }

    // 5. process 空 → 不出 Fold 项
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
            add(say(13, "中途"))
        }
        assertEquals(10, stepCount(steps))
        assertEquals("过程 · 10 步 · Bash ×7 · Read ×3", foldLabel(steps))

        // 前三之外的工具不列；同次数按首次出现
        val many = listOf(use(1, "Edit"), use(2, "Grep"), use(3, "Bash"), use(4, "Bash"), use(5, "Read"), use(6, "Write"))
        assertEquals("过程 · 6 步 · Bash ×2 · Edit ×1 · Grep ×1", foldLabel(many))
        assertEquals("过程 · 0 步", foldLabel(listOf(think(1))))
    }

    // 7. live 尾轮的 liveTail = 最后一条 tool_use
    @Test fun liveTailIsTheLastToolUseOfTheLiveLastTurn() {
        val msgs = listOf(user(1), use(2, "Read", "a.kt"), result(3), say(4, "看完了"), use(5, "Bash", "cargo test"), result(6))
        val live = flattenForList(foldTurns(msgs, live = true), emptySet())
        val fold = live.filterIsInstance<StreamItem.Fold>().single()
        assertTrue(fold.live)
        assertEquals(5L, fold.liveTail?.seq)
        assertEquals("进行中 · 2 步 · 最近：Bash cargo test", liveLabel(fold.steps, fold.liveTail))
        // 最新那条 assistant text 暂当 reply；之后的工具调用已经滚进 process
        assertEquals(listOf("u1", "f1", "r4"), live.map { it.key })

        // 会话不在 running：没有 liveTail
        val idle = flattenForList(foldTurns(msgs, live = false), emptySet()).filterIsInstance<StreamItem.Fold>().single()
        assertFalse(idle.live)
        assertNull(idle.liveTail)

        // 只有最后一轮算 live
        val two = flattenForList(foldTurns(msgs + listOf(user(7), use(8, "Edit")), live = true), emptySet())
        val folds = two.filterIsInstance<StreamItem.Fold>()
        assertEquals(listOf(false, true), folds.map { it.live })
        assertEquals(8L, folds[1].liveTail?.seq)
    }

    @Test fun expandedFoldIsFollowedByItsSteps() {
        val turns = foldTurns(listOf(user(1), think(2), use(3, "Bash"), result(4), say(5), user(6), use(7, "Read"), say(8)), live = false)
        val items = flattenForList(turns, expanded = setOf(1L))
        assertEquals(listOf("u1", "f1", "s2", "s3", "s4", "r5", "u6", "f6", "r8"), items.map { it.key })
        val collapsed = flattenForList(turns, expanded = emptySet())
        assertEquals(listOf("u1", "f1", "r5", "u6", "f6", "r8"), collapsed.map { it.key })
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
