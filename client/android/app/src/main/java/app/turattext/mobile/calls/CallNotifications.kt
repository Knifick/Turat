package app.turattext.mobile.calls

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Person
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.drawable.Icon
import android.os.Build
import android.util.Base64
import app.turattext.mobile.MainActivity
import app.turattext.mobile.R

/**
 * Уведомления звонка: входящий вызов поверх экрана блокировки и строка идущего разговора,
 * которую держит [CallService].
 */
internal object CallNotifications {
    const val IncomingId = 7101
    const val OngoingId = 7102
    private const val IncomingChannel = "calls-incoming"
    private const val OngoingChannel = "calls-ongoing"

    /** Открыть приложение на экране звонка; `accept` — сразу ответить. */
    const val ActionAccept = "app.turattext.mobile.CALL_ACCEPT"
    const val ActionShow = "app.turattext.mobile.CALL_SHOW"

    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java)
        // Мелодию и вибрацию входящего играет само приложение: так их можно остановить ровно
        // в момент ответа, а повтор не зависит от прихоти оболочки.
        manager.createNotificationChannel(
            NotificationChannel(IncomingChannel, "Входящие звонки", NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Вызов поверх экрана блокировки"
                setSound(null, null)
                enableVibration(false)
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            },
        )
        manager.createNotificationChannel(
            NotificationChannel(OngoingChannel, "Текущий звонок", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Строка идущего разговора"
                setSound(null, null)
                enableVibration(false)
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
            },
        )
    }

    fun showIncoming(context: Context, call: CallState) {
        ensureChannels(context)
        val accept = activityIntent(context, ActionAccept, 1)
        val open = activityIntent(context, ActionShow, 2)
        val decline = receiverIntent(context, CallActionReceiver.ActionDecline, 3)
        val builder = builder(context, IncomingChannel)
            .setSmallIcon(R.drawable.ic_call)
            .setContentTitle(call.peerName)
            .setContentText("Входящий аудиозвонок · сквозное шифрование")
            .setCategory(Notification.CATEGORY_CALL)
            .setOngoing(true)
            .setAutoCancel(false)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setContentIntent(open)
            .setFullScreenIntent(open, true)
            .addAction(action(context, "Отклонить", decline))
            .addAction(action(context, "Ответить", accept))
        avatar(call)?.let(builder::setLargeIcon)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            @Suppress("DEPRECATION")
            builder.setPriority(Notification.PRIORITY_MAX)
        }
        notify(context, IncomingId, builder.build())
    }

    fun cancelIncoming(context: Context) {
        context.getSystemService(NotificationManager::class.java).cancel(IncomingId)
    }

    fun ongoing(context: Context, call: CallState?): Notification {
        ensureChannels(context)
        val open = activityIntent(context, ActionShow, 4)
        val hangUp = receiverIntent(context, CallActionReceiver.ActionHangUp, 5)
        val name = call?.peerName ?: "Звонок"
        val builder = builder(context, OngoingChannel)
            .setSmallIcon(R.drawable.ic_call)
            .setContentTitle(name)
            .setContentText(
                when (call?.phase) {
                    CallPhase.Active -> "Идёт звонок · сквозное шифрование"
                    CallPhase.Connecting -> "Соединение…"
                    CallPhase.Ringing -> "Звонит…"
                    else -> "Вызов…"
                },
            )
            .setCategory(Notification.CATEGORY_CALL)
            .setOngoing(true)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setContentIntent(open)
        if (call?.phase == CallPhase.Active) {
            builder.setUsesChronometer(true).setWhen(System.currentTimeMillis() - call.durationMs).setShowWhen(true)
        }
        call?.let(::avatar)?.let(builder::setLargeIcon)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val person = Person.Builder().setName(name).apply {
                call?.let(::avatar)?.let { setIcon(Icon.createWithBitmap(it)) }
            }.build()
            builder.setStyle(Notification.CallStyle.forOngoingCall(person, hangUp))
        } else {
            builder.addAction(action(context, "Завершить", hangUp))
        }
        return builder.build()
    }

    fun updateOngoing(context: Context, call: CallState) = notify(context, OngoingId, ongoing(context, call))

    private fun notify(context: Context, id: Int, notification: Notification) {
        // Без разрешения на уведомления система молча их отбросит — звонок от этого не страдает.
        runCatching { context.getSystemService(NotificationManager::class.java).notify(id, notification) }
    }

    private fun builder(context: Context, channel: String): Notification.Builder =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, channel)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(context)
        }

    private fun action(context: Context, title: String, intent: PendingIntent): Notification.Action =
        Notification.Action.Builder(Icon.createWithResource(context, R.drawable.ic_call), title, intent).build()

    private fun activityIntent(context: Context, action: String, request: Int): PendingIntent =
        PendingIntent.getActivity(
            context,
            request,
            Intent(context, MainActivity::class.java)
                .setAction(action)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun receiverIntent(context: Context, action: String, request: Int): PendingIntent =
        PendingIntent.getBroadcast(
            context,
            request,
            Intent(context, CallActionReceiver::class.java).setAction(action),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )

    private fun avatar(call: CallState): Bitmap? = call.peerAvatarBase64?.let {
        runCatching {
            val bytes = Base64.decode(it, Base64.DEFAULT)
            BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
        }.getOrNull()
    }
}
