package app.turattext.mobile

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Color
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.provider.OpenableColumns
import android.os.Build
import android.os.Bundle
import android.util.Base64
import android.view.View
import android.view.WindowInsetsController
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import app.turattext.mobile.model.Attachment
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.MediaKind
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
            val font by model.font.collectAsStateWithLifecycle()
            TuratTextTheme(theme, font) {
                val night = Telegram.colors.night
                LaunchedEffect(night) { applySystemBarIcons(night) }
                val state by model.state.collectAsStateWithLifecycle()
                val busy by model.busy.collectAsStateWithLifecycle()
                val uploads by model.uploads.collectAsStateWithLifecycle()
                val downloads by model.downloads.collectAsStateWithLifecycle()
                var pendingImport by remember { mutableStateOf<Pair<String, String>?>(null) }
                var pendingExport by remember { mutableStateOf<Triple<String, String, String>?>(null) }
                var pendingAttachmentSave by remember { mutableStateOf<String?>(null) }

                var pendingSendChoice by remember { mutableStateOf<SendChoice?>(null) }
                val attachmentPicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
                    val contact = model.state.value.selectedContactId ?: return@rememberLauncherForActivityResult
                    uri ?: return@rememberLauncherForActivityResult
                    // Копия в кэше нужна ядру как обычный файл; заодно из неё берутся размеры,
                    // длительность и миниатюра, чтобы пузырь не был пустым прямоугольником.
                    val path = copyToCache(this, uri, "attachment")
                    val attachment = describeAttachment(this, uri, path)
                    val choice = SendChoice(contact, path, attachment)
                    // Фото и видео можно отправить двумя способами, и выбор за пользователем:
                    // сжатое медиа или файл байт в байт. Всё остальное — всегда файл.
                    when {
                        attachment.kind != MediaKind.Image && attachment.kind != MediaKind.Video ->
                            sendAsFile(model, choice)

                        attachment.size > MaximumMediaBytes -> {
                            File(path).delete()
                            model.notify("Медиа больше ${formatBytes(MaximumMediaBytes)}")
                        }

                        else -> pendingSendChoice = choice
                    }
                }
                val attachmentSaver = rememberLauncherForActivityResult(
                    ActivityResultContracts.CreateDocument("application/octet-stream")
                ) { uri ->
                    val eventId = pendingAttachmentSave
                    pendingAttachmentSave = null
                    if (uri == null || eventId == null) return@rememberLauncherForActivityResult
                    val temporary = File(cacheDir, "save-" + System.nanoTime() + ".tmp")
                    model.exportAttachment(eventId, temporary.absolutePath) { ok ->
                        if (ok) {
                            contentResolver.openOutputStream(uri)?.use { out ->
                                temporary.inputStream().use { it.copyTo(out) }
                            }
                        }
                        temporary.delete()
                    }
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
                        markRead = model::markRead,
                        search = model::search,
                        setPresence = model::setPresencePublishing,
                        sync = model::sync,
                        command = model::execute,
                        pickAttachment = { attachmentPicker.launch("*/*") },
                        pickAvatar = { avatarPicker.launch("image/*") },
                        saveAttachment = { message ->
                            message.attachment?.let { attachment ->
                                pendingAttachmentSave = message.eventId
                                attachmentSaver.launch(attachment.fileName)
                            }
                        },
                        cancelTransfer = model::cancelUpload,
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
                    uploads = uploads,
                    downloads = downloads,
                    theme = theme,
                    font = font,
                    onThemeChange = model::setTheme,
                    onFontChange = model::setFont,
                    actions = actions,
                )

                pendingSendChoice?.let { choice ->
                    SendChoiceDialog(
                        choice = choice,
                        onMedia = {
                            pendingSendChoice = null
                            model.startAttachment(
                                choice.contactId, choice.path, choice.attachment,
                                caption = null, replyToEventId = null, compress = true,
                            )
                        },
                        onFile = {
                            pendingSendChoice = null
                            sendAsFile(model, choice)
                        },
                        onDismiss = {
                            pendingSendChoice = null
                            File(choice.path).delete()
                        },
                    )
                }
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

/**
 * Разбор выбранного файла: тип, настоящее имя, размеры кадра, длительность и миниатюра.
 *
 * Всё это уходит в сообщение вместе с вложением, поэтому получатель видит превью и
 * длительность ролика сразу, не дожидаясь расшифровки оригинала.
 */
private fun describeAttachment(context: Context, uri: Uri, path: String): Attachment {
    val mime = context.contentResolver.getType(uri) ?: "application/octet-stream"
    return describeMedia(path, mime, displayName(context, uri) ?: File(path).name)
}

/**
 * То же самое, но по готовому файлу с уже известными именем и типом: после сжатия описывать
 * надо результат, а не то, что пользователь выбрал, — размеры кадра и вес у него другие.
 */
