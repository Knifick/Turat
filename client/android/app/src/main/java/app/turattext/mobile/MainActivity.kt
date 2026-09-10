package app.turattext.mobile

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.util.Base64
import android.view.View
import android.view.WindowInsetsController
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.*
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.ui.AppActions
import app.turattext.mobile.ui.Telegram
import app.turattext.mobile.ui.TuratTextApp
import app.turattext.mobile.ui.TuratTextTheme
import java.io.ByteArrayOutputStream
import java.io.File

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        drawEdgeToEdge()
        setContent {
            val model: MainViewModel = viewModel()
            val theme by model.theme.collectAsStateWithLifecycle()
            TuratTextTheme(theme) {
                val night = Telegram.colors.night
                LaunchedEffect(night) { applySystemBarIcons(night) }
                val state by model.state.collectAsStateWithLifecycle()
                val busy by model.busy.collectAsStateWithLifecycle()
                var pendingImport by remember { mutableStateOf<Pair<String, String>?>(null) }
                var pendingExport by remember { mutableStateOf<Triple<String, String, String>?>(null) }

                val attachmentPicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
                    val contact = model.state.value.selectedContactId ?: return@rememberLauncherForActivityResult
                    uri ?: return@rememberLauncherForActivityResult
                    val path = copyToCache(this, uri, "attachment")
                    val mime = contentResolver.getType(uri) ?: "application/octet-stream"
                    model.execute(
                        CoreJson.command(
                            "attach_file", "user_id" to contact, "path" to path,
                            "mime_type" to mime, "caption" to null, "reply_to_event_id" to null,
                        )
                    )
                }
                val avatarPicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
                    uri ?: return@rememberLauncherForActivityResult
                    val encoded = encodeAvatar(this, uri) ?: return@rememberLauncherForActivityResult
                    val profile = model.state.value.profile
                    model.execute(
                        CoreJson.command(
                            "save_profile", "username" to profile.username,
                            "display_name" to profile.displayName, "about" to profile.about,
                            "avatar_base64" to encoded,
                        )
                    ) { saved -> if (saved) model.execute(CoreJson.command("publish_profile")) }
                }
                val importer = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
                    val action = pendingImport
                    pendingImport = null
                    if (uri == null || action == null) return@rememberLauncherForActivityResult
                    val path = copyToCache(this, uri, "import")
                    model.execute(CoreJson.command(action.first, "path" to path, "passphrase" to action.second))
                }
                val exporter = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
                    val action = pendingExport
                    pendingExport = null
                    if (uri == null || action == null) return@rememberLauncherForActivityResult
                    val temp = File(cacheDir, "${action.first}-${System.nanoTime()}.tmp")
                    model.execute(CoreJson.command(action.first, "path" to temp.absolutePath, "passphrase" to action.second)) { ok ->
                        if (ok) contentResolver.openOutputStream(uri)?.use { out -> temp.inputStream().use { it.copyTo(out) } }
                        temp.delete()
                    }
                }

                val actions = remember(model) {
                    AppActions(
                        selectContact = model::select,
                        addContact = model::addContact,
                        deleteContact = model::deleteContact,
                        acceptContact = model::accept,
                        rejectContact = model::reject,
                        verifyContact = model::verify,
                        send = model::send,
                        edit = model::edit,
                        deleteMessages = model::deleteMessages,
                        react = model::react,
                        forward = model::forward,
                        setPinned = model::setPinned,
                        setMuted = model::setMuted,
                        saveDraft = model::saveDraft,
                        clearHistory = model::clearHistory,
                        markUnread = model::markUnread,
                        search = model::search,
                        setPresence = model::setPresencePublishing,
                        sync = model::sync,
                        command = model::execute,
                        pickAttachment = { attachmentPicker.launch("*/*") },
                        pickAvatar = { avatarPicker.launch("image/*") },
                        export = { command, passphrase, fileName ->
                            pendingExport = Triple(command, passphrase, fileName)
                            exporter.launch(fileName)
                        },
                        importFile = { command, passphrase, mimeTypes ->
                            pendingImport = command to passphrase
                            importer.launch(mimeTypes)
                        },
                    )
                }

                TuratTextApp(
                    state = state,
                    busy = busy,
                    theme = theme,
                    onThemeChange = model::setTheme,
                    actions = actions,
                )
            }
        }
    }

    /** Приложение рисует фон под системными панелями, как Telegram. */
    private fun drawEdgeToEdge() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.setDecorFitsSystemWindows(false)
        } else {
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility =
                View.SYSTEM_UI_FLAG_LAYOUT_STABLE or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) {
            @Suppress("DEPRECATION")
            window.statusBarColor = Color.TRANSPARENT
            @Suppress("DEPRECATION")
            window.navigationBarColor = Color.TRANSPARENT
        }
    }

    /** Тёмная тема — светлые значки в системных панелях, светлая — тёмные. */
    private fun applySystemBarIcons(night: Boolean) {
        val light = if (night) 0 else
            WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or
                WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            window.insetsController?.setSystemBarsAppearance(
                light,
                WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS or
                    WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS,
            )
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            @Suppress("DEPRECATION")
            val flags = window.decorView.systemUiVisibility
            @Suppress("DEPRECATION")
            window.decorView.systemUiVisibility = if (night) {
                flags and View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR.inv()
            } else {
                flags or View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR
            }
        }
    }
}

private fun copyToCache(context: Context, uri: Uri, prefix: String): String {
    val target = File.createTempFile("$prefix-", ".bin", context.cacheDir)
    context.contentResolver.openInputStream(uri)?.use { input -> target.outputStream().use(input::copyTo) }
        ?: error("Не удалось прочитать выбранный файл")
    return target.absolutePath
}

/** Аватар уменьшается до 256 px и кодируется в JPEG: запись профиля должна быть компактной. */
private fun encodeAvatar(context: Context, uri: Uri): String? = runCatching {
    val source = context.contentResolver.openInputStream(uri)?.use(BitmapFactory::decodeStream) ?: return null
    val size = maxOf(source.width, source.height)
    val scaled = if (size <= 256) source else {
        Bitmap.createScaledBitmap(source, source.width * 256 / size, source.height * 256 / size, true)
    }
    val output = ByteArrayOutputStream()
    scaled.compress(Bitmap.CompressFormat.JPEG, 80, output)
    Base64.encodeToString(output.toByteArray(), Base64.NO_WRAP)
}.getOrNull()
