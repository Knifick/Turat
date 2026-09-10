package app.turattext.mobile.model

import org.json.JSONArray
import org.json.JSONObject

data class Identity(val userId: String = "", val deviceId: String = "", val isAuthority: Boolean = false)
data class Profile(val username: String = "", val displayName: String = "", val about: String = "", val avatarBase64: String? = null)
data class Contact(
    val userId: String, val displayName: String, val username: String?, val about: String?,
    val avatarBase64: String?, val verified: Boolean, val pending: Boolean, val lastSeen: Long?,
    val pinned: Boolean = false, val muted: Boolean = false, val draft: String = "",
    val manualUnread: Boolean = false,
)

/** Строка списка чатов: контакт плюс превью последнего события и непрочитанные. */
data class Chat(
    val contact: Contact,
    val preview: String,
    val lastActivity: Long,
    val hasLastMessage: Boolean,
    val outgoing: Boolean,
    val delivered: Boolean,
    val read: Boolean,
    val unreadCount: Int,
)

data class Attachment(val id: String, val fileName: String, val mimeType: String, val size: Long, val localPath: String)
data class Message(
    val eventId: String, val senderUserId: String, val text: String, val createdAt: Long,
    val outgoing: Boolean, val edited: Boolean, val deleted: Boolean, val reactions: List<String>,
    val delivered: Boolean, val read: Boolean, val attachment: Attachment?,
    val replyToEventId: String? = null, val forwardedFrom: String? = null,
)

/** Найденное сообщение в глобальном поиске. */
data class SearchHit(
    val eventId: String, val userId: String, val displayName: String, val avatarBase64: String?,
    val text: String, val createdAt: Long, val outgoing: Boolean,
)
data class Settings(
    val bootstrapUrl: String = "", val metadataProtection: String = "balanced",
    val publishPresence: Boolean = false,
)
data class AppSnapshot(
    val identity: Identity = Identity(), val profile: Profile = Profile(), val chats: List<Chat> = emptyList(),
    val selectedContactId: String? = null, val messages: List<Message> = emptyList(), val settings: Settings = Settings(),
    val online: Boolean = false, val statusMessage: String = "", val onboardingRequired: Boolean = true,
    val searchQuery: String = "", val searchResults: List<SearchHit> = emptyList(),
) {
    val selectedChat get() = chats.firstOrNull { it.contact.userId == selectedContactId }
    val selectedContact get() = selectedChat?.contact
    val unreadTotal get() = chats.sumOf { it.unreadCount }
}

data class CoreResult(val ok: Boolean, val snapshot: AppSnapshot?, val error: String?)

object CoreJson {
    fun command(name: String, vararg fields: Pair<String, Any?>): String = JSONObject().apply {
        put("command", name)
        fields.forEach { (key, value) -> put(key, value ?: JSONObject.NULL) }
    }.toString()

    fun parse(raw: String): CoreResult {
        val root = JSONObject(raw)
        return CoreResult(root.optBoolean("ok"), root.optJSONObject("snapshot")?.let(::snapshot), root.optStringOrNull("error"))
    }

    private fun snapshot(value: JSONObject) = AppSnapshot(
        identity = value.getJSONObject("identity").let { Identity(it.getString("userId"), it.getString("deviceId"), it.optBoolean("isAuthority")) },
        profile = value.getJSONObject("profile").let { Profile(it.optString("username"), it.optString("displayName"), it.optString("about"), it.optStringOrNull("avatarBase64")) },
        chats = value.getJSONArray("chats").objects().map(::chat),
        selectedContactId = value.optStringOrNull("selectedContactId"),
        messages = value.getJSONArray("messages").objects().map {
            Message(it.getString("eventId"), it.getString("senderUserId"), it.optString("text"), it.getLong("createdAtUnixMilliseconds"),
                it.getBoolean("outgoing"), it.getBoolean("edited"), it.getBoolean("deleted"), it.getJSONArray("reactions").strings(),
                it.getBoolean("delivered"), it.getBoolean("read"), it.optJSONObject("attachment")?.let { a ->
                    Attachment(a.getString("attachmentId"), a.getString("fileName"), a.getString("mimeType"), a.getLong("size"), a.getString("localPath"))
                },
                it.optStringOrNull("replyToEventId"), it.optStringOrNull("forwardedFrom"))
        },
        settings = value.getJSONObject("settings").let {
            Settings(it.getString("bootstrapUrl"), it.getString("metadataProtection"), it.optBoolean("publishPresence"))
        },
        online = value.getBoolean("online"), statusMessage = value.optString("statusMessage"),
        onboardingRequired = value.getBoolean("onboardingRequired"),
        searchQuery = value.optString("searchQuery"),
        searchResults = value.optJSONArray("searchResults")?.objects().orEmpty().map {
            SearchHit(
                it.getString("eventId"), it.getString("userId"), it.getString("displayName"),
                it.optStringOrNull("avatarBase64"), it.optString("text"),
                it.getLong("createdAtUnixMilliseconds"), it.getBoolean("outgoing"),
            )
        },
    )

    private fun chat(value: JSONObject) = Chat(
        contact = Contact(
            value.getString("userId"), value.getString("displayName"), value.optStringOrNull("username"),
            value.optStringOrNull("about"), value.optStringOrNull("avatarBase64"), value.optBoolean("fingerprintVerified"),
            value.optBoolean("pendingApproval"), value.optLongOrNull("lastSeenUnixMilliseconds"),
            value.optBoolean("pinned"), value.optBoolean("muted"), value.optString("draft"),
            value.optBoolean("manualUnread"),
        ),
        preview = value.optString("preview"),
        lastActivity = value.optLong("lastActivityUnixMilliseconds"),
        hasLastMessage = value.optBoolean("hasLastMessage"),
        outgoing = value.optBoolean("lastMessageOutgoing"),
        delivered = value.optBoolean("lastMessageDelivered"),
        read = value.optBoolean("lastMessageRead"),
        unreadCount = value.optInt("unreadCount"),
    )

    private fun JSONArray.objects() = (0 until length()).map(::getJSONObject)
    private fun JSONArray.strings() = (0 until length()).map(::getString)
    private fun JSONObject.optStringOrNull(name: String) = if (isNull(name) || !has(name)) null else getString(name)
    private fun JSONObject.optLongOrNull(name: String) = if (isNull(name) || !has(name)) null else getLong(name)
}
