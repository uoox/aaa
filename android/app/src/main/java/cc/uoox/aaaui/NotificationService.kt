package cc.uoox.aaaui

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import kotlinx.coroutines.launch

// ============================================================
// 通知：任务完成 / 待回复 / 出错，点按深链到会话。总开关在设置页。
// 待回复且是**权限请求**时，横幅上直接给「允许 / 拒绝」——不然为了放行一句
// `cargo test` 要解锁、点通知、翻列表、进会话、滚到底（v1.27）。
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
        nm.createNotificationChannel(NotificationChannel(CH_DONE, "会话", NotificationManager.IMPORTANCE_DEFAULT).apply { description = "待回复 / 运行结束 / 出错" })
        nm.createNotificationChannel(NotificationChannel(CH_SERVICE, "后台连接", NotificationManager.IMPORTANCE_MIN).apply { description = "维持与 daemon 的连接" })
    }

    private suspend fun handle(context: Context, store: AppStore, ev: NotifyEvent) {
        val notify = store.settings.current().notifyDone
        when (ev) {
            is NotifyEvent.Done -> {
                val session = ev.session
                if (!notify) return
                val title = if (ev.exited) {
                    "✗ 出错 · " + session.project_name + " · 退出码 ${session.exit_code ?: 0}"
                } else {
                    "✓ 运行结束 · " + session.project_name
                }
                notifySimple(context, CH_DONE, session.id, title, session.title.ifBlank { session.project_name })
            }
            is NotifyEvent.Asking -> {
                val session = ev.session
                if (!notify) return
                val p = session.permission
                // 权限请求能在横幅上直接答；结构化提问（AskUserQuestion）只能进会话，不给按钮
                val text = when {
                    p == null -> session.title.ifBlank { "弹着选项等你选" }
                    p.tool_name.isBlank() -> p.summary
                    else -> "${p.tool_name}：${p.summary}"
                }
                notifySimple(context, CH_DONE, session.id, "? 待回复 · " + session.project_name, text, decidable = p != null)
            }
            is NotifyEvent.Error -> {
                val session = ev.session
                if (!notify) return
                notifySimple(context, CH_DONE, session.id, "✗ 出错 · " + session.project_name, ev.error)
            }
        }
    }

    private fun openSessionIntent(context: Context, sessionId: String): PendingIntent =
        PendingIntent.getActivity(
            context, sessionId.hashCode(),
            Intent(context, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_SESSION_ID, sessionId)
                // **通知一律落终端**（2026-09-11 用户拍板）：三种通知里最要紧的那种是
                // 「它在等你答」，而 Claude Code 的结构化提问在消息流里替答不可靠
                // （daemon 是在盲操一个会变的 TUI，见 PROTOCOL「回答表单」）。
                // 终端里那个对话框是它自己画的，你按什么就是什么。
                .putExtra(MainActivity.EXTRA_VIEW, "terminal")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    /** 横幅上的「允许 / 拒绝」：广播回本进程，由 [PermissionActionReceiver] 发出去 */
    private fun decideIntent(context: Context, sessionId: String, behavior: String): PendingIntent =
        PendingIntent.getBroadcast(
            context, (sessionId + behavior).hashCode(),
            Intent(context, PermissionActionReceiver::class.java)
                .setAction("cc.uoox.aaaui.PERMISSION")
                .putExtra(PermissionActionReceiver.EXTRA_SESSION, sessionId)
                .putExtra(PermissionActionReceiver.EXTRA_BEHAVIOR, behavior),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun notifySimple(
        context: Context,
        channel: String,
        sessionId: String,
        title: String,
        text: String,
        decidable: Boolean = false,
    ) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val b = Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.ic_stat_aaa)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openSessionIntent(context, sessionId))
            .setAutoCancel(true)
        if (decidable) {
            b.addAction(Notification.Action.Builder(null, "允许", decideIntent(context, sessionId, "allow")).build())
            b.addAction(Notification.Action.Builder(null, "拒绝", decideIntent(context, sessionId, "deny")).build())
        }
        try { nm.notify(sessionId.hashCode(), b.build()) } catch (_: SecurityException) { }
    }
}

/**
 * 横幅上按了「允许 / 拒绝」：直接 `POST /sessions/:id/permission`，不开界面。
 * `goAsync()` 把进程多留一会儿，够发一次请求；答完把这条通知撤掉——它已经没意义了。
 * 失败不重试也不弹错：会话还卡在那儿，下一次事件会把同一条通知再推出来。
 */
class PermissionActionReceiver : BroadcastReceiver() {
    companion object {
        const val EXTRA_SESSION = "session"
        const val EXTRA_BEHAVIOR = "behavior"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(EXTRA_SESSION) ?: return
        val behavior = intent.getStringExtra(EXTRA_BEHAVIOR) ?: return
        val app = context.applicationContext
        val pending = goAsync()
        val store = AppStore.get(app)
        store.scope.launch {
            try {
                store.client?.permission(id, behavior)
                store.refreshSessions()
                (app.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager).cancel(id.hashCode())
            } catch (_: Exception) {
            } finally {
                pending.finish()
            }
        }
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

