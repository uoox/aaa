package cc.uoox.aaaui

import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import kotlinx.coroutines.launch

// ============================================================
// 通知：渠道分级（等待输入/空转 = high，完成 = default）、按项目静音、
// RemoteInput 内联回复、question 选项按钮、点按深链到会话。
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
    const val CH_WAITING = "waiting"
    const val CH_DONE = "done"
    const val CH_STALLED = "stalled"
    const val CH_SERVICE = "service"
    const val KEY_REMOTE_INPUT = "aaa_reply"

    @Volatile private var started = false
    private val deduper = WaitingDeduper()

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
        nm.createNotificationChannel(NotificationChannel(CH_WAITING, "等待输入", NotificationManager.IMPORTANCE_HIGH).apply { description = "agent 等待输入时推送" })
        nm.createNotificationChannel(NotificationChannel(CH_DONE, "任务完成", NotificationManager.IMPORTANCE_DEFAULT).apply { description = "会话退出时推送" })
        nm.createNotificationChannel(NotificationChannel(CH_STALLED, "疑似空转", NotificationManager.IMPORTANCE_HIGH).apply { description = "运行中长时间无输出" })
        nm.createNotificationChannel(NotificationChannel(CH_SERVICE, "后台连接", NotificationManager.IMPORTANCE_MIN).apply { description = "维持与 daemon 的连接" })
    }

    private suspend fun handle(context: Context, store: AppStore, ev: NotifyEvent) {
        val settings = store.settings.current().notifySettings
        when (ev) {
            is NotifyEvent.Waiting -> {
                val s = ev.session
                if (!NotifyFilter.shouldNotify(NotifyKind.WAITING, s.project_path, settings)) return
                if (!deduper.offer(s.id, s.question?.text)) return
                notifyWaiting(context, s)
            }
            is NotifyEvent.Exited -> {
                val s = ev.session
                deduper.clear(s.id)
                if (!NotifyFilter.shouldNotify(NotifyKind.EXITED, s.project_path, settings)) return
                notifySimple(
                    context, CH_DONE, s.id,
                    "✓ 会话结束 · ${s.project_name}",
                    s.title.ifBlank { s.project_name } + (s.exit_code?.let { if (it != 0) " · exit $it" else "" } ?: ""),
                )
            }
            is NotifyEvent.Stalled -> {
                val path = ev.session?.project_path.orEmpty()
                if (!NotifyFilter.shouldNotify(NotifyKind.STALLED, path, settings)) return
                notifySimple(
                    context, CH_STALLED, ev.id,
                    "⏳ 可能空转 · ${ev.session?.project_name ?: ev.id}",
                    "${ev.session?.title ?: ev.id} 已静默 ${ev.quietS / 60} 分钟",
                )
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

    private fun notifyWaiting(context: Context, s: Session) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val builder = Notification.Builder(context, CH_WAITING)
            .setSmallIcon(R.drawable.ic_stat_aaa)
            .setContentTitle("⚡ ${Tok.agentLabel(s.agent)} 等待输入 · ${s.project_name}")
            .setContentText(s.question?.text ?: s.title.ifBlank { s.project_name })
            .setStyle(Notification.BigTextStyle().bigText(s.question?.text ?: s.preview.take(300)))
            .setContentIntent(openSessionIntent(context, s.id))
            .setAutoCancel(true)

        // 内联回复
        val replyIntent = Intent(context, ReplyReceiver::class.java)
            .setAction(ReplyReceiver.ACTION_REPLY)
            .putExtra(ReplyReceiver.EXTRA_SESSION, s.id)
        val replyPending = PendingIntent.getBroadcast(
            context, ("reply:" + s.id).hashCode(), replyIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
        )
        builder.addAction(
            Notification.Action.Builder(null, "回复", replyPending)
                .addRemoteInput(RemoteInput.Builder(KEY_REMOTE_INPUT).setLabel("输入回复…").build())
                .build(),
        )

        // 前两个 question 选项作为快捷按钮
        s.question?.options?.take(2)?.forEach { opt ->
            val optIntent = Intent(context, ReplyReceiver::class.java)
                .setAction(ReplyReceiver.ACTION_OPTION)
                .putExtra(ReplyReceiver.EXTRA_SESSION, s.id)
                .putExtra(ReplyReceiver.EXTRA_TEXT, opt.key)
            val optPending = PendingIntent.getBroadcast(
                context, ("opt:${s.id}:${opt.key}").hashCode(), optIntent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            builder.addAction(Notification.Action.Builder(null, "${opt.key} · ${opt.label}", optPending).build())
        }

        try { nm.notify(s.id.hashCode(), builder.build()) } catch (_: SecurityException) { }
    }

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

/** 通知内联回复 / 选项按钮 → POST /sessions/:id/input */
class ReplyReceiver : BroadcastReceiver() {
    companion object {
        const val ACTION_REPLY = "cc.uoox.aaaui.REPLY"
        const val ACTION_OPTION = "cc.uoox.aaaui.OPTION"
        const val EXTRA_SESSION = "session_id"
        const val EXTRA_TEXT = "text"
    }

    override fun onReceive(context: Context, intent: Intent) {
        val sessionId = intent.getStringExtra(EXTRA_SESSION) ?: return
        val text = when (intent.action) {
            ACTION_REPLY -> RemoteInput.getResultsFromIntent(intent)?.getCharSequence(Notifier.KEY_REMOTE_INPUT)?.toString()
            ACTION_OPTION -> intent.getStringExtra(EXTRA_TEXT)
            else -> null
        } ?: return
        // enter 启发式（与会话屏一致）：纯数字选项键由 TUI 菜单直接消费，不补回车
        val isOptionKey = intent.action == ACTION_OPTION && text.all { it.isDigit() }
        val store = AppStore.get(context)
        store.ensureStarted()
        val pending = goAsync()
        store.scope.launch {
            try {
                store.client?.input(sessionId, text, enter = !isOptionKey)
                Notifier.cancelFor(context, sessionId)
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
