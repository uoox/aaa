package cc.uoox.aaaui

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull
import java.net.URI
import java.net.URLDecoder

// ---------- models (PROTOCOL.md v1 + v1.1) ----------

@Serializable data class Session(
    val id: String,
    val title: String = "",
    val project_path: String = "",
    val project_name: String = "",
    val agent: String = "",
    val state: String = "",
    val asking: Boolean = false,
    /** v1.13：waiting 且后台还有任务（后台 Bash / 异步子代理 / Monitor）没回来，会自己被叫醒 → 「后台」 */
    val background: Boolean = false,
    /** v1.16：正在等的权限对话框（Bash 授权 / ExitPlanMode 批准…）；消息流画成「允许 / 拒绝」卡片 */
    val permission: PermissionPrompt? = null,
    val rows: Int = 24,
    val cols: Int = 80,
    val exit_code: Int? = null,
    val resume_id: String? = null,
    val created_at: String = "",
    val last_output_at: String = "",
    /** v1.5：状态翻转 / 改名的时刻（不随每个 PTY 字节跳），首页按它排序；老 daemon 不给 → 空 */
    val updated_at: String = "",
    /** StopFailure 的错误类型：rate_limit / overloaded / authentication_failed… */
    val error: String? = null,
    /** v1.4：Claude Code statusline 喂来的用量（模型 / 上下文占比 / 花费）；没有就 null */
    val usage: SessionUsage? = null,
    /** v1.7：整个对话的进度清单（`- [x] 已做` / `- [ ] 未做` 的 markdown），每轮结束后 daemon 重写 */
    val summary: String = "",
    /**
     * v1.22：**待答的就是这一条**（消息流里的 `seq`），null = 没有待答表单。
     * 此前三端各自倒着找消息流判「最新一条 question 后面没有 answer」，比 `ts` 的方式还不一样
     * （daemon 整串比、两端取前 19 字符），同一秒里 daemon 说不是待答、客户端说是，点提交就是 409。
     */
    val queued: List<QueuedMsg> = emptyList(),
    val asking_seq: Long? = null,
    /** v1.22：[summary] 那串 markdown 由 daemon 解析好的结果；客户端直接画，不再各自解析 */
    val checklist: List<ChecklistItem> = emptyList(),
)

/** 进度清单的一项（daemon 解析 `summary` 得到，见 [Session.checklist]） */
@Serializable data class ChecklistItem(val done: Boolean, val text: String)

@Serializable data class SessionUsage(
    val model: String? = null,
    /** 0-100 */
    val context_pct: Double? = null,
    val cost_usd: Double? = null,
    val duration_ms: Long? = null,
    val lines_added: Long? = null,
    val lines_removed: Long? = null,
    val cache_hit_pct: Double? = null,
)

// v1.4 套餐用量（GET /usage 与 /events 的 usage 帧共用同一个 plan 对象）
/** resets_at：daemon 那边可能给 unix 秒数也可能给 ISO-8601 字串，留原样由 [parseResetsAt] 解 */
@Serializable data class PlanWindow(val used_percentage: Double? = null, val resets_at: JsonElement? = null)
@Serializable data class ModelScopedUsage(val display_name: String = "", val utilization: Double? = null, val resets_at: JsonElement? = null)
@Serializable data class PlanUsage(
    val five_hour: PlanWindow? = null,
    val seven_day: PlanWindow? = null,
    val model_scoped: List<ModelScopedUsage>? = null,
    val updated_at: JsonElement? = null,
)
@Serializable data class UsageResponse(val plan: PlanUsage? = null)

// v1.4 产物（会话里发布的 Artifact 链接）
@Serializable data class ArtifactInfo(
    val url: String,
    val title: String = "",
    val description: String = "",
    val file_path: String = "",
    val ts: String = "",
)
@Serializable data class ArtifactsResponse(val artifacts: List<ArtifactInfo> = emptyList())

