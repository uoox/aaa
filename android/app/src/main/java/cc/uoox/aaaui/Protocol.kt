package cc.uoox.aaaui

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull
import java.net.URI
import java.net.URLDecoder

// ---------- models (PROTOCOL.md v1 + v1.1) ----------

@Serializable data class Question(val text: String, val options: List<Option> = emptyList())
@Serializable data class Option(val key: String, val label: String)

@Serializable data class Session(
    val id: String,
    val title: String = "",
    val project_path: String = "",
    val project_name: String = "",
    val agent: String = "",
    val state: String = "",
    val question: Question? = null,
    val preview: String = "",
    val rows: Int = 24,
    val cols: Int = 80,
    val pid: Int? = null,
    val exit_code: Int? = null,
    val resume_id: String? = null,
    val created_at: String = "",
    val last_output_at: String = "",
)

@Serializable data class Health(val version: String = "", val ssd_mounted: Boolean = false, val project_root: String = "", val uptime_s: Long = 0)
@Serializable data class Permission(val id: String, val label: String, val status: String)
@Serializable data class Agent(val id: String, val label: String, val available: Boolean = false)
@Serializable data class Project(
    val path: String,
    val name: String,
    val mtime: String = "",
    val dir_size: Long = 0,
    val ctx_size: Long = 0,
    val agent: String? = null,
    val session_title: String? = null,
)
@Serializable data class PortInfo(val port: Int, val cmd: String = "")
@Serializable data class PurgedAgent(val agent_label: String, val count: Int)
@Serializable data class ProjectDeleteResult(val path: String, val ok: Boolean, val purged: List<PurgedAgent> = emptyList())

// v1.1 messages
@Serializable data class ToolInfo(val name: String = "", val summary: String = "", val status: String = "")
@Serializable data class ChatMessage(
    val seq: Long,
    val ts: String = "",
    val role: String = "",
    val kind: String = "",
    val text: String = "",
    val tool: ToolInfo? = null,
)
@Serializable data class MessagesResponse(
    val supported: Boolean = false,
    val source: String = "none",
    val last_seq: Long = 0,
    val messages: List<ChatMessage> = emptyList(),
)

// v1.1 diff
@Serializable data class DiffFile(
    val path: String,
    val status: String = "modified",
    val additions: Int = 0,
    val deletions: Int = 0,
    val patch: String = "",
    val truncated: Boolean = false,
)
@Serializable data class DiffResponse(val supported: Boolean = false, val base: String = "", val files: List<DiffFile> = emptyList())

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
    data class SessionStalled(val id: String, val quietS: Long) : EventFrame()
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
                    "session_stalled" -> SessionStalled(obj["id"]!!.jsonPrimitive.content, obj["quiet_s"]?.jsonPrimitive?.longOrNull ?: 0)
                    else -> Unknown(t)
                }
            } catch (_: Exception) { Unknown(t) }
        }
    }
}

// ---------- notification filtering (client-side, PROTOCOL v1.1 通知细化) ----------

enum class NotifyKind { WAITING, EXITED, STALLED }

data class NotifySettings(
    val waitingEnabled: Boolean = true,
    val exitedEnabled: Boolean = true,
    val stalledEnabled: Boolean = true,
    val mutedProjects: Set<String> = emptySet(),
)

object NotifyFilter {
    fun shouldNotify(kind: NotifyKind, projectPath: String, settings: NotifySettings): Boolean {
        if (projectPath in settings.mutedProjects) return false
        return when (kind) {
            NotifyKind.WAITING -> settings.waitingEnabled
            NotifyKind.EXITED -> settings.exitedEnabled
            NotifyKind.STALLED -> settings.stalledEnabled
        }
    }
}

/**
 * Waiting-notification dedup: same session + same question notifies once, and a session
 * cools down for [cooldownMs] between waiting notifications (mirrors daemon-side ntfy dedup).
 */
class WaitingDeduper(private val cooldownMs: Long = 5 * 60 * 1000) {
    private data class Entry(val questionKey: String, val at: Long)
    private val last = HashMap<String, Entry>()

    fun offer(sessionId: String, questionText: String?, now: Long = System.currentTimeMillis()): Boolean {
        val key = questionText.orEmpty()
        val prev = last[sessionId]
        if (prev != null) {
            if (prev.questionKey == key) return false
            if (now - prev.at < cooldownMs) return false
        }
        last[sessionId] = Entry(key, now)
        return true
    }

    fun clear(sessionId: String) { last.remove(sessionId) }
}
