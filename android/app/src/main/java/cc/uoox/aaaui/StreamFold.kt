package cc.uoox.aaaui

// ============================================================
// 消息流按「轮」折叠——纯函数，不碰 Compose，TurnFoldTest 直接跑
// ============================================================

/**
 * 一轮 = 用户发出的一条 text 到下一条之间的全部消息。默认只露出用户消息与这一轮
 * 最后一条 assistant text（[reply]）；中间的思考 / 工具调用 / 工具结果 / 中途文本
 * 全部折进 [process]；question / answer 永不折叠（[questions]）。
 *
 * [live]：会话仍在 running 且这是最后一轮——过程行画成「进行中」并带最近一步。
 * 最新那条 assistant text 暂当 reply，新工具调用来了它自然滚进 process。
 */
data class Turn(
    val user: ChatMessage?,
    val process: List<ChatMessage>,
    val reply: ChatMessage?,
    val questions: List<ChatMessage>,
    val key: Long,
    val live: Boolean = false,
)

/** LazyColumn 的一项。key 带类型前缀，同一条消息从 Reply 变成 Step 时 key 跟着变。 */
sealed class StreamItem {
    abstract val key: String

    data class User(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "u${msg.seq}"
    }

    /** 折叠行；[liveTail] 是 live 尾轮里最后一条 tool_use，折叠着也能看出它在干什么。 */
    data class Fold(
        val turnKey: Long,
        val steps: List<ChatMessage>,
        val liveTail: ChatMessage?,
        val live: Boolean = false,
    ) : StreamItem() {
        override val key: String get() = "f$turnKey"
    }

    data class Step(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "s${msg.seq}"
    }

    data class Reply(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "r${msg.seq}"
    }

    data class Question(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "q${msg.seq}"
    }

    data class Answer(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "a${msg.seq}"
    }
}

private fun ChatMessage.startsTurn() = role == "user" && kind == "text"
private fun ChatMessage.isAssistantText() = role == "assistant" && kind == "text"

/**
 * 每条 user text 开一轮；第一轮之前的非 user 消息归入一个 `user == null` 的首轮
 * （增量拉取只拿到后半段时常见）。轮的 key 取轮内第一条消息的 seq——用户轮就是
 * 用户消息的 seq，跨次刷新稳定。
 */
fun foldTurns(messages: List<ChatMessage>, live: Boolean): List<Turn> {
    if (messages.isEmpty()) return emptyList()
    val groups = mutableListOf<MutableList<ChatMessage>>()
    for (m in messages) {
        if (groups.isEmpty() || m.startsTurn()) groups.add(mutableListOf())
        groups.last().add(m)
    }
    return groups.mapIndexed { i, g ->
        val user = g.first().takeIf { it.startsTurn() }
        val body = if (user != null) g.drop(1) else g
        val questions = body.filter { it.kind == "question" || it.kind == "answer" }
        val rest = body.filter { it.kind != "question" && it.kind != "answer" }
        val reply = rest.lastOrNull { it.isAssistantText() }
        Turn(
            user = user,
            process = if (reply == null) rest else rest.filter { it.seq != reply.seq },
            reply = reply,
            questions = questions,
            key = g.first().seq,
            live = live && i == groups.lastIndex,
        )
    }
}

/**
 * 轮 → 列表项。轮内先出 User，随后 Fold / Reply / Question 按各自起始 seq 排序
 * （question 可能出现在过程中间，也可能在回复之后，跟着真实顺序走）；展开的 Fold
 * 紧跟各 Step。process 为空不出 Fold。
 */
fun flattenForList(turns: List<Turn>, expanded: Set<Long>): List<StreamItem> = buildList {
    for (t in turns) {
        t.user?.let { add(StreamItem.User(it)) }
        val blocks = mutableListOf<Pair<Long, List<StreamItem>>>()
        if (t.process.isNotEmpty()) {
            val fold = StreamItem.Fold(
                turnKey = t.key,
                steps = t.process,
                liveTail = if (t.live) t.process.lastOrNull { it.kind == "tool_use" } else null,
                live = t.live,
            )
            val block = if (t.key in expanded) listOf(fold) + t.process.map { StreamItem.Step(it) } else listOf(fold)
            blocks += t.process.first().seq to block
        }
        t.reply?.let { blocks += it.seq to listOf(StreamItem.Reply(it)) }
        t.questions.forEach {
            val item = if (it.kind == "question") StreamItem.Question(it) else StreamItem.Answer(it)
            blocks += it.seq to listOf(item)
        }
        blocks.sortBy { it.first }
        blocks.forEach { addAll(it.second) }
    }
}

/**
 * 待答的表单：会话还活着，且最新一条 question 后面没有 answer，且不早于会话进程的
 * created_at（与 daemon 同一口径）。客户端若仍判错，提交会得到 409，卡上会显示原因。
 */
fun pendingQuestionSeq(messages: List<ChatMessage>, alive: Boolean, since: String? = null): Long? {
    if (!alive) return null
    for (m in messages.asReversed()) {
        if (m.kind == "question") {
            // 早于本进程 created_at 的悬置问题（resume 带进来的）不算待答；秒级前缀比较，
            // created_at 是整秒、transcript 时间戳带毫秒
            if (since != null && since.length >= 19 && m.ts.length >= 19 && m.ts.substring(0, 19) < since.substring(0, 19)) return null
            return m.seq
        }
        if (m.kind == "answer") return null
    }
    return null
}

/** 已被回答的 question：每条 answer 归到它前面最近的那条 question。 */
fun answeredQuestionSeqs(messages: List<ChatMessage>): Set<Long> {
    val out = HashSet<Long>()
    var lastQuestion: Long? = null
    for (m in messages) {
        when (m.kind) {
            "question" -> lastQuestion = m.seq
            "answer" -> { lastQuestion?.let { out += it }; lastQuestion = null }
        }
    }
    return out
}

/** 步数 = tool_use 条数；thinking / 中途文本 / 结果都不算步。 */
fun stepCount(steps: List<ChatMessage>): Int = steps.count { it.kind == "tool_use" }

private fun ChatMessage.toolName(): String = tool?.name?.ifBlank { null } ?: "tool"

/** 折叠态标签：`过程 · 10 步 · Bash ×7 · Read ×3`——工具名按次数降序取前三，同次数按首次出现。 */
fun foldLabel(steps: List<ChatMessage>): String {
    val counts = LinkedHashMap<String, Int>()
    steps.filter { it.kind == "tool_use" }.forEach { counts.merge(it.toolName(), 1, Int::plus) }
    return buildString {
        append("过程 · ").append(stepCount(steps)).append(" 步")
        counts.entries.sortedByDescending { it.value }.take(3).forEach { append(" · ").append(it.key).append(" ×").append(it.value) }
    }
}

/** live 尾轮的折叠态标签：`进行中 · 5 步 · 最近：Bash cargo test`。 */
fun liveLabel(steps: List<ChatMessage>, tail: ChatMessage?): String {
    val base = "进行中 · ${stepCount(steps)} 步"
    if (tail == null) return base
    val what = listOf(tail.toolName(), tail.tool?.summary.orEmpty()).filter { it.isNotBlank() }.joinToString(" ")
    return "$base · 最近：$what"
}

/**
 * 消息集变化后要不要滚到底：首批数据总是到底；之后只有变化**前**就在底部才跟——
 * 用户正在翻历史时新消息不该把人拽下去。
 */
fun shouldFollowTail(wasAtBottom: Boolean, hadMessages: Boolean, hasMessages: Boolean): Boolean =
    hasMessages && (!hadMessages || wasAtBottom)