// v1.17 详情屏（GET /sessions/:id/detail）：消息流里翻不出来的四样东西
@Serializable data class Subagent(
    /** 工具名：Agent（新）/ Task（老） */
    val tool: String = "",
    /** 子代理类型（subagent_type），拿不到就空 */
    val kind: String = "",
    val summary: String = "",
    /** running | ok | err */
    val status: String = "",
    val ts: String = "",
)
@Serializable data class BgTask(val tool: String = "", val summary: String = "", val ts: String = "")
@Serializable data class UploadInfo(val name: String = "", val path: String = "", val size: Long = 0, val ts: String = "")
@Serializable data class SkillUse(val name: String = "", val count: Int = 0, val last_ts: String = "")
@Serializable data class SessionDetail(
    val subagents: List<Subagent> = emptyList(),
    val background_tasks: List<BgTask> = emptyList(),
    val uploads: List<UploadInfo> = emptyList(),
    val skills: List<SkillUse> = emptyList(),
)

@Serializable data class Health(
    val version: String = "", val ssd_mounted: Boolean = false, val project_root: String = "", val uptime_s: Long = 0,
    /** 二进制被重新构建过、跑的还是旧进程：设置页亮「需重启」 */
    val update_pending: Boolean = false,
    /**
     * v1.22：**客户端唯一的兼容闸门**（PROTOCOL「版本兼容」）。老 daemon 不给这个字段 → 0，
     * 小于 [SCHEMA_PROJECT_STATUS] 时项目列表顶上挂降级横幅；不许悄悄退回自己算一套。
     */
    val schema: Int = 0,
)

/** 项目行的 `status` / `title` / `session_id` / `updated_at` 与会话的 `asking_seq` / `checklist` 从这一版起由 daemon 下发 */
const val SCHEMA_PROJECT_STATUS = 2
/**
 * 项目列表的一行。**v1.22 起这一行就是「这个项目此刻的样子」**：状态、代表会话、标题、排序时间
 * 全由 daemon 算好（PROTOCOL「版本兼容」），客户端只画。此前 mac 取 `updated_at` 最大的会话、
 * Android 先按 agent 过滤再按「待回复 < 执行中 < 其它」挑，同一个项目在两台设备上显示的标题和
 * 状态能不一样——那不是重复，是同一个问题三个答案。
 */
@Serializable data class Project(
    val path: String,
    val name: String,
    val mtime: String = "",
    val dir_size: Long = 0,
    val agent: String? = null,
    val session_title: String? = null,
    /** v1.8：置顶（daemon 侧存，三端一起变） */
    val pinned: Boolean = false,
    /** v1.22：代表这个项目的会话；没有活会话时是最近退出的那个，一个都没有 → null */
    val session_id: String? = null,
    /** v1.22：`asking|running|background|active|paused` 五态之一；没有活会话 = `paused`。老 daemon 不给 → 空串 */
    val status: String = "",
    /** v1.22：标题回退链（活会话标题 → `session_title` → 目录名）daemon 已走完；老 daemon 不给 → null */
    val title: String? = null,
    /** v1.22：排序键——该项目最新一条非终端会话的 `updated_at`（含已退出的），一个会话都没有 → 目录 mtime */
    val updated_at: String? = null,
    /**
     * v1.22：在注册表里。`false` = 在别处 `aaa open` 开出来、注册表没登记但此刻有活会话的目录，
     * daemon 补的一行——能点开它的会话，但不能 resume / 删项目。老 daemon 不给这类行 → 默认 true。
     */
    val registered: Boolean = true,
)
/** kind：permission（能替答）| elicitation（MCP 表单，只能去终端） */
@Serializable data class PermissionPrompt(val kind: String = "permission", val tool_name: String = "", val summary: String = "", val since: String = "")
@Serializable data class ScreenText(val text: String = "")
@Serializable data class PurgedAgent(val agent_label: String, val count: Int)
@Serializable data class ProjectDeleteResult(val path: String, val ok: Boolean, val purged: List<PurgedAgent> = emptyList())

// v1.1 messages
@Serializable data class ToolInfo(val name: String = "", val summary: String = "", val status: String = "")
@Serializable data class QOption(val label: String, val description: String = "")
@Serializable data class QuestionItem(
    val header: String = "",
    val question: String,
    val options: List<QOption> = emptyList(),
    val multi_select: Boolean = false,
)
@Serializable data class QuestionSpec(val questions: List<QuestionItem> = emptyList())
@Serializable data class ChatMessage(
    val seq: Long,
    val ts: String = "",
    val role: String = "",
    val kind: String = "",
    val text: String = "",
    val tool: ToolInfo? = null,
    val question: QuestionSpec? = null,
)
@Serializable data class AnswerItem(val selected: List<Int> = emptyList(), val other: String? = null)
@Serializable data class MessagesResponse(
    val supported: Boolean = false,
    val source: String = "none",
    val last_seq: Long = 0,
    val messages: List<ChatMessage> = emptyList(),
)

