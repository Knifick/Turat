package app.turattext.mobile.calls

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Кнопки «Отклонить» и «Завершить» в уведомлениях: работают без открытия приложения. */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            ActionDecline, ActionHangUp -> {
                CallController.attach(context)
                CallController.hangUp()
            }
        }
    }

    companion object {
        const val ActionDecline = "app.turattext.mobile.CALL_DECLINE"
        const val ActionHangUp = "app.turattext.mobile.CALL_HANGUP"
    }
}
