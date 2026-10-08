package app.turattext.mobile

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.turattext.mobile.calls.CallController
import app.turattext.mobile.core.NativeCore
import app.turattext.mobile.describeMedia
import app.turattext.mobile.media.Compression
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.Contact
import app.turattext.mobile.model.Attachment
import app.turattext.mobile.model.MediaKind
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.CoreResult
import app.turattext.mobile.model.PendingUpload
import app.turattext.mobile.ui.AppTheme
import app.turattext.mobile.ui.AppFont
import app.turattext.mobile.ui.MediaTransfer
import app.turattext.mobile.update.AppUpdater
import app.turattext.mobile.update.UpdateState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import java.io.File

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val preferences = application.getSharedPreferences("turattext-ui", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(AppSnapshot(statusMessage = "Запуск…"))
    val state = _state.asStateFlow()
    private val _busy = MutableStateFlow(false)
    val busy = _busy.asStateFlow()
    private val _theme = MutableStateFlow(AppTheme.parse(preferences.getString("theme", null)))
    val theme = _theme.asStateFlow()
    private val _font = MutableStateFlow(AppFont.parse(preferences.getString("font", null)))
    val font = _font.asStateFlow()

    /** Вложения, которые прямо сейчас шифруются перед отправкой. */
    private val _uploads = MutableStateFlow<List<PendingUpload>>(emptyList())
    val uploads = _uploads.asStateFlow()

    /** Сохранение вложения на диск: прогресс по идентификатору сообщения. */
    private val _downloads = MutableStateFlow<Map<String, MediaTransfer>>(emptyMap())
    val downloads = _downloads.asStateFlow()

    /** Ядро однопоточное: фоновая синхронизация не должна пересекаться с действиями пользователя. */
    private val gate = Mutex()

    private val _update = MutableStateFlow(
        UpdateState(skippedVersion = preferences.getString(SkippedUpdateKey, null)),
    )
    val update = _update.asStateFlow()
    private var updateDownload: Job? = null

    init {
        NativeCore.initialize(application)
        CallController.attach(application)
        // Конец звонка ядро досылает собеседнику и пишет в историю — через ту же очередь,
        // чтобы запись о звонке сразу появилась в ленте.
        CallController.runner = { command ->
            viewModelScope.launch { runCommand(command, background = true) }
        }
        execute(CoreJson.command("snapshot"))
        startBackgroundSync()
        startUpdateChecks(application)
    }

    // --- обновления из GitHub Releases ---------------------------------------

    /**
     * Первая проверка — вскоре после запуска, дальше раз в несколько часов. Фоновая проверка
     * молчит об ошибках: недоступный GitHub не повод беспокоить пользователя.
     */
    private fun startUpdateChecks(application: Application) {
        // До установки новой версии здесь могли остаться скачанные APK — теперь они не нужны.
        AppUpdater.clearDownloads(application)
        viewModelScope.launch {
            AppUpdater.installFailures.collect { reason ->
                _update.update { it.copy(message = "Установка не удалась: $reason") }
            }
        }
        viewModelScope.launch {
            delay(FirstUpdateCheckDelayMilliseconds)
            while (isActive) {
                findUpdate(manual = false)
                delay(UpdateCheckIntervalMilliseconds)
            }
        }
    }

    /** Ручная проверка показывает результат и снова предлагает даже пропущенную версию. */
    fun checkForUpdates() {
        viewModelScope.launch { findUpdate(manual = true) }
    }

    private suspend fun findUpdate(manual: Boolean) {
        if (_update.value.checking) return
        _update.update { it.copy(checking = true, message = if (manual) "Проверяем релизы на GitHub…" else it.message) }
        val result = runCatching { AppUpdater.findUpdate() }
        val release = result.getOrNull()
        if (manual && release != null && release.version == _update.value.skippedVersion) {
            preferences.edit().remove(SkippedUpdateKey).apply()
            _update.update { it.copy(skippedVersion = null) }
        }
        _update.update { current ->
            current.copy(
                checking = false,
                available = if (result.isSuccess) release else current.available,
                bannerDismissed = current.bannerDismissed && !(manual && release != null),
                message = when {
                    !manual -> current.message
                    result.isFailure -> "Не удалось проверить обновления: ${result.exceptionOrNull()?.message}"
                    release == null -> "У вас последняя версия."
                    else -> "Доступна версия ${release.version}."
                },
            )
        }
    }

    /** Скрывает строку над списком до следующего запуска. */
    fun dismissUpdate() = _update.update { it.copy(bannerDismissed = true) }

    /** Отказ от конкретной версии: о ней больше не напоминаем, о следующей — напомним. */
    fun skipUpdate() {
        val version = _update.value.available?.version ?: return
        preferences.edit().putString(SkippedUpdateKey, version).apply()
        _update.update { it.copy(skippedVersion = version) }
    }

    fun installUpdate() {
        val release = _update.value.available ?: return
        if (updateDownload?.isActive == true) return
        val context = getApplication<Application>()
        if (!AppUpdater.canInstallPackages(context)) {
            _update.update {
                it.copy(message = "Разрешите Turat устанавливать приложения, затем нажмите «Обновить» ещё раз.")
            }
            AppUpdater.openInstallPermissionSettings(context)
            return
        }
        updateDownload = viewModelScope.launch {
            _update.update { it.copy(downloading = 0f, message = null) }
            try {
                val apk = AppUpdater.download(context, release) { fraction ->
                    _update.update { it.copy(downloading = fraction) }
                }
                _update.update {
                    it.copy(downloading = null, message = "Контрольная сумма совпала. Подтвердите установку в окне системы.")
                }
                withContext(Dispatchers.IO) { AppUpdater.install(context, apk) }
            } catch (cancelled: CancellationException) {
                _update.update { it.copy(downloading = null, message = "Загрузка отменена.") }
                throw cancelled
            } catch (error: Exception) {
                _update.update { it.copy(downloading = null, message = "Не удалось обновиться: ${error.message}") }
            }
        }
    }

    fun cancelUpdate() {
        updateDownload?.cancel()
    }

    /**
     * Клиент сам держит связь с Node: первая попытка сразу после запуска, дальше — ожидание
     * конверта на самой Node. Node держит запрос открытым и отвечает в тот момент, когда
     * сообщение приходит, поэтому оно попадает в ленту за доли секунды, а не к следующему
     * циклу опроса. Опрос по таймеру остаётся запасным путём: пока связи нет, пока Node не
     * умеет ждать или пока ящик ещё не заведён.
     */
    private fun startBackgroundSync() = viewModelScope.launch {
        while (isActive) {
            val online = runCommand(CoreJson.command("sync"), background = true)
            if (!online) {
                delay(OfflineRetryIntervalMilliseconds)
                continue
            }
            // Ожидание идёт мимо ядра и мимо `gate`: отправка сообщения его не ждёт.
            val awaited = withContext(Dispatchers.IO) { NativeCore.waitForEnvelopes(WaitWindowSeconds) }
            // Конверт пришёл или окно истекло — цикл сам сходит за ним. Ждать негде
            // (-1) — возвращаемся к прежнему интервалу опроса.
            if (awaited < 0) delay(OnlineSyncIntervalMilliseconds)
        }
    }

    fun execute(command: String, after: ((Boolean) -> Unit)? = null) {
        viewModelScope.launch {
            _busy.value = true
            val ok = runCommand(command, background = false)
            _busy.value = false
            after?.invoke(ok)
        }
    }

    private suspend fun runCommand(command: String, background: Boolean): Boolean =
        applySnapshot(invoke(command), background).ok

    /** Вызов ядра с полным ответом: командам вложений нужен не снимок, а возвращённое значение. */
    private suspend fun invoke(command: String): CoreResult = gate.withLock {
        withContext(Dispatchers.IO) { CoreJson.parse(NativeCore.invoke(command)) }
    }

    private fun applySnapshot(result: CoreResult, background: Boolean): CoreResult {
        val snapshot = result.snapshot ?: return result
        // Фоновая ошибка не должна затирать подсказку, которую пользователь только что увидел.
        _state.value = if (background && !result.ok) {
            snapshot.copy(statusMessage = _state.value.statusMessage)
        } else {
            snapshot
        }
        return result
    }

    /**
     * Отправка файла.
     *
     * Ядро шифрует его в фоне и сразу возвращает идентификатор задачи, поэтому пузырь с
     * прогрессом появляется в ленте немедленно, а не после того как 300 МБ будут обработаны.
     * Готовая задача превращается в сообщение командой `finish_attachment`.
     */
    fun startAttachment(
        userId: String,
        path: String,
        attachment: Attachment,
        caption: String?,
        replyToEventId: String?,
        compress: Boolean = false,
    ) = viewModelScope.launch {
        var source = path
        var media = attachment
        if (compress) {
            compressed(userId, path, attachment, caption)?.let { (producedPath, produced) ->
                source = producedPath
                media = produced
                // Копия оригинала в кэше больше не нужна: дальше в ядро уходит сжатый файл.
                File(path).delete()
            }
        }
        val path = source
        val attachment = media
        val started = invoke(
            CoreJson.command(
                "start_attachment",
                "user_id" to userId,
                "path" to path,
                "mime_type" to attachment.mimeType,
                "caption" to caption,
                "reply_to_event_id" to replyToEventId,
                "kind" to attachment.kind.name.lowercase(),
                "width" to attachment.width,
                "height" to attachment.height,
                "duration_milliseconds" to attachment.durationMilliseconds,
                "thumbnail_base64" to attachment.thumbnailBase64,
            )
        )
        val jobId = started.value?.optString("jobId").orEmpty()
        if (!started.ok || jobId.isEmpty()) {
            _state.value = _state.value.copy(
                statusMessage = started.error ?: "Не удалось начать отправку файла",
            )
            return@launch
        }
        _uploads.update {
            it + PendingUpload(
                jobId = jobId,
                userId = userId,
                attachment = attachment,
                caption = caption.orEmpty(),
                createdAt = System.currentTimeMillis(),
                total = attachment.size,
            )
        }
        val finished = track(jobId) { done, total ->
            _uploads.update { list ->
                list.map { if (it.jobId == jobId) it.copy(done = done, total = total) else it }
            }
        }
        if (finished == null) {
            _uploads.update { list -> list.filterNot { it.jobId == jobId } }
            return@launch
        }
        if (finished.first) {
            runCommand(CoreJson.command("finish_attachment", "job_id" to jobId), background = false)
            _uploads.update { list -> list.filterNot { it.jobId == jobId } }
        } else {
            // Ошибку видно на самом пузыре: она исчезает вместе с ним через несколько секунд.
            _uploads.update { list ->
                list.map { if (it.jobId == jobId) it.copy(failed = true, error = finished.second) else it }
            }
            delay(FailedTransferLingerMilliseconds)
            _uploads.update { list -> list.filterNot { it.jobId == jobId } }
        }
    }

    /**
     * Сжатие перед отправкой. Задачи в ядре ещё нет, поэтому на время перекодирования в ленте
     * висит собственный пузырь с процентами: иначе выбранное видео просто пропало бы на минуту.
     *
     * Возвращает `null`, если сжимать нечего или не вышло, — тогда уходит оригинал.
     */
    private suspend fun compressed(
        userId: String,
        path: String,
        attachment: Attachment,
        caption: String?,
    ): Pair<String, Attachment>? {
        val context = getApplication<Application>()
        val token = "compress-" + System.nanoTime()
        _uploads.update {
            it + PendingUpload(
                jobId = token,
                userId = userId,
                attachment = attachment,
                caption = caption.orEmpty(),
                createdAt = System.currentTimeMillis(),
                total = 100,
                compressing = true,
            )
        }
        val report: (Int) -> Unit = { percent ->
            _uploads.update { list ->
                list.map { if (it.jobId == token) it.copy(done = percent.toLong()) else it }
            }
        }
        val target = File(
            context.cacheDir,
            "compressed-" + System.nanoTime() + if (attachment.kind == MediaKind.Image) ".jpg" else ".mp4",
        )
        val produced = try {
            when (attachment.kind) {
                MediaKind.Image -> Compression.image(path, target.absolutePath)
                MediaKind.Video -> Compression.video(
                    context,
                    path,
                    target.absolutePath,
                    minOf(attachment.width, attachment.height),
                    report,
                )
                else -> null
            }
        } finally {
            _uploads.update { list -> list.filterNot { it.jobId == token } }
        }
        if (produced == null) return null
        val mime = if (attachment.kind == MediaKind.Image) "image/jpeg" else "video/mp4"
        val extension = if (attachment.kind == MediaKind.Image) "jpg" else "mp4"
        return produced to describeMedia(produced, mime, renamed(attachment.fileName, extension))
    }

    /** Имя остаётся узнаваемым, но расширение должно отвечать новому содержимому файла. */
    private fun renamed(fileName: String, extension: String): String {
        val base = fileName.substringBeforeLast('.', fileName).ifBlank { "media" }
        return "$base.$extension"
    }

    /** Короткое сообщение в строке состояния: отказ должен быть виден там же, где всё прочее. */
    fun notify(message: String) {
        _state.value = _state.value.copy(statusMessage = message)
    }

    fun cancelUpload(jobId: String) = viewModelScope.launch {
        invoke(CoreJson.command("cancel_media_job", "job_id" to jobId))
        _uploads.update { list -> list.filterNot { it.jobId == jobId } }
    }

    /**
     * Сохранение вложения в файл. Расшифровка тоже идёт фоновой задачей, поэтому прогресс
     * виден прямо на пузыре, а тяжёлое видео не блокирует интерфейс.
     */
    fun exportAttachment(eventId: String, destinationPath: String, after: (Boolean) -> Unit) =
        viewModelScope.launch {
            val started = invoke(
                CoreJson.command(
                    "start_export_attachment",
                    "event_id" to eventId,
                    "destination_path" to destinationPath,
                )
            )
            val jobId = started.value?.optString("jobId").orEmpty()
            if (!started.ok || jobId.isEmpty()) {
                after(false)
                return@launch
            }
            _downloads.update { it + (eventId to MediaTransfer(jobId, 0, 0)) }
            val finished = track(jobId) { done, total ->
                _downloads.update { it + (eventId to MediaTransfer(jobId, done, total)) }
            }
            _downloads.update { it - eventId }
            val ok = finished?.first == true
            if (!ok) {
                _state.value = _state.value.copy(
                    statusMessage = finished?.second?.ifBlank { null } ?: "Не удалось сохранить файл",
                )
            }
            after(ok)
        }

    /**
     * Следит за фоновой задачей ядра до её завершения. Возвращает `null`, если задача пропала
     * (например была отменена), иначе — успех и текст ошибки.
     */
    private suspend fun track(
        jobId: String,
        onProgress: (Long, Long) -> Unit,
    ): Pair<Boolean, String>? {
        while (true) {
            val polled = invoke(CoreJson.command("media_job", "job_id" to jobId))
            val value = polled.value
            if (!polled.ok || value == null) return null
            onProgress(value.optLong("done"), value.optLong("total"))
            when (value.optString("state")) {
                "done" -> return true to ""
                "failed" -> return false to value.optString("error")
            }
            delay(TransferPollIntervalMilliseconds)
        }
    }

    fun setTheme(theme: AppTheme) {
        _theme.value = theme
        preferences.edit().putString("theme", theme.name).apply()
    }

    fun setFont(font: AppFont) {
        _font.value = font
        preferences.edit().putString("font", font.name).apply()
    }

    /** Открытие чата сразу снимает счётчик непрочитанных — как в Telegram. */
    fun select(userId: String?) = execute(CoreJson.command("select_contact", "user_id" to userId)) {
        if (userId != null) execute(CoreJson.command("mark_read", "user_id" to userId))
    }

    fun addContact(query: String, after: ((Boolean) -> Unit)? = null) =
        execute(CoreJson.command("add_contact", "query" to query, "display_name" to null), after)

    fun send(userId: String, text: String, replyTo: String? = null) =
        execute(CoreJson.command("send_text", "user_id" to userId, "text" to text, "reply_to_event_id" to replyTo))

    fun forward(eventIds: Set<String>, userId: String) =
        execute(CoreJson.command("forward_messages", "event_ids" to JSONArray(eventIds.toList()), "user_id" to userId))

    fun setPinned(userId: String, pinned: Boolean) =
        execute(CoreJson.command("set_chat_pinned", "user_id" to userId, "pinned" to pinned))

    fun setMuted(userId: String, muted: Boolean) =
        execute(CoreJson.command("set_chat_muted", "user_id" to userId, "muted" to muted))

    fun saveDraft(userId: String, text: String) =
        execute(CoreJson.command("save_draft", "user_id" to userId, "text" to text))

    /**
     * Отметка прочтения для открытого диалога.
     *
     * Идёт мимо [execute]: сообщение, прочитанное прямо на глазах у пользователя, не повод
     * зажигать индикатор занятости и затирать строку состояния.
     */
    fun markRead(userId: String) = viewModelScope.launch {
        runCommand(CoreJson.command("mark_read", "user_id" to userId), background = true)
    }

    fun clearHistory(userId: String) = execute(CoreJson.command("clear_history", "user_id" to userId))
    fun markUnread(userId: String) = execute(CoreJson.command("mark_unread", "user_id" to userId))
    fun search(query: String) = execute(CoreJson.command("search", "query" to query))

    fun setPresencePublishing(enabled: Boolean) =
        execute(CoreJson.command("set_presence_publishing", "enabled" to enabled))
    fun deleteContact(userId: String) = execute(CoreJson.command("delete_contact", "user_id" to userId))
    fun accept(userId: String) = execute(CoreJson.command("accept_contact", "user_id" to userId))
    fun reject(userId: String) = execute(CoreJson.command("reject_contact", "user_id" to userId))
    fun verify(userId: String, verified: Boolean) =
        execute(CoreJson.command("verify_contact", "user_id" to userId, "verified" to verified))

    fun react(eventIds: Set<String>, reaction: String) = execute(CoreJson.command("react", "event_ids" to JSONArray(eventIds.toList()), "reaction" to reaction))
    fun deleteMessages(eventIds: Set<String>) = execute(CoreJson.command("delete_messages", "event_ids" to JSONArray(eventIds.toList())))
    fun edit(eventId: String, text: String) = execute(CoreJson.command("edit_message", "event_id" to eventId, "text" to text))
    fun sync() = execute(CoreJson.command("sync"))

    // --- звонки ----------------------------------------------------------------

    /** Экран звонка открывается сразу, а комнату на Node ядро создаёт в фоне. */
    fun startCall(contact: Contact) {
        if (CallController.state.value?.live == true) {
            notify("Уже идёт звонок")
            return
        }
        CallController.dial(contact.userId, contact.displayName, contact.avatarBase64)
        viewModelScope.launch {
            val result = applySnapshot(invoke(CoreJson.command("start_call", "user_id" to contact.userId)), background = false)
            if (result.ok) {
                CallController.dialStarted()
            } else {
                CallController.dialFailed(result.error ?: "Не удалось позвонить")
            }
        }
    }

    fun acceptCall() {
        CallController.answering()
        viewModelScope.launch {
            val result = applySnapshot(invoke(CoreJson.command("accept_call")), background = false)
            if (!result.ok) {
                notify(result.error ?: "Не удалось ответить")
                CallController.hangUp()
            }
        }
    }

    override fun onCleared() {
        CallController.runner = null
        // Идущий звонок переживает закрытие окна: его держит сервис, а ядро нужно звуку.
        if (CallController.state.value?.live != true) NativeCore.close()
        super.onCleared()
    }

    private companion object {
        const val OnlineSyncIntervalMilliseconds = 20_000L
        /** Окно ожидания конверта; Node ограничивает его своей настройкой. */
        const val WaitWindowSeconds = 25
        const val OfflineRetryIntervalMilliseconds = 6_000L
        const val TransferPollIntervalMilliseconds = 120L
        const val FailedTransferLingerMilliseconds = 4_000L
        const val FirstUpdateCheckDelayMilliseconds = 5_000L
        const val UpdateCheckIntervalMilliseconds = 6 * 60 * 60 * 1000L
        const val SkippedUpdateKey = "update-skipped"
    }
}
