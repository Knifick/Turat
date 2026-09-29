package app.turattext.mobile.ui

import android.content.Intent
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.ChannelAdmin
import app.turattext.mobile.model.ChannelInfo
import app.turattext.mobile.model.ChannelPostInfo
import app.turattext.mobile.model.ChannelRights
import app.turattext.mobile.model.ChannelSubscriber
import app.turattext.mobile.model.Chat
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.Message
import org.json.JSONArray

/** Реакции, которые понимает ядро: чужую оно просто не применит. */
val ChannelReactions = listOf("❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔")

/** «1 подписчик», «3 подписчика», «11 подписчиков». */
fun subscribersLabel(count: Int): String {
    val tens = count % 100
    val units = count % 10
    val word = when {
        tens in 11..14 -> "подписчиков"
        units == 1 -> "подписчик"
        units in 2..4 -> "подписчика"
        else -> "подписчиков"
    }
    return "$count $word"
}

/** «1 комментарий», «2 комментария», «5 комментариев». */
fun commentsLabel(count: Int): String {
    if (count == 0) return "Прокомментировать"
    val tens = count % 100
    val units = count % 10
    val word = when {
        tens in 11..14 -> "комментариев"
        units == 1 -> "комментарий"
        units in 2..4 -> "комментария"
        else -> "комментариев"
    }
    return "$count $word"
}

/** Компактное число просмотров, как в Telegram: 950, 1,2K, 3,4M. */
fun compactCount(value: Int): String = when {
    value >= 1_000_000 -> String.format("%.1fM", value / 1_000_000f).replace('.', ',')
    value >= 10_000 -> "${value / 1000}K"
    value >= 1_000 -> String.format("%.1fK", value / 1000f).replace('.', ',')
    else -> value.toString()
}

/** Подпись под названием канала в шапке и списке чатов. */
fun channelSubtitle(chat: Chat?, channel: ChannelInfo?): String = when {
    channel?.closed == true -> "канал удалён"
    channel?.removed == true -> "вас удалили из канала"
    channel?.awaitingState == true -> "ждём ответа администратора"
    channel?.pendingInvite == true || chat?.contact?.pending == true -> "приглашение в канал"
    chat?.groupLeft == true -> "вы отписались"
    else -> subscribersLabel(channel?.subscriberCount ?: chat?.memberCount ?: 0)
}

private data class ChannelConfirmation(val title: String, val text: String, val action: () -> Unit)

@Composable
private fun ChannelConfirmationDialog(confirmation: ChannelConfirmation?, onDismiss: () -> Unit) {
    confirmation ?: return
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(confirmation.title) },
        text = { Text(confirmation.text) },
        confirmButton = {
            TextButton(onClick = { onDismiss(); confirmation.action() }) {
                Text("Да", color = Telegram.colors.danger)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Отмена") } },
    )
}

