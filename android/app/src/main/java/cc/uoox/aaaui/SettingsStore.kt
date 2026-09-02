package cc.uoox.aaaui

import android.content.Context
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.intPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.dataStore by preferencesDataStore(name = "aaa_settings")

data class AppSettings(
    val server: ServerConfig? = null,
    val notifyWaiting: Boolean = true,
    val notifyExited: Boolean = true,
    val notifyStalled: Boolean = true,
    val serviceEnabled: Boolean = false,
    val defaultUi: String = "messages", // "messages" | "terminal"
    val fontSize: Int = 14,
    /** 终端视图下收起预输入框，直接在 shell 里打字；全局记住用户的选择。 */
    val terminalComposerHidden: Boolean = false,
    val mutedProjects: Set<String> = emptySet(),
) {
    val notifySettings: NotifySettings
        get() = NotifySettings(notifyWaiting, notifyExited, notifyStalled, mutedProjects)
}

class SettingsStore(private val context: Context) {
    private object K {
        val SERVER = stringPreferencesKey("server_json")
        val NOTIFY_WAITING = booleanPreferencesKey("notify_waiting")
        val NOTIFY_EXITED = booleanPreferencesKey("notify_exited")
        val NOTIFY_STALLED = booleanPreferencesKey("notify_stalled")
        val SERVICE_ENABLED = booleanPreferencesKey("service_enabled")
        val DEFAULT_UI = stringPreferencesKey("default_ui")
        val FONT_SIZE = intPreferencesKey("font_size")
        val TERMINAL_COMPOSER_HIDDEN = booleanPreferencesKey("terminal_composer_hidden")
        val MUTED_PROJECTS = stringSetPreferencesKey("muted_projects")
    }

    private val json = ProtocolJson.instance

    val flow: Flow<AppSettings> = context.dataStore.data.map { p ->
        AppSettings(
            server = p[K.SERVER]?.let { runCatching { json.decodeFromString(ServerConfig.serializer(), it) }.getOrNull() },
            notifyWaiting = p[K.NOTIFY_WAITING] ?: true,
            notifyExited = p[K.NOTIFY_EXITED] ?: true,
            notifyStalled = p[K.NOTIFY_STALLED] ?: true,
            serviceEnabled = p[K.SERVICE_ENABLED] ?: false,
            defaultUi = p[K.DEFAULT_UI] ?: "messages",
            fontSize = p[K.FONT_SIZE] ?: 14,
            terminalComposerHidden = p[K.TERMINAL_COMPOSER_HIDDEN] ?: false,
            mutedProjects = p[K.MUTED_PROJECTS] ?: emptySet(),
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

    suspend fun setNotifyWaiting(v: Boolean) = context.dataStore.edit { it[K.NOTIFY_WAITING] = v }
    suspend fun setNotifyExited(v: Boolean) = context.dataStore.edit { it[K.NOTIFY_EXITED] = v }
    suspend fun setNotifyStalled(v: Boolean) = context.dataStore.edit { it[K.NOTIFY_STALLED] = v }
    suspend fun setServiceEnabled(v: Boolean) = context.dataStore.edit { it[K.SERVICE_ENABLED] = v }
    suspend fun setDefaultUi(v: String) = context.dataStore.edit { it[K.DEFAULT_UI] = v }
    suspend fun setFontSize(v: Int) = context.dataStore.edit { it[K.FONT_SIZE] = v.coerceIn(8, 28) }
    suspend fun setTerminalComposerHidden(v: Boolean) = context.dataStore.edit { it[K.TERMINAL_COMPOSER_HIDDEN] = v }

    suspend fun setProjectMuted(path: String, muted: Boolean) = context.dataStore.edit { p ->
        val cur = p[K.MUTED_PROJECTS] ?: emptySet()
        p[K.MUTED_PROJECTS] = if (muted) cur + path else cur - path
    }
}
