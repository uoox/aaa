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
    val preview: String = "",
    val rows: Int = 24,
    val cols: Int = 80,
    val pid: Int? = null,
    val exit_code: Int? = null,
    val resume_id: String? = null,
    val created_at: String = "",
    val last_output_at: String = "",
    /** v1.5：状态翻转 / 改名的时刻（不随每个 PTY 字节跳），首页按它排序；老 daemon 不给 → 空 */
    val updated_at: String = "",
    /** v1.3：状态由 Claude Code hooks 驱动（不再是屏幕静默猜的） */
    val hooked: Boolean = false,
    /** StopFailure 的错误类型：rate_limit / overloaded / authentication_failed… */
    val error: String? = null,
    /** 正在整理上下文（PreCompact → PostCompact） */
    val compacting: Boolean = false,
    /** 用户自己结束的：退出不弹通知 */
    val user_killed: Boolean = false,
    /** v1.4：Claude Code statusline 喂来的用量（模型 / 上下文占比 / 花费）；没有就 null */
    val usage: SessionUsage? = null,
    /** v1.7：整个对话的进度清单（`- [x] 已做` / `- [ ] 未做` 的 markdown），每轮结束后 daemon 重写 */
    val summary: String = "",
)

/** 进度清单的一项（[parseChecklist] 解析 `summary` 的一行） */
@Serializable data class ChecklistItem(val done: Boolean, val text: String)

/** `- [x] …` / `- [ ] …` 行 → 项；其它行忽略 */
fun parseChecklist(md: String): List<ChecklistItem> = md.lines().mapNotNull { raw ->
    val l = raw.trim().trimStart('-', '*').trimStart()
    val (done, rest) = when {
        l.startsWith("[x]") || l.startsWith("[X]") -> true to l.substring(3)
        l.startsWith("[ ]") -> false to l.substring(3)
        else -> return@mapNotNull null
    }
    rest.trim().takeIf { it.isNotEmpty() }?.let { ChecklistItem(done, it) }
}

@Serializable data class SessionUsage(
    val model: String? = null,
    val model_id: String? = null,
    /** 0-100 */
    val context_pct: Double? = null,
    val context_window_size: Long? = null,
    val input_tokens: Long? = null,
    val output_tokens: Long? = null,
    val cost_usd: Double? = null,
    val duration_ms: Long? = null,
    val lines_added: Long? = null,
    val lines_removed: Long? = null,
    val effort: String? = null,
    /** v1.8：提示缓存——最近一次调用里从缓存读 / 新写进缓存 / 新读的 token，与命中率（0–100） */
    val cache_read_tokens: Long? = null,
    val cache_creation_tokens: Long? = null,
    val fresh_input_tokens: Long? = null,
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

@Serializable data class Health(
    val version: String = "", val ssd_mounted: Boolean = false, val project_root: String = "", val uptime_s: Long = 0,
    /** 二进制被重新构建过、跑的还是旧进程：设置页亮「需重启」 */
    val update_pending: Boolean = false,
)
@Serializable data class Agent(val id: String, val label: String, val available: Boolean = false, val terminal: Boolean = false)
@Serializable data class Project(
    val path: String,
    val name: String,
    val mtime: String = "",
    val dir_size: Long = 0,
    val ctx_size: Long = 0,
    val agent: String? = null,
    val session_title: String? = null,
    /** v1.8：置顶（daemon 侧存，三端一起变） */
    val pinned: Boolean = false,
    /** v1.15：归档（daemon 侧存）——列表 / 看板默认藏起来，一个开关翻出来 */
    val archived: Boolean = false,
)
@Serializable data class PermissionPrompt(val tool_name: String = "", val summary: String = "", val since: String = "")
@Serializable data class PortInfo(val port: Int, val cmd: String = "")
@Serializable data class ScreenText(val text: String = "", val alternate_screen: Boolean = false)
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
    val asking: Int = 0, val running: Int = 0, val background: Int = 0, val active: Int = 0, val paused: Int = 0,
    /** 未删除会话里没勾的清单项总数 */
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
    /** v1.15：项目已归档 */
    val archived: Boolean = false,
    val done: Int = 0,
    val open: Int = 0,
    val items: List<ChecklistItem> = emptyList(),
    val updated_at: String = "",
)

val DASH_STATUSES = listOf("asking", "running", "background", "active", "paused")

/** 状态字（与首页同一套五态） */
fun statusLabel(status: String): String = when (status) {
    "asking" -> "待回复"; "running" -> "运行"; "background" -> "后台"; "active" -> "激活"; else -> "暂停"
}

/** 看板搜索：标题 / 项目 / 任一清单项含关键字（不分大小写）；空串全匹配 */
fun cardMatches(c: SessionCard, query: String): Boolean {
    val q = query.trim().lowercase()
    return q.isEmpty() || c.title.lowercase().contains(q) || c.project_name.lowercase().contains(q) || c.items.any { it.text.lowercase().contains(q) }
}

/** 「已完成」= 暂停且清单全勾完（或没清单）：真正结束的活儿，看板默认收起来 */
fun cardIsFinished(c: SessionCard): Boolean = c.status == "paused" && c.open == 0

// v1.1 inbox
@Serializable data class InboxItem(val id: String, val text: String, val created_at: String = "")

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
    data class InboxChanged(val path: String) : EventFrame()
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
                    "inbox_changed" -> InboxChanged(obj["path"]?.jsonPrimitive?.contentOrNull ?: "")
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
        if (projectPath in settings.mutedProjects) return false
        return settings.doneEnabled
    }
}