/** Новый канал и подписка по ссылке — на одном экране, как «Новый канал» в Telegram. */
@Composable
fun NewChannelScreen(
    busy: Boolean,
    error: String?,
    onBack: () -> Unit,
    onCreate: (String, String) -> Unit,
    onSubscribe: (String) -> Unit,
) {
    val colors = Telegram.colors
    var name by remember { mutableStateOf("") }
    var about by remember { mutableStateOf("") }
    var link by remember { mutableStateOf("") }
    TelegramScreen("Новый канал", onBack) {
        item {
            Text(
                "Канал — это лента публикаций для подписчиков. Писать в него могут только администраторы, " +
                    "подписчики читают, ставят реакции и комментируют. Подписчики не видят друг друга.",
                Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
                color = colors.hint,
                fontSize = 14.sp,
            )
        }
        item { TelegramField(name, { if (it.length <= 64) name = it }, "Название канала") }
        item {
            TelegramField(
                about,
                { if (it.length <= 255) about = it },
                "Описание (необязательно)",
                singleLine = false,
                imeAction = ImeAction.Done,
            )
        }
        item {
            TelegramButton(
                if (busy) "Создание…" else "Создать канал",
                { onCreate(name.trim(), about.trim()) },
                enabled = name.isNotBlank() && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        item { SectionTitle("Подписаться по ссылке") }
        item {
            Text(
                "Вставьте ссылку-приглашение вида turat://channel/… — запрос уйдёт администратору, " +
                    "и канал появится, когда он будет в сети.",
                Modifier.padding(horizontal = 18.dp),
                color = colors.hint,
                fontSize = 14.sp,
            )
        }
        item { TelegramField(link, { link = it.trim() }, "Ссылка на канал", imeAction = ImeAction.Done) }
        item {
            TelegramButton(
                if (busy) "Отправка…" else "Подписаться",
                { onSubscribe(link) },
                enabled = link.contains("ttch1-") && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        if (!error.isNullOrBlank()) {
            item { Text(error, Modifier.padding(horizontal = 18.dp), color = colors.danger, fontSize = 14.sp) }
        }
    }
}

/**
 * Страница канала: данные, ссылка-приглашение, настройки, администраторы и подписчики.
 * Права только прячут недоступное: решает ядро, а за ним — устройство каждого получателя.
 */
@Composable
fun ChannelInfoScreen(
    state: AppSnapshot,
    actions: AppActions,
    onBack: () -> Unit,
    onInvite: () -> Unit,
    onAddAdmin: () -> Unit,
    onEditAdmin: (String, String) -> Unit,
    onPickDiscussion: () -> Unit,
    onOpenChat: (String) -> Unit,
) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    val context = LocalContext.current
    val channel = state.channel ?: return
    var confirmation by remember { mutableStateOf<ChannelConfirmation?>(null) }
    var name by remember(channel.channelId, channel.epoch) { mutableStateOf(channel.name) }
    var about by remember(channel.channelId, channel.epoch) { mutableStateOf(channel.about) }

    fun command(name: String, vararg fields: Pair<String, Any?>) =
        actions.command(CoreJson.command(name, "channel_id" to channel.channelId, *fields), null)

    TelegramScreen("Канал", onBack) {
        item {
            Column(
                Modifier.fillMaxWidth().padding(14.dp)
                    .glass(colors, GlassShape.Panel, raised = true)
                    .padding(vertical = 20.dp, horizontal = 16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(contentAlignment = Alignment.BottomEnd) {
                    Avatar(
                        channel.name,
                        channel.channelId,
                        channel.avatarBase64,
                        96.dp,
                        modifier = if (channel.canEditInfo) {
                            Modifier.clickable { actions.pickGroupAvatar(channel.channelId) }
                        } else {
                            Modifier
                        },
                    )
                    if (channel.canEditInfo) {
                        Box(
                            Modifier.size(30.dp).clip(CircleShape).background(colors.accent)
                                .clickable { actions.pickGroupAvatar(channel.channelId) },
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(painterResource(R.drawable.ic_camera), "Сменить фото", Modifier.size(16.dp), colors.onAccent)
                        }
                    }
                }
                Spacer(Modifier.height(12.dp))
                Text(channel.name, color = colors.text, fontSize = 20.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center)
                Text(
                    channelSubtitle(null, channel) + when (channel.myRole) {
                        "owner" -> " · вы владелец"
                        "admin" -> " · вы администратор"
                        else -> ""
                    },
                    color = colors.hint,
                    fontSize = 14.sp,
                )
                if (channel.about.isNotBlank()) {
                    Spacer(Modifier.height(8.dp))
                    Text(channel.about, color = colors.text, fontSize = 14.sp, textAlign = TextAlign.Center)
                }
            }
        }

        channel.inviteLink?.let { link ->
            item { SectionTitle("Ссылка-приглашение") }
            item {
                SectionRow(R.drawable.ic_link, link, "Нажмите, чтобы скопировать. По ссылке подписка идёт через вас") {
                    clipboard.setText(AnnotatedString(link))
                    actions.command(CoreJson.command("snapshot"), null)
                }
            }
            item {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    SecondaryButton("Поделиться") {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain")
                            .putExtra(Intent.EXTRA_TEXT, "Подписывайтесь на канал «${channel.name}» в Turat: $link")
                        context.startActivity(Intent.createChooser(send, "Ссылка на канал"))
                    }
                    SecondaryButton("Пригласить контакты", onClick = onInvite)
                }
            }
        }

        if (channel.canEditInfo) {
            item { SectionTitle("Данные канала") }
            item { TelegramField(name, { if (it.length <= 64) name = it }, "Название") }
            item { TelegramField(about, { if (it.length <= 255) about = it }, "Описание", singleLine = false) }
            item {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    TelegramButton(
                        "Сохранить",
                        {
                            command(
                                "update_channel_info",
                                "name" to name.trim(),
                                "about" to about.trim(),
                                "avatar_base64" to channel.avatarBase64,
                            )
                        },
                        enabled = name.isNotBlank() && (name.trim() != channel.name || about.trim() != channel.about),
                    )
                    if (channel.avatarBase64 != null) {
                        SecondaryButton("Убрать фото") {
                            command("update_channel_info", "name" to channel.name, "about" to channel.about, "avatar_base64" to null)
                        }
                    }
                }
            }
            item { SectionTitle("Настройки") }
            item {
                SectionSwitch(
                    R.drawable.ic_edit,
                    "Подписывать посты",
                    if (channel.signPosts) "Под постом видно имя автора" else "Посты выходят от имени канала",
                    channel.signPosts,
                ) { checked ->
                    command("set_channel_settings", "sign_posts" to checked, "comments_enabled" to channel.commentsEnabled)
                }
            }
            item {
                SectionSwitch(
                    R.drawable.ic_comment,
                    "Комментарии",
                    if (channel.commentsEnabled) "Подписчики могут комментировать посты" else "Комментарии выключены",
                    channel.commentsEnabled,
                ) { checked ->
                    command("set_channel_settings", "sign_posts" to channel.signPosts, "comments_enabled" to checked)
                }
            }
        }

        item { SectionTitle("Обсуждение") }
        val discussion = channel.discussionGroupId
        if (discussion != null) {
            item {
                SectionRow(
                    R.drawable.ic_group,
                    channel.discussionGroupName.ifBlank { "Группа обсуждения" },
                    if (channel.discussionJoined) "Открыть группу — туда приходят все посты"
                    else "Вы не участник этой группы: попросите администратора добавить вас",
                ) { if (channel.discussionJoined) onOpenChat(discussion) }
            }
        } else {
            item {
                Text(
                    "Группа обсуждения не привязана. Комментарии под постами работают и без неё.",
                    Modifier.padding(horizontal = 18.dp, vertical = 4.dp),
                    color = colors.hint,
                    fontSize = 13.sp,
                )
            }
        }
        if (channel.canEditInfo) {
            item {
                SectionRow(
                    R.drawable.ic_link,
                    if (discussion == null) "Привязать группу" else "Сменить или отвязать группу",
                    "Посты будут автоматически пересылаться в неё",
                    onClick = onPickDiscussion,
                )
            }
        }

        if (channel.isAdmin) {
            item { SectionTitle("Администраторы · ${channel.admins.size}") }
            if (channel.canAddAdmins) {
                item { SectionRow(R.drawable.ic_add, "Добавить администратора", onClick = onAddAdmin) }
            }
            items(channel.admins, key = { "admin-" + it.userId }) { admin ->
                AdminRow(
                    channel = channel,
                    admin = admin,
                    onEdit = { onEditAdmin(admin.userId, admin.displayName) },
                    onDemote = {
                        confirmation = ChannelConfirmation(
                            "Разжаловать администратора?",
                            "${admin.displayName} останется подписчиком канала, но больше не сможет публиковать.",
                        ) { command("remove_channel_admin", "user_id" to admin.userId) }
                    },
                    onTransfer = {
                        confirmation = ChannelConfirmation(
                            "Передать канал?",
                            "${admin.displayName} станет владельцем канала, а вы — администратором со всеми правами.",
                        ) { command("transfer_channel_ownership", "user_id" to admin.userId) }
                    },
                )
            }

            item { SectionTitle(subscribersLabel(channel.subscriberCount).replaceFirstChar { it.uppercase() }) }
            if (channel.subscribers.isEmpty()) {
                item {
                    Text(
                        "Пока никого. Поделитесь ссылкой-приглашением или пригласите контакты.",
                        Modifier.padding(horizontal = 18.dp),
                        color = colors.hint,
                        fontSize = 13.sp,
                    )
                }
            }
            items(channel.subscribers, key = { "sub-" + it.userId }) { subscriber ->
                SubscriberRow(
                    channel = channel,
                    subscriber = subscriber,
                    onMakeAdmin = { onEditAdmin(subscriber.userId, subscriber.displayName) },
                    onRemove = { ban ->
                        confirmation = ChannelConfirmation(
                            if (ban) "Заблокировать подписчика?" else "Удалить подписчика?",
                            if (ban) "${subscriber.displayName} перестанет получать посты и не сможет подписаться снова."
                            else "${subscriber.displayName} перестанет получать посты, но сможет подписаться по ссылке.",
                        ) { command("remove_channel_subscriber", "user_id" to subscriber.userId, "ban" to ban) }
                    },
                    onUnban = { command("unban_channel_subscriber", "user_id" to subscriber.userId) },
                )
            }
        } else {
            item { SectionTitle("Администраторы") }
            items(channel.admins, key = { "admin-" + it.userId }) { admin ->
                AdminRow(channel, admin, onEdit = {}, onDemote = {}, onTransfer = {})
            }
        }

        item { SectionTitle("Безопасность") }
        item {
            Text(
                "Каждый пост шифруется отдельно для каждого подписчика его личным сквозным каналом. " +
                    "Права администраторов проверяет устройство каждого получателя. Комментарии подписчиков " +
                    "разносит администратор, но подделать их не может: подпись автора проверяется у всех.",
                Modifier.padding(horizontal = 18.dp, vertical = 4.dp),
                color = colors.hint,
                fontSize = 13.sp,
            )
        }
        item {
            SectionRow(R.drawable.ic_copy, shortId(channel.channelId), "ChannelID · нажмите, чтобы скопировать") {
                clipboard.setText(AnnotatedString(channel.channelId))
            }
        }

        item { SectionTitle("Канал") }
        if (channel.active && channel.myRole != "owner") {
            item {
                SectionRow(R.drawable.ic_close, if (channel.isAdmin) "Сложить полномочия и отписаться" else "Отписаться", danger = true) {
                    confirmation = ChannelConfirmation(
                        "Отписаться от канала?",
                        "История останется на этом устройстве, но новые посты приходить не будут.",
                    ) { command("leave_channel") }
                }
            }
        }
        if (channel.active && channel.myRole == "owner") {
            item {
                SectionRow(R.drawable.ic_delete, "Удалить канал у всех", danger = true) {
                    confirmation = ChannelConfirmation(
                        "Удалить канал?",
                        "Канал закроется у всех подписчиков: публиковать в него больше будет нельзя. Это необратимо.",
                    ) { command("close_channel") }
                }
            }
        }
        if (!channel.active || channel.myRole != "owner") {
            item {
                SectionRow(R.drawable.ic_delete, "Удалить из списка", danger = true) {
                    confirmation = ChannelConfirmation(
                        "Удалить канал из списка?",
                        if (channel.active) "Вы отпишетесь, а история канала будет удалена с этого устройства."
                        else "История канала будет удалена с этого устройства.",
                    ) {
                        onBack()
                        actions.deleteContact(channel.channelId)
                    }
                }
            }
        }
    }
    ChannelConfirmationDialog(confirmation) { confirmation = null }
}

private fun rightsSummary(admin: ChannelAdmin): String {
    if (admin.role == "owner") return "владелец"
    val rights = admin.rights
    val parts = buildList {
        if (rights.postMessages) add("публикует")
        if (rights.editMessages) add("правит")
        if (rights.deleteMessages) add("удаляет")
        if (rights.inviteUsers) add("приглашает")
        if (rights.changeInfo) add("меняет данные")
        if (rights.banUsers) add("блокирует")
        if (rights.addAdmins) add("назначает админов")
    }
    val title = admin.title.takeIf { it.isNotBlank() }?.let { "$it · " }.orEmpty()
    return title + if (parts.isEmpty()) "без прав" else parts.joinToString(", ")
}

@Composable
private fun AdminRow(
    channel: ChannelInfo,
    admin: ChannelAdmin,
    onEdit: () -> Unit,
    onDemote: () -> Unit,
    onTransfer: () -> Unit,
) {
    val colors = Telegram.colors
    var menu by remember { mutableStateOf(false) }
    val canTransfer = channel.myRole == "owner" && admin.role == "admin"
    Box {
        Row(
            Modifier.fillMaxWidth()
                .clickable(enabled = admin.canEdit || canTransfer) { menu = true }
                .padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(admin.displayName, admin.userId, admin.avatarBase64, 44.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    admin.displayName + if (admin.isSelf) " (вы)" else "",
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(rightsSummary(admin), color = colors.hint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Box(
                Modifier.clip(RoundedCornerShape(10.dp)).background(colors.accentSoft)
                    .padding(horizontal = 8.dp, vertical = 3.dp),
            ) {
                Text(if (admin.role == "owner") "владелец" else "админ", color = colors.text, fontSize = 12.sp, fontWeight = FontWeight.Medium)
            }
        }
        DropdownMenu(menu, { menu = false }) {
            if (admin.canEdit) {
                ChannelMenuItem("Изменить права") { menu = false; onEdit() }
            }
            if (canTransfer) {
                ChannelMenuItem("Передать канал") { menu = false; onTransfer() }
            }
            if (admin.canEdit) {
                ChannelMenuItem("Разжаловать", danger = true) { menu = false; onDemote() }
            }
        }
    }
}

@Composable
private fun SubscriberRow(
    channel: ChannelInfo,
    subscriber: ChannelSubscriber,
    onMakeAdmin: () -> Unit,
    onRemove: (Boolean) -> Unit,
    onUnban: () -> Unit,
) {
    val colors = Telegram.colors
    var menu by remember { mutableStateOf(false) }
    val actionable = channel.canBan || (channel.canAddAdmins && !subscriber.banned)
    Box {
        Row(
            Modifier.fillMaxWidth()
                .clickable(enabled = actionable) { menu = true }
                .padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(subscriber.displayName, subscriber.userId, subscriber.avatarBase64, 40.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    subscriber.displayName,
                    color = if (subscriber.banned) colors.hint else colors.text,
                    fontSize = 15.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    when {
                        subscriber.banned -> "заблокирован(а)"
                        subscriber.isContact -> "в ваших контактах · с ${dateSeparator(subscriber.subscribedAt)}"
                        else -> "подписан(а) с ${dateSeparator(subscriber.subscribedAt)}"
                    },
                    color = if (subscriber.banned) colors.danger.copy(alpha = .8f) else colors.hint,
                    fontSize = 12.sp,
                    maxLines = 1,
                )
            }
        }
        DropdownMenu(menu, { menu = false }) {
            if (channel.canAddAdmins && !subscriber.banned) {
                ChannelMenuItem("Назначить администратором") { menu = false; onMakeAdmin() }
            }
            if (channel.canBan && !subscriber.banned) {
                ChannelMenuItem("Удалить из канала", danger = true) { menu = false; onRemove(false) }
                ChannelMenuItem("Заблокировать", danger = true) { menu = false; onRemove(true) }
            }
            if (channel.canBan && subscriber.banned) {
                ChannelMenuItem("Разблокировать") { menu = false; onUnban() }
            }
        }
    }
}

@Composable
private fun ChannelMenuItem(title: String, danger: Boolean = false, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(title, color = if (danger) Telegram.colors.danger else Telegram.colors.text) },
        onClick = onClick,
    )
}

/** Права администратора: семь переключателей и подпись, как в Telegram. */
@Composable
fun AdminRightsScreen(
    state: AppSnapshot,
    userId: String,
    displayName: String,
    busy: Boolean,
    onBack: () -> Unit,
    onSave: (ChannelRights, String) -> Unit,
) {
    val colors = Telegram.colors
    val channel = state.channel ?: return
    val existing = channel.admins.firstOrNull { it.userId == userId }
    var rights by remember(userId, existing) { mutableStateOf(existing?.rights ?: ChannelRights.Default) }
    var title by remember(userId, existing) { mutableStateOf(existing?.title.orEmpty()) }
    val mine = channel.myRights
    val owner = channel.myRole == "owner"
    TelegramScreen(if (existing == null) "Новый администратор" else "Права администратора", onBack) {
        item {
            Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                Avatar(displayName, userId, existing?.avatarBase64, 52.dp)
                Spacer(Modifier.width(12.dp))
                Column {
                    Text(displayName, color = colors.text, fontSize = 17.sp, fontWeight = FontWeight.Medium)
                    Text(shortId(userId), color = colors.hint, fontSize = 13.sp)
                }
            }
        }
        item { SectionTitle("Что может этот администратор") }
        item {
            RightSwitch("Публиковать посты", rights.postMessages, owner || mine.postMessages) {
                rights = rights.copy(postMessages = it)
            }
        }
        item {
            RightSwitch("Редактировать чужие посты", rights.editMessages, owner || mine.editMessages) {
                rights = rights.copy(editMessages = it)
            }
        }
        item {
            RightSwitch("Удалять чужие посты и комментарии", rights.deleteMessages, owner || mine.deleteMessages) {
                rights = rights.copy(deleteMessages = it)
            }
        }
        item {
            RightSwitch("Приглашать подписчиков", rights.inviteUsers, owner || mine.inviteUsers) {
                rights = rights.copy(inviteUsers = it)
            }
        }
        item {
            RightSwitch("Менять данные и настройки канала", rights.changeInfo, owner || mine.changeInfo) {
                rights = rights.copy(changeInfo = it)
            }
        }
        item {
            RightSwitch("Удалять и блокировать подписчиков", rights.banUsers, owner || mine.banUsers) {
                rights = rights.copy(banUsers = it)
            }
        }
        item {
            RightSwitch("Назначать администраторов", rights.addAdmins, owner || mine.addAdmins) {
                rights = rights.copy(addAdmins = it)
            }
        }
        if (!owner) {
            item {
                Text(
                    "Выдать можно только те права, которые есть у вас самих.",
                    Modifier.padding(horizontal = 18.dp, vertical = 4.dp),
                    color = colors.hint,
                    fontSize = 13.sp,
                )
            }
        }
        item { SectionTitle("Подпись") }
        item { TelegramField(title, { if (it.length <= 16) title = it }, "Например, «Редактор»", imeAction = ImeAction.Done) }
        item {
            TelegramButton(
                if (busy) "Сохранение…" else if (existing == null) "Назначить" else "Сохранить",
                { onSave(rights, title.trim()) },
                enabled = !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 14.dp),
            )
        }
    }
}

