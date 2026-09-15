package app.turattext.mobile.update

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build

/**
 * Статус сессии установки. Системе нужно подтверждение пользователя — открываем её окно;
 * отказ установщика пересылаем на экран обновления.
 */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirmation = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                } ?: return
                context.startActivity(confirmation.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            }

            PackageInstaller.STATUS_SUCCESS -> Unit

            // Пользователь сам закрыл системное окно — это не ошибка.
            PackageInstaller.STATUS_FAILURE_ABORTED -> Unit

            else -> AppUpdater.reportInstallFailure(
                intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE) ?: "установщик отклонил пакет",
            )
        }
    }
}
