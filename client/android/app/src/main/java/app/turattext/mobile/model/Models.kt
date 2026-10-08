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
    /** Строка — группа: `contact.userId` тогда содержит GroupID. */
    val isGroup: Boolean = false,
    val memberCount: Int = 0,
    val groupRole: String? = null,
    val groupLeft: Boolean = false,
    /** Строка — канал: `contact.userId` содержит ChannelID, `memberCount` — подписчики. */
    val isChannel: Boolean = false,
    val channelRole: String? = null,
    val channelCanPost: Boolean = false,
) {
    /** Писать можно: диалог принят, из группы не вышли, а в канале есть право публикации. */
    val canWrite get() = !contact.pending && !groupLeft && (!isChannel || channelCanPost)
}

/** Права администратора канала — те же семь, что в Telegram. */
data class ChannelRights(
    val postMessages: Boolean = false,
    val editMessages: Boolean = false,
    val deleteMessages: Boolean = false,
    val inviteUsers: Boolean = false,
    val changeInfo: Boolean = false,
    val banUsers: Boolean = false,
    val addAdmins: Boolean = false,
) {
    fun toJson(): JSONObject = JSONObject().apply {
        put("postMessages", postMessages)
        put("editMessages", editMessages)
        put("deleteMessages", deleteMessages)
        put("inviteUsers", inviteUsers)
        put("changeInfo", changeInfo)
        put("banUsers", banUsers)
        put("addAdmins", addAdmins)
    }

    companion object {
        /** Права нового администратора по умолчанию: публиковать и править посты. */
        val Default = ChannelRights(postMessages = true, editMessages = true, deleteMessages = true)

        fun parse(value: JSONObject?) = ChannelRights(
            postMessages = value?.optBoolean("postMessages") == true,
            editMessages = value?.optBoolean("editMessages") == true,
            deleteMessages = value?.optBoolean("deleteMessages") == true,
            inviteUsers = value?.optBoolean("inviteUsers") == true,
            changeInfo = value?.optBoolean("changeInfo") == true,
            banUsers = value?.optBoolean("banUsers") == true,
            addAdmins = value?.optBoolean("addAdmins") == true,
        )
    }
}

data class ChannelAdmin(
    val userId: String,
    val displayName: String,
    val avatarBase64: String?,
    val role: String,
    val rights: ChannelRights,
    val title: String,
    val isSelf: Boolean,
    val addedByName: String,
    val canEdit: Boolean,
)

data class ChannelSubscriber(
    val userId: String,
    val displayName: String,
    val avatarBase64: String?,
    val isContact: Boolean,
    val banned: Boolean,
    val subscribedAt: Long,
)

/** Карточка открытого канала и права текущего пользователя в нём — их считает ядро. */
data class ChannelInfo(
    val channelId: String,
    val name: String,
    val about: String,
    val avatarBase64: String?,
    val epoch: Long,
    val myRole: String?,
    val myRights: ChannelRights,
    val awaitingState: Boolean,
    val pendingInvite: Boolean,
    val invitedByName: String?,
    val left: Boolean,
    val removed: Boolean,
    val closed: Boolean,
    val signPosts: Boolean,
    val commentsEnabled: Boolean,
    val discussionGroupId: String?,
    val discussionGroupName: String,
    val discussionJoined: Boolean,
    val subscriberCount: Int,
    val admins: List<ChannelAdmin>,
    val subscribers: List<ChannelSubscriber>,
    val inviteLink: String?,
    val canPost: Boolean,
    val canEditInfo: Boolean,
    val canInvite: Boolean,
    val canBan: Boolean,
    val canAddAdmins: Boolean,
    val canDeleteMessages: Boolean,
    val canEditMessages: Boolean,
    val canComment: Boolean,
    val canReact: Boolean,
    val threadPostEventId: String?,
) {
    val isAdmin get() = myRole != null
    val active get() = !awaitingState && !pendingInvite && !left && !removed && !closed
}

data class ReactionCount(val reaction: String, val count: Int, val mine: Boolean)

/** Просмотры, комментарии и реакции поста канала. */
data class ChannelPostInfo(val views: Int, val comments: Int, val reactions: List<ReactionCount>)

data class GroupMember(
    val userId: String,
    val displayName: String,
    val avatarBase64: String?,
    val role: String,
    val isSelf: Boolean,
    val isContact: Boolean,
    val confirmed: Boolean,
    val addedByName: String,
) {
    val rank get() = roleRank(role)
}

fun roleRank(role: String?): Int = when (role) {
    "owner" -> 2
    "admin" -> 1
    "member" -> 0
    else -> -1
}

