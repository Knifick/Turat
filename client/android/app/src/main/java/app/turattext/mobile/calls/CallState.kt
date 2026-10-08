package app.turattext.mobile.calls

import org.json.JSONObject

/** Фаза звонка — так же, как её называет ядро. */
enum class CallPhase {
    /** Приглашение ушло, собеседник ещё не подтвердил, что телефон звонит. */
    Calling,
    /** У собеседника звонит. */
    Ringing,
    Incoming,
    /** Ответили: ключи согласованы, идёт подключение к ретранслятору. */
    Connecting,
    Active,
    Ended;

    companion object {
        fun parse(value: String): CallPhase = entries.firstOrNull { it.name.equals(value, ignoreCase = true) } ?: Ended
    }
}

/** Снимок звонка из ядра; экран и уведомления рисуются только по нему. */
data class CallState(
    val callId: String,
    val peerUserId: String,
    val peerName: String,
    val peerAvatarBase64: String?,
    val outgoing: Boolean,
    val phase: CallPhase,
    val endReason: String?,
    val durationMs: Long,
    val muted: Boolean,
    val peerMuted: Boolean,
    /** `udp`, `tls` или пусто, пока медиапотока нет. */
    val transport: String,
    /** `good`, `fair` или `poor`. */
    val quality: String,
    val rttMs: Long,
    val localLevel: Float,
    val peerLevel: Float,
    /** Четыре эмодзи из общего ключа: у обоих собеседников они совпадают. */
    val safetyCode: List<String>,
) {
    val ringing get() = phase == CallPhase.Calling || phase == CallPhase.Ringing || phase == CallPhase.Incoming
    val live get() = phase != CallPhase.Ended
    val hasMedia get() = phase == CallPhase.Connecting || phase == CallPhase.Active

    companion object {
        /** Пустой ответ (`active: false`) — звонка нет. */
        fun parse(json: String): CallState? = runCatching {
            val value = JSONObject(json)
            if (!value.optBoolean("active")) return null
            val code = value.optJSONArray("safetyCode")
            CallState(
                callId = value.optString("callId"),
                peerUserId = value.optString("peerUserId"),
                peerName = value.optString("peerName").ifBlank { "Собеседник" },
                peerAvatarBase64 = value.optString("peerAvatarBase64").takeIf { it.isNotBlank() && it != "null" },
                outgoing = value.optBoolean("outgoing"),
                phase = CallPhase.parse(value.optString("phase")),
                endReason = value.optString("endReason").takeIf { it.isNotBlank() && it != "null" },
                durationMs = value.optLong("durationMs"),
                muted = value.optBoolean("muted"),
                peerMuted = value.optBoolean("peerMuted"),
                transport = value.optString("transport"),
                quality = value.optString("quality", "good"),
                rttMs = value.optLong("rttMs"),
                localLevel = value.optDouble("localLevel", 0.0).toFloat(),
                peerLevel = value.optDouble("peerLevel", 0.0).toFloat(),
                safetyCode = List(code?.length() ?: 0) { code!!.optString(it) },
            )
        }.getOrNull()

        /** Заглушка на время, пока ядро создаёт комнату: экран открывается сразу по нажатию. */
        fun dialing(peerUserId: String, peerName: String, avatar: String?) = CallState(
            callId = "",
            peerUserId = peerUserId,
            peerName = peerName,
            peerAvatarBase64 = avatar,
            outgoing = true,
            phase = CallPhase.Calling,
            endReason = null,
            durationMs = 0,
            muted = false,
            peerMuted = false,
            transport = "",
            quality = "good",
            rttMs = 0,
            localLevel = 0f,
            peerLevel = 0f,
            safetyCode = emptyList(),
        )
    }
}

/** «1:05» или «1:02:03». */
fun formatCallDuration(milliseconds: Long): String {
    val total = (milliseconds / 1000).coerceAtLeast(0)
    val hours = total / 3600
    val minutes = (total % 3600) / 60
    val seconds = total % 60
    return if (hours > 0) "%d:%02d:%02d".format(hours, minutes, seconds) else "%d:%02d".format(minutes, seconds)
}

/** Подпись под именем: что сейчас происходит со звонком. */
fun CallState.statusLine(): String = when (phase) {
    CallPhase.Calling -> "Вызов…"
    CallPhase.Ringing -> "Звонит…"
    CallPhase.Incoming -> "Входящий звонок…"
    CallPhase.Connecting -> "Соединение…"
    CallPhase.Active -> formatCallDuration(durationMs)
    CallPhase.Ended -> endLine()
}

fun CallState.endLine(): String = when (endReason) {
    "declined_remote" -> "Звонок отклонён"
    "busy" -> "Собеседник занят"
    "no_answer" -> "Нет ответа"
    "missed", "cancelled_remote" -> "Звонок отменён"
    "declined" -> "Вы отклонили звонок"
    "cancelled" -> "Звонок отменён"
    "answered_elsewhere" -> "Принят на другом устройстве"
    "failed" -> "Не удалось соединиться"
    "lost" -> if (durationMs > 0) "Связь прервалась · ${formatCallDuration(durationMs)}" else "Не удалось соединиться"
    else -> if (durationMs > 0) "Звонок завершён · ${formatCallDuration(durationMs)}" else "Звонок завершён"
}
