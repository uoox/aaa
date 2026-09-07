package cc.uoox.aaaui

import android.content.Context
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer

private val DraftMap = MapSerializer(String.serializer(), String.serializer())

private val Context.dataStore by preferencesDataStore(name = "aaa_settings")

data class AppSettings(
    val server: ServerConfig? = null,
    val notifyDone: Boolean = true,
    val serviceEnabled: Boolean = false,
    val defaultUi: String = "messages", // "messages" | "terminal"
    val mutedProjects: Set<String> = emptySet(),
    /** 界面主题：dark | light | claude（见 Palette）。 */
    val theme: String = "dark",
    /** 最近打开的会话 id：没有首页了，app 起来直接回到它 */
    val lastSession: String? = null,
) {
    val notifySettings: NotifySettings
        get() = NotifySettings(notifyDone, mutedProjects)
}

class SettingsStore(private val context: Context) {
    private object K {
        val SERVER = stringPreferencesKey("server_json")
        val NOTIFY_DONE = booleanPreferencesKey("notify_done")
        val SERVICE_ENABLED = booleanPreferencesKey("service_enabled")
        val DEFAULT_UI = stringPreferencesKey("default_ui")
        val MUTED_PROJECTS = stringSetPreferencesKey("muted_projects")
        val THEME = stringPreferencesKey("theme")
        /** 会话 id → 输入框草稿（JSON 对象）。切出去 / 被系统杀掉再回来，字还在。 */
        val DRAFTS = stringPreferencesKey("drafts_json")
        val LAST_SESSION = stringPreferencesKey("last_session")
        /** 上次见到的 daemon 项目根：变了就把静音路径的前缀跟着改 */
        val PROJECT_ROOT = stringPreferencesKey("project_root")
    }

    private val json = ProtocolJson.instance

    val flow: Flow<AppSettings> = context.dataStore.data.map { p ->
        AppSettings(
            server = p[K.SERVER]?.let { runCatching { json.decodeFromString(ServerConfig.serializer(), it) }.getOrNull() },
            notifyDone = p[K.NOTIFY_DONE] ?: true,
            serviceEnabled = p[K.SERVICE_ENABLED] ?: false,
            defaultUi = p[K.DEFAULT_UI] ?: "messages",
            mutedProjects = p[K.MUTED_PROJECTS] ?: emptySet(),
            theme = p[K.THEME] ?: "dark",
            lastSession = p[K.LAST_SESSION]?.takeIf { it.isNotBlank() },
        )
    }

    suspend fun current(): AppSettings = flow.first()

    suspend fun setServer(server: ServerConfig?) = context.dataStore.edit { p ->
        if (server == null) p.remove(K.SERVER)
        else p[K.SERVER] = json.encodeToString(ServerConfig.serializer(), server)
    }

    suspend fun setPreferredHost(host: String) {
        val cur = current().server ?: return
        if (cur.preferredHost != host) setServer(cur.copy(preferredHost = host))
    }

    suspend fun setNotifyDone(v: Boolean) = context.dataStore.edit { it[K.NOTIFY_DONE] = v }
    suspend fun setServiceEnabled(v: Boolean) = context.dataStore.edit { it[K.SERVICE_ENABLED] = v }
    suspend fun setDefaultUi(v: String) = context.dataStore.edit { it[K.DEFAULT_UI] = v }
    suspend fun setLastSession(id: String?) = context.dataStore.edit { p -> if (id.isNullOrBlank()) p.remove(K.LAST_SESSION) else p[K.LAST_SESSION] = id }
    /** 只存认识的名字：不认识的落回黑暗，读的那头就不用再兜底。 */
    suspend fun setTheme(v: String) = context.dataStore.edit { it[K.THEME] = Palette.forName(v).name }

    /** 全部草稿；坏数据当空处理 */
    suspend fun drafts(): Map<String, String> {
        val raw = context.dataStore.data.first()[K.DRAFTS] ?: return emptyMap()
        return runCatching { json.decodeFromString(DraftMap, raw) }.getOrDefault(emptyMap())
    }

    suspend fun setDrafts(drafts: Map<String, String>) = context.dataStore.edit { p ->
        if (drafts.isEmpty()) p.remove(K.DRAFTS)
        else p[K.DRAFTS] = json.encodeToString(DraftMap, drafts)
    }

    /**
     * daemon 报的项目根变了（迁根）：静音项目按路径存，前缀跟着换，用户不用重新点。
     * 第一次见到的根只记下来。返回是否改写过。
     */
    suspend fun noteProjectRoot(root: String): Boolean {
        if (root.isBlank()) return false
        var changed = false
        context.dataStore.edit { p ->
            val old = p[K.PROJECT_ROOT]
            if (old != null && old != root) {
                val cur = p[K.MUTED_PROJECTS] ?: emptySet()
                val next = cur.map { rerootPath(it, old, root) }.toSet()
                if (next != cur) { p[K.MUTED_PROJECTS] = next; changed = true }
            }
            p[K.PROJECT_ROOT] = root
        }
        return changed
    }

    suspend fun setProjectMuted(path: String, muted: Boolean) = context.dataStore.edit { p ->
        val cur = p[K.MUTED_PROJECTS] ?: emptySet()
        p[K.MUTED_PROJECTS] = if (muted) cur + path else cur - path
    }
}

/** `p` 在旧根下 → 换成新根下的同一相对路径；不在 → 原样（与 daemon migrate::reroot 同口径） */
fun rerootPath(p: String, oldRoot: String, newRoot: String): String {
    val o = oldRoot.trimEnd('/'); val n = newRoot.trimEnd('/')
    return when {
        p == o -> n
        p.startsWith("$o/") -> n + p.substring(o.length)
        else -> p
    }
}