internal fun describeMedia(path: String, mime: String, name: String): Attachment {
    val kind = MediaKind.parse(null, mime)
    val file = File(path)
    var width = 0
    var height = 0
    var duration = 0L
    var thumbnail: String? = null
    when (kind) {
        MediaKind.Image -> {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            runCatching { BitmapFactory.decodeFile(path, bounds) }
            width = bounds.outWidth.coerceAtLeast(0)
            height = bounds.outHeight.coerceAtLeast(0)
            thumbnail = runCatching {
                val sample = BitmapFactory.Options().apply {
                    inSampleSize = sampleSize(maxOf(width, height), ThumbnailEdge)
                }
                BitmapFactory.decodeFile(path, sample)?.let(::encodeThumbnail)
            }.getOrNull()
        }

        // MediaMetadataRetriever стал AutoCloseable только на API 29, а приложение живёт с 23.
        MediaKind.Video, MediaKind.Audio -> {
            val retriever = MediaMetadataRetriever()
            try {
                retriever.setDataSource(path)
                duration = retriever
                    .extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)
                    ?.toLongOrNull() ?: 0L
                width = retriever
                    .extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH)
                    ?.toIntOrNull() ?: 0
                height = retriever
                    .extractMetadata(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT)
                    ?.toIntOrNull() ?: 0
                thumbnail = retriever.frameAtTime?.let(::encodeThumbnail)
            } catch (error: RuntimeException) {
                // Повреждённый или неподдерживаемый контейнер — вложение уйдёт без превью.
            } finally {
                retriever.release()
            }
        }

        MediaKind.File -> Unit
    }
    return Attachment(
        id = "", fileName = name, mimeType = mime, size = file.length(), localPath = path,
        kind = kind, width = width, height = height, durationMilliseconds = duration,
        thumbnailBase64 = thumbnail,
    )
}

/** Провайдеры отдают файл под техническим именем, поэтому настоящее берётся из курсора. */
private fun displayName(context: Context, uri: Uri): String? = runCatching {
    context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        ?.use { cursor -> if (cursor.moveToFirst()) cursor.getString(0) else null }
}.getOrNull()

internal const val ThumbnailEdge = 240

internal fun sampleSize(longestEdge: Int, target: Int): Int {
    var sample = 1
    while (longestEdge > 0 && longestEdge / sample > target) sample *= 2
    return sample
}

/** Миниатюра едет вместе с сообщением, поэтому она нарочно мелкая и сильно сжатая. */
internal fun encodeThumbnail(source: Bitmap): String? = runCatching {
    val longest = maxOf(source.width, source.height)
    val scaled = if (longest <= ThumbnailEdge) source else Bitmap.createScaledBitmap(
        source,
        (source.width * ThumbnailEdge / longest).coerceAtLeast(1),
        (source.height * ThumbnailEdge / longest).coerceAtLeast(1),
        true,
    )
    val output = ByteArrayOutputStream()
    scaled.compress(Bitmap.CompressFormat.JPEG, 62, output)
    Base64.encodeToString(output.toByteArray(), Base64.NO_WRAP)
}.getOrNull()

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

/** Выбранное вложение, ждущее решения: отправить сжатым медиа или файлом как есть. */
internal data class SendChoice(
    val contactId: String,
    val path: String,
    val attachment: Attachment,
)

/** Медиа клиент сжимает, поэтому исходник может быть крупным. */
internal const val MaximumMediaBytes = 512L * 1024 * 1024

/** Файл уходит байт в байт, и предел на него заметно строже. */
internal const val MaximumFileBytes = 100L * 1024 * 1024

/**
 * Отправка без сжатия. Тип принудительно становится файлом: получатель должен увидеть
 * вложение таким, каким его отправили, а не пытаться проиграть его в ленте.
 */
private fun sendAsFile(model: MainViewModel, choice: SendChoice) {
    if (choice.attachment.size > MaximumFileBytes) {
        File(choice.path).delete()
        model.notify("Файл больше ${formatBytes(MaximumFileBytes)}")
        return
    }
    model.startAttachment(
        choice.contactId,
        choice.path,
        choice.attachment.copy(kind = MediaKind.File, thumbnailBase64 = null),
        caption = null,
        replyToEventId = null,
    )
}

/**
 * Выбор способа отправки.
 *
 * Пункт «как файл» остаётся видимым и когда файл слишком велик — но с подписью, объясняющей
 * почему он недоступен: молча спрятанная кнопка выглядит как поломка.
 */
@Composable
private fun SendChoiceDialog(
    choice: SendChoice,
    onMedia: () -> Unit,
    onFile: () -> Unit,
    onDismiss: () -> Unit,
) {
    val video = choice.attachment.kind == MediaKind.Video
    val fileAllowed = choice.attachment.size <= MaximumFileBytes
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (video) "Отправить видео" else "Отправить фото") },
        text = {
            Column {
                Text(
                    if (video) {
                        "Как видео — ролик будет сжат и пойдёт быстрее. Как файл — исходник без изменений."
                    } else {
                        "Как фото — снимок будет сжат и пойдёт быстрее. Как файл — исходник без изменений."
                    },
                )
                Spacer(Modifier.height(8.dp))
                Text(
                    if (fileAllowed) {
                        "Размер: ${formatBytes(choice.attachment.size)}"
                    } else {
                        "Размер: ${formatBytes(choice.attachment.size)} — файлом можно до " +
                            formatBytes(MaximumFileBytes)
                    },
                )
            }
        },
        confirmButton = {
            TextButton(onClick = onMedia) { Text(if (video) "Как видео" else "Как фото") }
        },
        dismissButton = {
            TextButton(onClick = onFile, enabled = fileAllowed) { Text("Как файл") }
        },
    )
}

internal fun formatBytes(value: Long): String {
    val megabytes = 1024.0 * 1024.0
    return if (value >= 1024 * megabytes) {
        String.format(java.util.Locale.US, "%.1f ГБ", value / (1024 * megabytes))
    } else {
        String.format(java.util.Locale.US, "%.1f МБ", value / megabytes)
    }
}
