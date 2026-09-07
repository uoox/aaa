package cc.uoox.aaaui

// ============================================================
// 消息流按「轮」折叠——纯函数，不碰 Compose，TurnFoldTest 直接跑
// ============================================================

/**
 * 一轮 = 用户发出的一条 text 到下一条之间的全部消息。**assistant 的每一条 text 都是回答**，
 * 一律露出（Claude 的回答天生分段：说一句 → 干活 → 再说一句）；2026-09-07 之前只留最后
 * 一条、其余折进过程，看起来就像把回答当成了思考。折叠里只有思考 / 工具调用 / 工具结果 /
 * system；question / answer 永不折叠。
 *
 * [body] 是除用户消息外的全部消息，保持原序。
 * [live]：会话仍在 running 且这是最后一轮——贴在轮尾的那段过程画成「进行中」并带最近一步。
 */
data class Turn(
    val user: ChatMessage?,
    val body: List<ChatMessage>,
    val key: Long,
    val live: Boolean = false,
) {
    /** 轮内所有会被折叠的消息（思考 / 工具 / system） */
    val steps: List<ChatMessage> get() = body.filter { it.isStep() }

    /** 轮内所有回答（assistant text），按顺序 */
    val replies: List<ChatMessage> get() = body.filter { it.isAssistantText() }
}

/** LazyColumn 的一项。key 带类型前缀，同一条消息从 Reply 变成 Step 时 key 跟着变。 */
sealed class StreamItem {
    abstract val key: String

    data class User(val msg: ChatMessage) : StreamItem() {
        override val key: String get() = "u${msg.seq}"
    }

    /**
     * 折叠段；[foldKey] = 段内第一条消息的 seq（展开状态按它记）。
     * [liveTail] 是 live 尾段里最后一条 tool_use，折叠着也能看出它在干什么。
     */
    data class Fold(
        val foldKey: Long,
        val steps: List<ChatMessage>,
        val liveTail: ChatMessage?,
        val live: Boolean = false,
    ) : StreamItem() {
        override val key: String get() = "f$foldKey"
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
private fun ChatMessage.isForm() = kind == "question" || kind == "answer"

/** 会被折叠的：思考 / 工具调用 / 工具结果 / system 提示。回答与表单永远不折。 */
private fun ChatMessage.isStep() = !isAssistantText() && !isForm()

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
        Turn(
            user = user,
            body = if (user != null) g.drop(1) else g,
            key = g.first().seq,
            live = live && i == groups.lastIndex,
        )
    }
}

/** 连续的过程消息切成段（原序）。 */
private fun segments(body: List<ChatMessage>): List<List<ChatMessage>> {
    val segs = mutableListOf<MutableList<ChatMessage>>()
    var open = false
    for (m in body) {
        if (m.isStep()) {
            if (!open) { segs.add(mutableListOf()); open = true }
            segs.last().add(m)
        } else {
            open = false
        }
    }
    return segs
}

/**
 * 轮 → 列表项。轮内先出 User，随后按真实顺序走：连续的过程消息并成一个 Fold（展开时
 * 紧跟它的 Step），回答 / question / answer 各自成项。只有 live 轮**贴在轮尾**的那一段
 * 画成「进行中」——后面还有回答，说明那段已经结束了。
 */
fun flattenForList(turns: List<Turn>, expanded: Set<Long>): List<StreamItem> = buildList {
    for (t in turns) {
        t.user?.let { add(StreamItem.User(it)) }
        val segs = segments(t.body)
        val liveKey = if (t.live && t.body.lastOrNull()?.isStep() == true) segs.lastOrNull()?.first()?.seq else null
        var segIx = 0
        var inSeg = false
        for (m in t.body) {
            if (m.isStep()) {
                if (!inSeg) {
                    val seg = segs[segIx]
                    val foldKey = seg.first().seq
                    val isLive = foldKey == liveKey
                    add(
                        StreamItem.Fold(
                            foldKey = foldKey,
                            steps = seg,
                            liveTail = if (isLive) seg.lastOrNull { it.kind == "tool_use" } else null,
                            live = isLive,
                        )
                    )
                    if (foldKey in expanded) addAll(seg.map { StreamItem.Step(it) })
                    segIx++
                    inSeg = true
                }
                continue
            }
            inSeg = false
            add(
                when {
                    m.isAssistantText() -> StreamItem.Reply(m)
                    m.kind == "question" -> StreamItem.Question(m)
                    else -> StreamItem.Answer(m)
                }
            )
        }
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

/** 步数 = tool_use 条数；thinking / 结果不算步。 */
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

/** live 尾段的折叠态标签：`进行中 · 5 步 · 最近：Bash cargo test`。 */
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