// v1.9 会话日志（GET /history）：所有出现过的会话，含已退出、已删除
/**
 * 看板（GET /history/dashboard，2026-09-07 第二版）：**所有会话的进度**，没有时间维度。
 * daemon 一次算好，两端只画。
 */
/** `sessions` 故意没有默认值：老 daemon（v1.11/1.12）返回的是另一种形状，缺这个字段就该解码失败报「请升级 daemon」，
 *  而不是静默画一个空看板（gpt-6 审阅指出） */
@Serializable data class Dashboard(val counts: DashCounts = DashCounts(), val sessions: List<SessionCard>)

@Serializable data class DashCounts(
    /** 未删除会话里没勾的清单项总数——看板顶上唯一还留着的数字 */
    val open_items: Int = 0,
)

/** 一张卡 = 一个会话的进度 */
@Serializable data class SessionCard(
    val id: String = "",
    val title: String = "",
    val project_name: String = "",
    val project_path: String = "",
    /** asking | running | background | active | paused */
    val status: String = "",
    /** 还在池子里（能点开，已退出的回放也算） */
    val alive: Boolean = false,
    val deleted: Boolean = false,
    val done: Int = 0,
    val open: Int = 0,
    val items: List<ChecklistItem> = emptyList(),
    val updated_at: String = "",
)

/**
 * 卡片上画不画蓝点（2026-09-08 用户拍板：看板和项目列表说同一套话——蓝点 / 黄点 / 什么都没有，
 * 五个状态字连同顶上的计数条一起去掉）。`status` 本身还留在协议里，它是排序和这个判断的依据。
 */
fun cardRunning(c: SessionCard): Boolean = !c.deleted && (c.status == "running" || c.status == "background")

/**
 * 「在 AAA 里」= 这个会话此刻**还活着**：进程在跑、或者停在输入框等你说话（2026-09-08
 * 用户拍板的看板分节口径）。`alive`（还在 daemon 池子里）**不算**——daemon 会把已经退出的
 * 会话留在池子里供回放，真实数据里 318 张卡有 164 张是这种，按 `alive` 切等于没切。真正
 * 「在 AAA 里」的就是项目列表上那几行。已退出的仍可能点得开（`alive`），那是「打开」的事。
 */
fun cardInAaa(c: SessionCard): Boolean = c.alive && c.status != "paused"

/** 看板搜索：标题 / 项目 / 任一清单项含关键字（不分大小写）；空串全匹配 */
fun cardMatches(c: SessionCard, query: String): Boolean {
    val q = query.trim().lowercase()
    return q.isEmpty() || c.title.lowercase().contains(q) || c.project_name.lowercase().contains(q) || c.items.any { it.text.lowercase().contains(q) }
}


/**
 * 排着还没送进去的一条（会话的 `queued`）。**这是 Claude Code 自己的队列**：模型在跑时
 * 往 TUI 里敲的字它自己会排队，这一轮结束再送进去；daemon 从 transcript 读出来下发，
 * 外加信任对话框弹着时它替用户收下的那几条。
 *
 * v1.22 用户拍板「排队发送按照 claude code 逻辑，不需要另外实现这个功能」——AAA 那套
 * 「发送时按状态分流去 POST /inbox」就此拆掉，客户端只画不管，也没有撤回（那是 TUI 里的事）。
 */
@Serializable data class QueuedMsg(val ts: String = "", val text: String = "")

@Serializable data class UploadResult(val saved_path: String)

object ProtocolJson { val instance = Json { ignoreUnknownKeys = true; coerceInputValues = true; explicitNulls = false } }

// ---------- pairing ----------