/** Карточка открытой группы и права текущего пользователя в ней — их считает ядро. */
data class GroupInfo(
    val groupId: String,
    val name: String,
    val about: String,
    val avatarBase64: String?,
    val epoch: Long,
    val myRole: String?,
    val pendingInvite: Boolean,
    val invitedByName: String?,
    val left: Boolean,
    val membersCanInvite: Boolean,
    val membersCanEditInfo: Boolean,
    val members: List<GroupMember>,
    val canSend: Boolean,
    val canInvite: Boolean,
    val canEditInfo: Boolean,
    val canRemoveMembers: Boolean,
    val canManageAdmins: Boolean,
    val canDeleteMessages: Boolean,
)

/** Категория вложения: от неё зависит, рисует ли пузырь превью, плеер или карточку файла. */
enum class MediaKind { Image, Video, Audio, File;
    companion object {
        fun parse(value: String?, mimeType: String): MediaKind = when (value) {
            "image" -> Image
            "video" -> Video
            "audio" -> Audio
            "file" -> File
            else -> when {
                mimeType.startsWith("image/") -> Image
                mimeType.startsWith("video/") -> Video
                mimeType.startsWith("audio/") -> Audio
                else -> File
            }
        }
    }
}

data class Attachment(
    val id: String, val fileName: String, val mimeType: String, val size: Long, val localPath: String,
    val kind: MediaKind = MediaKind.File, val width: Int = 0, val height: Int = 0,
    val durationMilliseconds: Long = 0, val thumbnailBase64: String? = null,
) {
    /** Соотношение сторон превью; для неизвестного размера — почти квадрат, как в Telegram. */
    val aspectRatio: Float
        get() = if (width > 0 && height > 0) (width.toFloat() / height).coerceIn(0.5f, 1.9f) else 1.2f
}
data class Message(
    val eventId: String, val senderUserId: String, val text: String, val createdAt: Long,
    val outgoing: Boolean, val edited: Boolean, val deleted: Boolean, val reactions: List<String>,
    val delivered: Boolean, val read: Boolean, val attachment: Attachment?,
    val replyToEventId: String? = null, val forwardedFrom: String? = null,
    /** Служебная отметка группы: рисуется по центру, действий с ней нет. */
    val service: Boolean = false,
    /** Имя автора в группе; у своих сообщений и в личных диалогах пусто. */
    val senderName: String? = null,
    /** Счётчики поста канала; у обычных сообщений пусто. */
    val channelPost: ChannelPostInfo? = null,
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

/** Устройство аккаунта в списке сеансов. */
data class AccountDevice(val deviceId: String, val name: String, val current: Boolean, val addedAt: Long)

/**
 * Учётная запись. `state`: `none` — устройство чистое, нужен вход или регистрация; `legacy` —
 * переписка есть, а аккаунта ещё нет; `active` — вход выполнен.
 */
data class AccountView(
    val state: String = "none",
    val username: String = "",
    val node: String = "",
    /** Ключ восстановления, который надо показать один раз. */
    val recoveryKey: String? = null,
    val usernameConflict: Boolean = false,
    val notice: String? = null,
    val devices: List<AccountDevice> = emptyList(),
) {
    val signedIn get() = state == "active"
}
data class AppSnapshot(
    val identity: Identity = Identity(), val profile: Profile = Profile(), val chats: List<Chat> = emptyList(),
    val selectedContactId: String? = null, val messages: List<Message> = emptyList(), val settings: Settings = Settings(),
    val online: Boolean = false, val statusMessage: String = "", val onboardingRequired: Boolean = true,
    val searchQuery: String = "", val searchResults: List<SearchHit> = emptyList(),
    val group: GroupInfo? = null,
    val channel: ChannelInfo? = null,
    /** Комментарии открытого поста канала (`channel.threadPostEventId`). */
    val comments: List<Message> = emptyList(),
    val account: AccountView = AccountView(),
) {
    val selectedChat get() = chats.firstOrNull { it.contact.userId == selectedContactId }
    val selectedContact get() = selectedChat?.contact
    val unreadTotal get() = chats.sumOf { it.unreadCount }
}

/**
 * Ответ ядра. `value` заполнен у команд, которые возвращают не состояние, а результат —
 * например идентификатор фоновой передачи или её прогресс.
 */
data class CoreResult(
    val ok: Boolean,
    val snapshot: AppSnapshot?,
    val error: String?,
    val value: JSONObject? = null,
)

/** Вложение, которое прямо сейчас шифруется перед отправкой. */
data class PendingUpload(
    val jobId: String,
    val userId: String,
    val attachment: Attachment,
    val caption: String,
    val createdAt: Long,
    val done: Long = 0,
    val total: Long = 0,
    val failed: Boolean = false,
    val error: String = "",
    /** Фаза сжатия: задачи в ядре ещё нет, прогресс идёт в процентах от перекодирования. */
    val compressing: Boolean = false,
)

object CoreJson {
    fun command(name: String, vararg fields: Pair<String, Any?>): String = JSONObject().apply {
        put("command", name)
        fields.forEach { (key, value) -> put(key, value ?: JSONObject.NULL) }
    }.toString()

    fun parse(raw: String): CoreResult {
        val root = JSONObject(raw)
        return CoreResult(
            root.optBoolean("ok"),
            root.optJSONObject("snapshot")?.let(::snapshot),
            root.optStringOrNull("error"),
            root.optJSONObject("value"),
        )
    }

    private fun snapshot(value: JSONObject) = AppSnapshot(
        identity = value.getJSONObject("identity").let { Identity(it.getString("userId"), it.getString("deviceId"), it.optBoolean("isAuthority")) },
        profile = value.getJSONObject("profile").let { Profile(it.optString("username"), it.optString("displayName"), it.optString("about"), it.optStringOrNull("avatarBase64")) },
        chats = value.getJSONArray("chats").objects().map(::chat),
        selectedContactId = value.optStringOrNull("selectedContactId"),
        messages = value.getJSONArray("messages").objects().map(::message),
        group = value.optJSONObject("group")?.let(::group),
        channel = value.optJSONObject("channel")?.let(::channel),
        comments = value.optJSONArray("comments")?.objects().orEmpty().map(::message),
        settings = value.getJSONObject("settings").let {
            Settings(it.getString("bootstrapUrl"), it.getString("metadataProtection"), it.optBoolean("publishPresence"))
        },
        online = value.getBoolean("online"), statusMessage = value.optString("statusMessage"),
        onboardingRequired = value.getBoolean("onboardingRequired"),
        account = value.optJSONObject("account")?.let(::account) ?: AccountView(),
        searchQuery = value.optString("searchQuery"),
        searchResults = value.optJSONArray("searchResults")?.objects().orEmpty().map {
            SearchHit(
                it.getString("eventId"), it.getString("userId"), it.getString("displayName"),
                it.optStringOrNull("avatarBase64"), it.optString("text"),
                it.getLong("createdAtUnixMilliseconds"), it.getBoolean("outgoing"),
            )
        },
    )

    private fun account(it: JSONObject) = AccountView(
        state = it.optString("state", "none"),
        username = it.optString("username"),
        node = it.optString("node"),
        recoveryKey = it.optStringOrNull("recoveryKey"),
        usernameConflict = it.optBoolean("usernameConflict"),
        notice = it.optStringOrNull("notice"),
        devices = it.optJSONArray("devices")?.objects().orEmpty().map { device ->
            AccountDevice(
                device.getString("deviceId"),
                device.optString("name"),
                device.optBoolean("current"),
                device.optLong("addedAtUnixMilliseconds"),
            )
        },
    )

    private fun message(it: JSONObject) = Message(
        it.getString("eventId"), it.getString("senderUserId"), it.optString("text"), it.getLong("createdAtUnixMilliseconds"),
        it.getBoolean("outgoing"), it.getBoolean("edited"), it.getBoolean("deleted"), it.getJSONArray("reactions").strings(),
        it.getBoolean("delivered"), it.getBoolean("read"), it.optJSONObject("attachment")?.let { a ->
            val mime = a.getString("mimeType")
            Attachment(
                a.getString("attachmentId"), a.getString("fileName"), mime, a.getLong("size"),
                a.getString("localPath"), MediaKind.parse(a.optStringOrNull("kind"), mime),
                a.optInt("width"), a.optInt("height"), a.optLong("durationMilliseconds"),
                a.optStringOrNull("thumbnailBase64"),
            )
        },
        it.optStringOrNull("replyToEventId"), it.optStringOrNull("forwardedFrom"),
        it.optBoolean("service"), it.optStringOrNull("senderName"),
        it.optJSONObject("channelPost")?.let { post ->
            ChannelPostInfo(
                post.optInt("views"),
                post.optInt("comments"),
                post.optJSONArray("reactions")?.objects().orEmpty().map { r ->
                    ReactionCount(r.getString("reaction"), r.optInt("count"), r.optBoolean("mine"))
                },
            )
        },
    )

    private fun channel(value: JSONObject): ChannelInfo {
        val settings = value.optJSONObject("settings")
        return ChannelInfo(
            channelId = value.getString("channelId"),
            name = value.optString("name"),
            about = value.optString("about"),
            avatarBase64 = value.optStringOrNull("avatarBase64"),
            epoch = value.optLong("epoch"),
            myRole = value.optStringOrNull("myRole"),
            myRights = ChannelRights.parse(value.optJSONObject("myRights")),
            awaitingState = value.optBoolean("awaitingState"),
            pendingInvite = value.optBoolean("pendingInvite"),
            invitedByName = value.optStringOrNull("invitedByName"),
            left = value.optBoolean("left"),
            removed = value.optBoolean("removed"),
            closed = value.optBoolean("closed"),
            signPosts = settings?.optBoolean("signPosts") == true,
            commentsEnabled = settings?.optBoolean("commentsEnabled") == true,
            discussionGroupId = settings?.optStringOrNull("discussionGroupId"),
            discussionGroupName = settings?.optString("discussionGroupName").orEmpty(),
            discussionJoined = value.optBoolean("discussionJoined"),
            subscriberCount = value.optInt("subscriberCount"),
            admins = value.optJSONArray("admins")?.objects().orEmpty().map {
                ChannelAdmin(
                    it.getString("userId"), it.optString("displayName"), it.optStringOrNull("avatarBase64"),
                    it.optString("role"), ChannelRights.parse(it.optJSONObject("rights")), it.optString("title"),
                    it.optBoolean("isSelf"), it.optString("addedByName"), it.optBoolean("canEdit"),
                )
            },
            subscribers = value.optJSONArray("subscribers")?.objects().orEmpty().map {
                ChannelSubscriber(
                    it.getString("userId"), it.optString("displayName"), it.optStringOrNull("avatarBase64"),
                    it.optBoolean("isContact"), it.optBoolean("banned"), it.optLong("subscribedAtUnixMilliseconds"),
                )
            },
            inviteLink = value.optStringOrNull("inviteLink"),
            canPost = value.optBoolean("canPost"),
            canEditInfo = value.optBoolean("canEditInfo"),
            canInvite = value.optBoolean("canInvite"),
            canBan = value.optBoolean("canBan"),
            canAddAdmins = value.optBoolean("canAddAdmins"),
            canDeleteMessages = value.optBoolean("canDeleteMessages"),
            canEditMessages = value.optBoolean("canEditMessages"),
            canComment = value.optBoolean("canComment"),
            canReact = value.optBoolean("canReact"),
            threadPostEventId = value.optStringOrNull("threadPostEventId"),
        )
    }

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
        isGroup = value.optBoolean("isGroup"),
        memberCount = value.optInt("memberCount"),
        groupRole = value.optStringOrNull("groupRole"),
        groupLeft = value.optBoolean("groupLeft"),
        isChannel = value.optBoolean("isChannel"),
        channelRole = value.optStringOrNull("channelRole"),
        channelCanPost = value.optBoolean("channelCanPost"),
    )

    private fun group(value: JSONObject): GroupInfo {
        val permissions = value.optJSONObject("permissions")
        return GroupInfo(
            groupId = value.getString("groupId"),
            name = value.optString("name"),
            about = value.optString("about"),
            avatarBase64 = value.optStringOrNull("avatarBase64"),
            epoch = value.optLong("epoch"),
            myRole = value.optStringOrNull("myRole"),
            pendingInvite = value.optBoolean("pendingInvite"),
            invitedByName = value.optStringOrNull("invitedByName"),
            left = value.optBoolean("left"),
            membersCanInvite = permissions?.optBoolean("membersCanInvite") == true,
            membersCanEditInfo = permissions?.optBoolean("membersCanEditInfo") == true,
            members = value.optJSONArray("members")?.objects().orEmpty().map {
                GroupMember(
                    it.getString("userId"), it.optString("displayName"), it.optStringOrNull("avatarBase64"),
                    it.optString("role"), it.optBoolean("isSelf"), it.optBoolean("isContact"),
                    it.optBoolean("confirmed"), it.optString("addedByName"),
                )
            },
            canSend = value.optBoolean("canSend"),
            canInvite = value.optBoolean("canInvite"),
            canEditInfo = value.optBoolean("canEditInfo"),
            canRemoveMembers = value.optBoolean("canRemoveMembers"),
            canManageAdmins = value.optBoolean("canManageAdmins"),
            canDeleteMessages = value.optBoolean("canDeleteMessages"),
        )
    }

    private fun JSONArray.objects() = (0 until length()).map(::getJSONObject)
    private fun JSONArray.strings() = (0 until length()).map(::getString)
    private fun JSONObject.optStringOrNull(name: String) = if (isNull(name) || !has(name)) null else getString(name)
    private fun JSONObject.optLongOrNull(name: String) = if (isNull(name) || !has(name)) null else getLong(name)
}
