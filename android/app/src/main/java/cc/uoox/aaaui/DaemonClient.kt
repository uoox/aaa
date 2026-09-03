package cc.uoox.aaaui

import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import okhttp3.Call
import okhttp3.Callback
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

class DaemonHttpException(val code: Int, val errorCode: String, message: String) : IOException("HTTP $code $errorCode: $message")

/** Typed REST + WS client for one daemon base URL (`http://host:port`). */
class DaemonClient(
    val baseUrl: String,
    private val token: String,
    val http: OkHttpClient = defaultHttp(),
) {
    companion object {
        val JSON_MEDIA = "application/json; charset=utf-8".toMediaType()
        val OCTET_MEDIA = "application/octet-stream".toMediaType()
        private val shared: OkHttpClient by lazy {
            OkHttpClient.Builder()
                .connectTimeout(6, TimeUnit.SECONDS)
                .readTimeout(30, TimeUnit.SECONDS)
                .pingInterval(20, TimeUnit.SECONDS)
                .build()
        }
        fun defaultHttp(): OkHttpClient = shared
        fun normalizeBase(hostPort: String): String {
            val trimmed = hostPort.trim().removeSuffix("/")
            return if (trimmed.startsWith("http://") || trimmed.startsWith("https://")) trimmed else "http://$trimmed"
        }
    }

    private val json = ProtocolJson.instance

    private fun builder(path: String): Request.Builder =
        Request.Builder().url(baseUrl.trimEnd('/') + "/api/v1" + path).header("Authorization", "Bearer $token")

    private suspend fun execute(request: Request): String = suspendCancellableCoroutine { cont ->
        val call = http.newCall(request)
        cont.invokeOnCancellation { call.cancel() }
        call.enqueue(object : Callback {
            override fun onFailure(call: Call, e: IOException) { if (cont.isActive) cont.resumeWithException(e) }
            override fun onResponse(call: Call, response: Response) {
                response.use {
                    val body = it.body?.string().orEmpty()
                    if (it.isSuccessful || it.code == 202) { if (cont.isActive) cont.resume(body) }
                    else {
                        var code = "internal"; var msg = body
                        try {
                            val err = json.parseToJsonElement(body).jsonObject["error"]?.jsonObject
                            code = err?.get("code")?.jsonPrimitive?.contentOrNull ?: code
                            msg = err?.get("message")?.jsonPrimitive?.contentOrNull ?: msg
                        } catch (_: Exception) { }
                        if (cont.isActive) cont.resumeWithException(DaemonHttpException(it.code, code, msg))
                    }
                }
            }
        })
    }

    private suspend fun get(path: String): String = execute(builder(path).build())
    private suspend fun post(path: String, body: String = "{}"): String =
        execute(builder(path).post(body.toRequestBody(JSON_MEDIA)).build())
    private suspend fun delete(path: String): String = execute(builder(path).delete().build())

    // ---------- v1 ----------

    suspend fun health(): Health = json.decodeFromString(get("/health"))
    /** `POST /restart`：force = 连存活会话一起终止。有存活会话且不 force → 409 */
    suspend fun restartDaemon(force: Boolean): String = post("/restart", """{"force":$force}""")
    suspend fun agents(): List<Agent> = json.decodeFromString(ListSerializer(Agent.serializer()), get("/agents"))
    suspend fun projects(): List<Project> = json.decodeFromString(ListSerializer(Project.serializer()), get("/projects"))

    suspend fun createProject(name: String?, agent: String?): Project {
        val body = buildJsonObject { if (!name.isNullOrBlank()) put("name", name); if (!agent.isNullOrBlank()) put("agent", agent) }
        return json.decodeFromString(post("/projects", body.toString()))
    }

    suspend fun deleteProjects(paths: List<String>): List<ProjectDeleteResult> {
        val body = buildJsonObject { put("paths", kotlinx.serialization.json.JsonArray(paths.map { kotlinx.serialization.json.JsonPrimitive(it) })) }
        val resp = json.parseToJsonElement(post("/projects/delete", body.toString())).jsonObject
        return json.decodeFromJsonElement(ListSerializer(ProjectDeleteResult.serializer()), resp["results"]!!)
    }

    suspend fun sessions(): List<Session> = json.decodeFromString(ListSerializer(Session.serializer()), get("/sessions"))

    suspend fun createSession(projectPath: String, agent: String, resume: Boolean, feedInbox: Boolean = true, fresh: Boolean = false): Session {
        val body = buildJsonObject {
            put("project_path", projectPath); put("agent", agent); put("resume", resume)
            if (!feedInbox) put("feed_inbox", false)
            if (fresh) put("fresh", true)
        }
        return json.decodeFromString(post("/sessions", body.toString()))
    }

    suspend fun input(id: String, text: String, enter: Boolean) {
        post("/sessions/$id/input", buildJsonObject { put("text", text); put("enter", enter) }.toString())
    }

    suspend fun kill(id: String) { post("/sessions/$id/kill") }
    suspend fun deleteSession(id: String) { delete("/sessions/$id") }
    suspend fun rename(id: String, title: String) { post("/sessions/$id/rename", buildJsonObject { put("title", title) }.toString()) }
    suspend fun ports(id: String): List<PortInfo> = json.decodeFromString(ListSerializer(PortInfo.serializer()), get("/sessions/$id/ports"))
    /** daemon 侧 vt100 的整屏文本（非备用屏时带回滚尾巴）：复制屏幕、抓链接用。 */
    suspend fun screen(id: String): ScreenText = json.decodeFromString(ScreenText.serializer(), get("/sessions/$id/screen"))

    // ---------- v1.1 ----------

    suspend fun messages(id: String, after: Long = 0, limit: Int = 200): MessagesResponse =
        json.decodeFromString(get("/sessions/$id/messages?after=$after&limit=$limit"))

    suspend fun answer(sessionId: String, answers: List<AnswerItem>) {
        val body = buildJsonObject {
            put("answers", kotlinx.serialization.json.JsonArray(answers.map { a ->
                buildJsonObject {
                    put("selected", kotlinx.serialization.json.JsonArray(a.selected.map { kotlinx.serialization.json.JsonPrimitive(it) }))
                    put("other", a.other?.let { kotlinx.serialization.json.JsonPrimitive(it) } ?: kotlinx.serialization.json.JsonNull)
                }
            }))
        }
        post("/sessions/$sessionId/answer", body.toString())
    }

    suspend fun diff(id: String): DiffResponse = json.decodeFromString(get("/sessions/$id/diff"))

    suspend fun rollback(id: String, force: Boolean) {
        post("/sessions/$id/rollback", buildJsonObject { put("confirm", true); put("force", force) }.toString())
    }

    suspend fun inbox(path: String): List<InboxItem> =
        json.decodeFromString(ListSerializer(InboxItem.serializer()), get("/inbox?path=" + urlEncode(path)))

    suspend fun inboxAdd(path: String, text: String) {
        post("/inbox", buildJsonObject { put("path", path); put("text", text) }.toString())
    }

    suspend fun inboxDelete(id: String) { delete("/inbox/$id") }

    // ---------- v1.4 ----------

    /** 套餐用量；plan 为 null = daemon 暂时没有数据 */
    suspend fun usage(): PlanUsage? = json.decodeFromString(UsageResponse.serializer(), get("/usage")).plan

    suspend fun artifacts(id: String): List<ArtifactInfo> =
        json.decodeFromString(ArtifactsResponse.serializer(), get("/sessions/$id/artifacts")).artifacts

    suspend fun upload(projectPath: String, name: String, bytes: ByteArray): UploadResult {
        val req = builder("/projects/upload?path=" + urlEncode(projectPath) + "&name=" + urlEncode(name))
            .post(bytes.toRequestBody(OCTET_MEDIA)).build()
        return json.decodeFromString(execute(req))
    }

    // ---------- WS ----------

    fun attachSocket(id: String, listener: WebSocketListener): WebSocket =
        http.newWebSocket(builder("/sessions/$id/attach").build(), listener)

    fun eventsSocket(listener: WebSocketListener): WebSocket =
        http.newWebSocket(builder("/events").build(), listener)

    private fun urlEncode(s: String): String = java.net.URLEncoder.encode(s, "UTF-8")
}
