package app.turattext.mobile

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import app.turattext.mobile.core.NativeCore
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.ui.AppTheme
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val preferences = application.getSharedPreferences("turattext-ui", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(AppSnapshot(statusMessage = "Запуск…"))
    val state = _state.asStateFlow()
    private val _busy = MutableStateFlow(false)
    val busy = _busy.asStateFlow()
    private val _theme = MutableStateFlow(AppTheme.parse(preferences.getString("theme", null)))
    val theme = _theme.asStateFlow()

    /** Ядро однопоточное: фоновая синхронизация не должна пересекаться с действиями пользователя. */
    private val gate = Mutex()

    init {
        NativeCore.initialize(application)
        execute(CoreJson.command("snapshot"))
        startBackgroundSync()
    }

    /**
     * Клиент сам держит связь с Node: первая попытка сразу после запуска, дальше — короткий
     * интервал, пока связи нет, и спокойный, когда она есть. Кнопка синхронизации остаётся
     * ручным ускорителем, а не единственным способом получить сообщения.
     */
    private fun startBackgroundSync() = viewModelScope.launch {
        while (isActive) {
            val online = runCommand(CoreJson.command("sync"), background = true)
            delay(if (online) OnlineSyncIntervalMilliseconds else OfflineRetryIntervalMilliseconds)
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

    private suspend fun runCommand(command: String, background: Boolean): Boolean {
        val result = gate.withLock {
            withContext(Dispatchers.IO) { CoreJson.parse(NativeCore.invoke(command)) }
        }
        val snapshot = result.snapshot ?: return result.ok
        // Фоновая ошибка не должна затирать подсказку, которую пользователь только что увидел.
        _state.value = if (background && !result.ok) {
            snapshot.copy(statusMessage = _state.value.statusMessage)
        } else {
            snapshot
        }
        return result.ok
    }

    fun setTheme(theme: AppTheme) {
        _theme.value = theme
        preferences.edit().putString("theme", theme.name).apply()
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

    override fun onCleared() {
        NativeCore.close()
        super.onCleared()
    }

    private companion object {
        const val OnlineSyncIntervalMilliseconds = 20_000L
        const val OfflineRetryIntervalMilliseconds = 6_000L
    }
}