@Composable
private fun RightSwitch(title: String, checked: Boolean, enabled: Boolean, onChange: (Boolean) -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth()
            .clickable(enabled = enabled) { onChange(!checked) }
            .padding(horizontal = 18.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, Modifier.weight(1f), color = if (enabled) colors.text else colors.hint, fontSize = 15.sp)
        androidx.compose.material3.Switch(
            checked = checked,
            onCheckedChange = onChange,
            enabled = enabled,
            colors = turatSwitchColors(),
        )
    }
}

/** Кого назначить администратором: подписчики канала и принятые контакты. */
@Composable
fun PickChannelAdminScreen(state: AppSnapshot, onBack: () -> Unit, onPick: (String, String) -> Unit) {
    val colors = Telegram.colors
    val channel = state.channel ?: return
    val admins = channel.admins.map { it.userId }.toSet()
    val candidates = buildList {
        channel.subscribers.filter { !it.banned && it.userId !in admins }
            .forEach { add(Triple(it.userId, it.displayName, it.avatarBase64)) }
        state.chats.filter { !it.isGroup && !it.isChannel && !it.contact.pending && it.contact.userId !in admins }
            .forEach { chat ->
                if (none { it.first == chat.contact.userId }) {
                    add(Triple(chat.contact.userId, chat.contact.displayName, chat.contact.avatarBase64))
                }
            }
    }
    TelegramScreen("Добавить администратора", onBack) {
        if (candidates.isEmpty()) {
            item {
                Text(
                    "Назначить можно подписчика канала или принятый контакт.",
                    Modifier.padding(18.dp),
                    color = colors.hint,
                    fontSize = 14.sp,
                )
            }
        }
        items(candidates, key = { it.first }) { (userId, name, avatar) ->
            Row(
                Modifier.fillMaxWidth().clickable { onPick(userId, name) }.padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(name, userId, avatar, 44.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(name, color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                    Text(
                        if (channel.subscribers.any { it.userId == userId }) "подписчик" else "контакт",
                        color = colors.hint,
                        fontSize = 13.sp,
                    )
                }
            }
        }
    }
}

/** Приглашение контактов: каждый получит запрос и решит сам. */
@Composable
fun InviteToChannelScreen(state: AppSnapshot, busy: Boolean, onBack: () -> Unit, onInvite: (List<String>) -> Unit) {
    val colors = Telegram.colors
    val channel = state.channel ?: return
    val present = (channel.subscribers.map { it.userId } + channel.admins.map { it.userId }).toSet()
    val candidates = state.chats.filter {
        !it.isGroup && !it.isChannel && !it.contact.pending && it.contact.userId !in present
    }
    val selected = remember(channel.channelId) { mutableStateListOf<String>() }
    TelegramScreen("Пригласить в канал", onBack) {
        item {
            TelegramButton(
                when {
                    busy -> "Отправка…"
                    selected.isEmpty() -> "Отметьте, кого пригласить"
                    else -> "Пригласить: ${selected.size}"
                },
                { onInvite(selected.toList()) },
                enabled = selected.isNotEmpty() && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        if (candidates.isEmpty()) {
            item {
                Text("Все ваши контакты уже подписаны.", Modifier.padding(horizontal = 18.dp), color = colors.hint, fontSize = 14.sp)
            }
        }
        items(candidates, key = { it.contact.userId }) { chat ->
            val id = chat.contact.userId
            val checked = id in selected
            Row(
                Modifier.fillMaxWidth()
                    .clickable { if (checked) selected.remove(id) else selected.add(id) }
                    .padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(chat.contact.displayName, id, chat.contact.avatarBase64, 46.dp)
                Spacer(Modifier.width(12.dp))
                Text(chat.contact.displayName, Modifier.weight(1f), color = colors.text, fontSize = 16.sp, maxLines = 1)
                Box(
                    Modifier.size(24.dp).clip(CircleShape)
                        .background(if (checked) colors.accent else Color.Transparent)
                        .border(1.5.dp, if (checked) colors.accent else colors.hint, CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    if (checked) Icon(painterResource(R.drawable.ic_check), null, Modifier.size(14.dp), colors.onAccent)
                }
            }
        }
    }
}

/** Выбор группы обсуждения из групп, где пользователь состоит. */
@Composable
fun DiscussionPickerScreen(state: AppSnapshot, onBack: () -> Unit, onPick: (String?) -> Unit) {
    val colors = Telegram.colors
    val channel = state.channel ?: return
    val groups = state.chats.filter { it.isGroup && it.canWrite }
    TelegramScreen("Группа обсуждения", onBack) {
        item {
            Text(
                "Все новые посты будут пересылаться в выбранную группу — там подписчики-участники " +
                    "смогут обсуждать их вместе. Привязать можно группу, в которой вы состоите.",
                Modifier.padding(18.dp),
                color = colors.hint,
                fontSize = 14.sp,
            )
        }
        if (channel.discussionGroupId != null) {
            item { SectionRow(R.drawable.ic_close, "Отвязать группу", danger = true) { onPick(null) } }
        }
        if (groups.isEmpty()) {
            item {
                Text("Сначала создайте группу.", Modifier.padding(horizontal = 18.dp), color = colors.hint, fontSize = 14.sp)
            }
        }
        items(groups, key = { it.contact.userId }) { chat ->
            Row(
                Modifier.fillMaxWidth().clickable { onPick(chat.contact.userId) }.padding(horizontal = 14.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(chat.contact.displayName, chat.contact.userId, chat.contact.avatarBase64, 44.dp)
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(chat.contact.displayName, color = colors.text, fontSize = 16.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                    Text(
                        if (chat.contact.userId == channel.discussionGroupId) "привязана сейчас" else membersLabel(chat.memberCount),
                        color = colors.hint,
                        fontSize = 13.sp,
                    )
                }
            }
        }
    }
}

/** Строка под постом: реакции, просмотры и кнопка комментариев. */
@Composable
fun ChannelPostBar(
    post: ChannelPostInfo,
    commentsEnabled: Boolean,
    canReact: Boolean,
    onComments: () -> Unit,
    onReact: (String) -> Unit,
) {
    val colors = Telegram.colors
    var picker by remember { mutableStateOf(false) }
    Column(Modifier.widthIn(max = 480.dp).padding(top = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            post.reactions.forEach { reaction ->
                Box(
                    Modifier.clip(RoundedCornerShape(12.dp))
                        .background(if (reaction.mine) colors.accent.copy(alpha = .30f) else colors.accent.copy(alpha = .12f))
                        .border(1.dp, if (reaction.mine) colors.accent else Color.Transparent, RoundedCornerShape(12.dp))
                        .clickable(enabled = canReact) { onReact(reaction.reaction) }
                        .padding(horizontal = 8.dp, vertical = 3.dp),
                ) {
                    Text("${reaction.reaction} ${reaction.count}", color = colors.text, fontSize = 13.sp)
                }
            }
            if (canReact) {
                Box {
                    Box(
                        Modifier.size(26.dp).clip(CircleShape)
                            .background(colors.accent.copy(alpha = .10f))
                            .clickable { picker = true },
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(painterResource(R.drawable.ic_reaction), "Реакция", Modifier.size(15.dp), colors.hint)
                    }
                    DropdownMenu(picker, { picker = false }) {
                        Row(Modifier.padding(horizontal = 6.dp)) {
                            ChannelReactions.forEach { reaction ->
                                Box(
                                    Modifier.size(38.dp).clip(CircleShape).clickable { picker = false; onReact(reaction) },
                                    contentAlignment = Alignment.Center,
                                ) { Text(reaction, fontSize = 20.sp) }
                            }
                        }
                    }
                }
            }
            Spacer(Modifier.weight(1f))
            Icon(painterResource(R.drawable.ic_eye), "Просмотры", Modifier.size(15.dp), colors.hint)
            Spacer(Modifier.width(3.dp))
            Text(compactCount(post.views), color = colors.hint, fontSize = 12.sp)
        }
        if (commentsEnabled || post.comments > 0) {
            Row(
                Modifier.padding(top = 4.dp).fillMaxWidth()
                    .clip(RoundedCornerShape(12.dp))
                    .glass(colors, RoundedCornerShape(12.dp))
                    .clickable(onClick = onComments)
                    .padding(horizontal = 10.dp, vertical = 7.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(painterResource(R.drawable.ic_comment), null, Modifier.size(17.dp), colors.accent)
                Spacer(Modifier.width(8.dp))
                Text(commentsLabel(post.comments), Modifier.weight(1f), color = colors.accent, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                Icon(painterResource(R.drawable.ic_forward), null, Modifier.size(14.dp), colors.hint)
            }
        }
    }
}

/** Нижняя панель канала для тех, кто не публикует: звук или состояние подписки. */
@Composable
fun ChannelFooter(chat: Chat?, channel: ChannelInfo?, actions: AppActions, onDelete: () -> Unit) {
    val colors = Telegram.colors
    val id = channel?.channelId ?: chat?.contact?.userId ?: return
    Column(Modifier.fillMaxWidth().glass(colors, GlassShape.Footer, raised = true).navigationBarsPadding()) {
        val closedText = when {
            channel?.closed == true -> "Канал удалён владельцем"
            channel?.removed == true -> "Администратор удалил вас из канала"
            channel?.left == true || chat?.groupLeft == true -> "Вы отписались от канала"
            channel?.awaitingState == true -> "Запрос на подписку отправлен — ждём администратора"
            else -> null
        }
        if (closedText != null) {
            Text(closedText, Modifier.fillMaxWidth().padding(top = 10.dp), color = colors.hint, fontSize = 13.sp, textAlign = TextAlign.Center)
            Box(Modifier.fillMaxWidth().height(48.dp).clickable(onClick = onDelete), contentAlignment = Alignment.Center) {
                Text(
                    if (channel?.awaitingState == true) "ОТМЕНИТЬ" else "УДАЛИТЬ КАНАЛ",
                    color = colors.danger,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                )
            }
        } else {
            val muted = chat?.contact?.muted == true
            Box(
                Modifier.fillMaxWidth().height(52.dp).clickable { actions.setMuted(id, !muted) },
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    if (muted) "ВКЛЮЧИТЬ ЗВУК" else "ВЫКЛЮЧИТЬ ЗВУК",
                    color = colors.accent,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                )
            }
        }
    }
}

/**
 * Комментарии к посту. Подписчики друг друга не знают, поэтому комментарий уходит
 * администратору, а тот разносит его остальным с подписью автора.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CommentsScreen(state: AppSnapshot, actions: AppActions, onBack: () -> Unit) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    val channel = state.channel ?: return
    val postId = channel.threadPostEventId ?: return
    val post = state.messages.firstOrNull { it.eventId == postId }
    val comments = state.comments
    var draft by remember(postId) { mutableStateOf("") }
    var replyTo by remember(postId) { mutableStateOf<Message?>(null) }
    var menuFor by remember(postId) { mutableStateOf<String?>(null) }
    val listState = rememberLazyListState()
    LaunchedEffect(postId, comments.size) {
        val count = comments.size + 1
        if (count > 0) listState.animateScrollToItem(count - 1)
    }

    fun send() {
        val text = draft.trim()
        if (text.isEmpty()) return
        actions.command(
            CoreJson.command(
                "send_comment",
                "post_event_id" to postId,
                "text" to text,
                "reply_to_event_id" to replyTo?.eventId,
            ),
            null,
        )
        draft = ""
        replyTo = null
    }

    Column(
        Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(colors.chatTop, colors.chatBottom))),
    ) {
        Column(Modifier.glass(colors, GlassShape.Header, raised = true).statusBarsPadding()) {
            Row(Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onBack) {
                    Icon(painterResource(R.drawable.ic_back), "Назад", Modifier.size(22.dp), colors.text)
                }
                Column(Modifier.weight(1f)) {
                    Text(commentsLabel(comments.size).takeIf { comments.isNotEmpty() } ?: "Комментарии",
                        color = colors.text, fontSize = 17.sp, fontWeight = FontWeight.Medium)
                    Text(channel.name, color = colors.hint, fontSize = 13.sp, maxLines = 1)
                }
            }
        }
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(horizontal = 10.dp, vertical = 10.dp),
        ) {
            item(key = "post") {
                Column(
                    Modifier.fillMaxWidth().padding(bottom = 10.dp)
                        .glass(colors, GlassShape.Panel, raised = true)
                        .padding(12.dp),
                ) {
                    Text(channel.name, color = colors.accent, fontSize = 14.sp, fontWeight = FontWeight.Medium)
                    Spacer(Modifier.height(2.dp))
                    Text(post?.let(::quoteOf) ?: "Пост", color = colors.text, fontSize = 15.sp, maxLines = 8, overflow = TextOverflow.Ellipsis)
                    post?.let { Text(timeOf(it.createdAt), color = colors.hint, fontSize = 11.sp, modifier = Modifier.align(Alignment.End)) }
                }
            }
            if (comments.isEmpty()) {
                item(key = "empty") {
                    Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) {
                        ServicePill(if (channel.canComment) "Будьте первым, кто оставит комментарий" else "Комментариев нет")
                    }
                }
            }
            items(comments, key = { it.eventId }) { comment ->
                val replied = comment.replyToEventId?.let { id -> comments.firstOrNull { it.eventId == id } }
                val canDelete = comment.outgoing || channel.canDeleteMessages
                Box(Modifier.fillMaxWidth().padding(vertical = 3.dp)) {
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (comment.outgoing) Arrangement.End else Arrangement.Start) {
                        Column(
                            Modifier.widthIn(max = 440.dp)
                                .clip(RoundedCornerShape(16.dp))
                                .background(
                                    if (comment.outgoing) colors.bubbleOut.copy(alpha = .9f) else colors.glassRaised,
                                )
                                .combinedClickable(onClick = {}, onLongClick = { menuFor = comment.eventId })
                                .padding(horizontal = 12.dp, vertical = 7.dp),
                        ) {
                            if (!comment.outgoing) {
                                Text(comment.senderName ?: shortId(comment.senderUserId), color = colors.accent,
                                    fontSize = 13.sp, fontWeight = FontWeight.Medium, maxLines = 1)
                            }
                            if (replied != null) {
                                Text(
                                    "↩ " + (if (replied.outgoing) "Вы" else replied.senderName ?: "") + ": " + replied.text,
                                    color = if (comment.outgoing) colors.bubbleOutMeta else colors.hint,
                                    fontSize = 12.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            Text(comment.text, color = if (comment.outgoing) colors.bubbleOutText else colors.bubbleInText, fontSize = 15.sp)
                            Text(
                                timeOf(comment.createdAt) + if (comment.outgoing && !comment.delivered) " · отправляется" else "",
                                color = if (comment.outgoing) colors.bubbleOutMeta else colors.bubbleInMeta,
                                fontSize = 11.sp,
                                modifier = Modifier.align(Alignment.End),
                            )
                        }
                    }
                    DropdownMenu(menuFor == comment.eventId, { menuFor = null }) {
                        if (channel.canComment) {
                            ChannelMenuItem("Ответить") { menuFor = null; replyTo = comment }
                        }
                        ChannelMenuItem("Копировать") {
                            menuFor = null
                            clipboard.setText(AnnotatedString(comment.text))
                        }
                        if (canDelete) {
                            ChannelMenuItem("Удалить", danger = true) {
                                menuFor = null
                                actions.deleteMessages(setOf(comment.eventId))
                            }
                        }
                    }
                }
            }
        }
        if (channel.canComment) {
            Column(Modifier.glass(colors, GlassShape.Footer, raised = true).imePadding()) {
                replyTo?.let { target ->
                    Row(Modifier.fillMaxWidth().padding(start = 14.dp, end = 4.dp, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Box(Modifier.width(2.dp).height(30.dp).background(colors.accent))
                        Spacer(Modifier.width(8.dp))
                        Column(Modifier.weight(1f)) {
                            Text(if (target.outgoing) "Вы" else target.senderName ?: "", color = colors.accent, fontSize = 13.sp, fontWeight = FontWeight.Medium)
                            Text(target.text, color = colors.hint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                        IconButton({ replyTo = null }) {
                            Icon(painterResource(R.drawable.ic_close), "Отменить", Modifier.size(18.dp), colors.hint)
                        }
                    }
                }
                Row(
                    Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 8.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.Bottom,
                ) {
                    Box(
                        Modifier.weight(1f).heightIn(min = 44.dp)
                            .clip(GlassShape.Input)
                            .glass(colors, GlassShape.Input)
                            .padding(horizontal = 14.dp),
                        contentAlignment = Alignment.CenterStart,
                    ) {
                        if (draft.isEmpty()) Text("Комментарий", color = colors.hint, fontSize = 16.sp)
                        BasicTextField(
                            value = draft,
                            onValueChange = { if (it.length <= 4096) draft = it },
                            textStyle = TextStyle(color = colors.text, fontSize = 16.sp),
                            cursorBrush = SolidColor(colors.accent),
                            maxLines = 6,
                            modifier = Modifier.fillMaxWidth().padding(vertical = 10.dp),
                        )
                    }
                    Spacer(Modifier.width(6.dp))
                    Box(
                        Modifier.size(44.dp).clip(CircleShape)
                            .background(if (draft.isBlank()) colors.accent.copy(alpha = .35f) else colors.accent.copy(alpha = .92f))
                            .clickable(enabled = draft.isNotBlank(), onClick = ::send),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(painterResource(R.drawable.ic_send), "Отправить", Modifier.size(21.dp), colors.onAccent)
                    }
                }
            }
        } else {
            Text(
                if (channel.active) "Комментарии в этом канале выключены" else "Комментировать могут только подписчики",
                Modifier.fillMaxWidth().glass(colors, GlassShape.Footer, raised = true).navigationBarsPadding().padding(14.dp),
                color = colors.hint,
                fontSize = 13.sp,
                textAlign = TextAlign.Center,
            )
        }
    }
}

/** Команда создания канала для ядра. */
fun createChannelCommand(name: String, about: String): String = CoreJson.command(
    "create_channel",
    "name" to name,
    "about" to about,
    "avatar_base64" to null,
)

fun inviteToChannelCommand(channelId: String, userIds: List<String>): String = CoreJson.command(
    "invite_to_channel",
    "channel_id" to channelId,
    "user_ids" to JSONArray(userIds),
)
