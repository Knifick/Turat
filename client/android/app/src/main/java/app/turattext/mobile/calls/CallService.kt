package app.turattext.mobile.calls

import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log

/**
 * Держит процесс на переднем плане, пока идёт звонок: без этого система отберёт микрофон,
 * как только приложение свернут, и усыпит сеть посреди разговора.
 */
class CallService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = CallNotifications.ongoing(this, CallController.state.value)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                startForeground(
                    CallNotifications.OngoingId,
                    notification,
                    ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
                )
            } else {
                startForeground(CallNotifications.OngoingId, notification)
            }
        } catch (error: Exception) {
            // Например, нет доступа к микрофону: звонок продолжится, пока приложение на экране.
            Log.w("TuratCall", "Сервис звонка не вышел на передний план", error)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    companion object {
        fun start(context: Context) {
            val intent = Intent(context, CallService::class.java)
            runCatching {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    context.startForegroundService(intent)
                } else {
                    context.startService(intent)
                }
            }.onFailure { Log.w("TuratCall", "Не удалось запустить сервис звонка", it) }
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, CallService::class.java))
        }
    }
}