data class PairPayload(val name: String, val hosts: List<String>, val token: String) {
    companion object {
        fun parse(raw: String): PairPayload {
            require(raw.startsWith("aaa://pair")) { "unsupported pair URI" }
            val uri = URI(raw)
            require(uri.scheme == "aaa" && (uri.host == "pair" || uri.authority == "pair")) { "invalid pair URI" }
            val values = uri.rawQuery.orEmpty().split('&').filter { it.isNotEmpty() }.associate {
                val parts = it.split('=', limit = 2)
                // String-overload decode：Charset 重载要 API 33，minSdk 29
                URLDecoder.decode(parts[0], "UTF-8") to URLDecoder.decode(parts.getOrElse(1) { "" }, "UTF-8")
            }
            require(values["v"] == "1") { "unsupported pair version" }
            val hosts = values["hosts"].orEmpty().split(',').filter { it.isNotBlank() }
            require(hosts.isNotEmpty() && !values["token"].isNullOrBlank()) { "pair payload missing hosts or token" }
            return PairPayload(values["name"].orEmpty(), hosts, values.getValue("token"))
        }
    }
}

/** Persisted server configuration (DataStore). */
@Serializable data class ServerConfig(
    val name: String = "",
    val hosts: List<String> = emptyList(),
    val token: String = "",
    val preferredHost: String? = null,
)

// ---------- /events frames ----------

sealed class EventFrame {
    data class Snapshot(val sessions: List<Session>) : EventFrame()
    data class SessionUpdate(val session: Session) : EventFrame()
    data class SessionRemoved(val id: String) : EventFrame()
    data object ProjectsChanged : EventFrame()
    data class HealthUpdate(val ssdMounted: Boolean) : EventFrame()
    data class MessagesChanged(val id: String, val lastSeq: Long) : EventFrame()
    /** 套餐用量变了：plan 为 null 表示 daemon 暂时拿不到 */
    data class UsageUpdate(val plan: PlanUsage?) : EventFrame()
    data class Unknown(val type: String) : EventFrame()

    companion object {
        fun parse(text: String): EventFrame {
            val obj: JsonObject = try { ProtocolJson.instance.parseToJsonElement(text).jsonObject } catch (_: Exception) { return Unknown("?") }
            val t = obj["t"]?.jsonPrimitive?.contentOrNull ?: return Unknown("?")
            return try {
                when (t) {
                    "snapshot" -> Snapshot(obj["sessions"]?.let { ProtocolJson.instance.decodeFromJsonElement(kotlinx.serialization.builtins.ListSerializer(Session.serializer()), it) } ?: emptyList())
                    "session" -> SessionUpdate(ProtocolJson.instance.decodeFromJsonElement(Session.serializer(), obj["session"]!!))
                    "session_removed" -> SessionRemoved(obj["id"]!!.jsonPrimitive.content)
                    "projects_changed" -> ProjectsChanged
                    "health" -> HealthUpdate(obj["ssd_mounted"]?.jsonPrimitive?.booleanOrNull ?: true)
                    "messages_changed" -> MessagesChanged(obj["id"]!!.jsonPrimitive.content, obj["last_seq"]?.jsonPrimitive?.longOrNull ?: 0)
                    "usage" -> UsageUpdate(obj["plan"]?.takeIf { it !is kotlinx.serialization.json.JsonNull }?.let { ProtocolJson.instance.decodeFromJsonElement(PlanUsage.serializer(), it) })
                    else -> Unknown(t)
                }
            } catch (_: Exception) { Unknown(t) }
        }
    }
}

// ---------- notification filtering (client-side, PROTOCOL v1.1 通知细化) ----------

data class NotifySettings(
    val doneEnabled: Boolean = true,
    val mutedProjects: Set<String> = emptySet(),
)

object NotifyFilter {
    fun shouldNotify(projectPath: String, settings: NotifySettings): Boolean {
        if (pathListContains(settings.mutedProjects, projectPath)) return false
        return settings.doneEnabled
    }
}

/**
 * 本机按路径存的集合（黄点 / 静音）里有没有这个项目。**两边都去掉尾斜杠再比**：daemon、
 * 通知、`/projects` 三处给的同一个目录可能一个带尾斜杠一个不带，裸字符串相等会把 `/p/a` 和
 * `/p/a/` 当成两个项目——黄点打在带斜杠的那份上，进会话时按不带斜杠的那份去清，清不掉。
 * 只削尾斜杠，不做前缀匹配：`/p/b` 不是 `/p/bg`。
 */
fun pathListContains(list: Set<String>, path: String): Boolean {
    val p = path.trimEnd('/')
    return list.any { it.trimEnd('/') == p }
}
