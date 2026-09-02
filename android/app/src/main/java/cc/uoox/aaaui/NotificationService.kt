package cc.uoox.aaaui

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import kotlinx.coroutines.launch

// ============================================================
// 通知：任务完成、按项目静音、点按深链到会话。
// 前台服务只负责保活 events WS（进程内的 AppStore 单例）。
// ============================================================

class AaaApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        val store = AppStore.get(this)
        store.ensureStarted()
        Notifier.ensureStarted(this, store)
        store.scope.launch {
            if (store.settings.current().serviceEnabled) {
                try { NotificationService.start(this@AaaApplication) } catch (_: Exception) { }
            }
        }
    }
}

object Notifier {
    const val CH_DONE = "done"
    const val CH_SERVICE = "service"

    @Volatile private var started = false

    fun ensureStarted(context: Context, store: AppStore) {
        if (started) return
        synchronized(this) {
            if (started) return
            started = true
            createChannels(context)
            store.scope.launch {
                store.notifyEvents.collect { ev -> handle(context.applicationContext, store, ev) }
            }
        }
    }

    private fun createChannels(context: Context) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(NotificationChannel(CH_DONE, "任务完成", NotificationManager.IMPORTANCE_DEFAULT).apply { description = "会话完成或结束时通知" })
        nm.createNotificationChannel(NotificationChannel(CH_SERVICE, "后台连接", NotificationManager.IMPORTANCE_MIN).apply { description = "维持与 daemon 的连接" })
    }

    private suspend fun handle(context: Context, store: AppStore, ev: NotifyEvent) {
        val settings = store.settings.current().notifySettings
        when (ev) {
            is NotifyEvent.Done -> {
                val session = ev.session
                if (!NotifyFilter.shouldNotify(session.project_path, settings)) return
                val title = if (ev.exited) {
                    "✓ 会话结束 · "+session.project_name + (session.exit_code?.let { if (it != 0) " · exit $it" else "" } ?: "")
                } else {
                    "✓ 完成 · "+session.project_name
                }
                val text = title.ifBlank { session.project_name }
                notifySimple(context, CH_DONE, session.id, title, text)
            }
        }
    }

    private fun openSessionIntent(context: Context, sessionId: String): PendingIntent =
        PendingIntent.getActivity(
            context, sessionId.hashCode(),
            Intent(context, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_SESSION_ID, sessionId)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun notifySimple(context: Context, channel: String, sessionId: String, title: String, text: String) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val n = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_aaa)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openSessionIntent(context, sessionId))
            .setAutoCancel(true)
            .build()
        try { nm.notify(sessionId.hashCode(), n) } catch (_: SecurityException) { }
    }

    fun cancelFor(context: Context, sessionId: String) {
        (context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancel(sessionId.hashCode())
    }
}

/** 前台服务：仅为在后台保住进程与 events WS。 */
class NotificationService : Service() {
    companion object {
        fun start(context: Context) {
            val intent = Intent(context, NotificationService::class.java)
            if (Build.VERSION.SDK_INT >= 26) context.startForegroundService(intent) else context.startService(intent)
        }
        fun stop(context: Context) {
            context.stopService(Intent(context, NotificationService::class.java))
        }
    }

    override fun onCreate() {
        super.onCreate()
        AppStore.get(this).ensureStarted()
        val n = Notification.Builder(this, Notifier.CH_SERVICE)
            .setSmallIcon(R.drawable.ic_stat_aaa)
            .setContentTitle("aaa-ui 保持连接")
            .setContentText("正在维持与 aaa-daemon 的事件连接")
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 34) {
            startForeground(1001, n, android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(1001, n)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onBind(intent: Intent?): IBinder? = null
}

