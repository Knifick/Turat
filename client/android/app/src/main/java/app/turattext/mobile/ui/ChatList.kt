package app.turattext.mobile.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.Chat
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.SearchHit
import app.turattext.mobile.update.UpdateState
import kotlinx.coroutines.delay

/** Левая колонка Telegram: шапка, поиск, список чатов и круглая кнопка нового чата. */
@Composable
fun ChatListPane(
    state: AppSnapshot,
    busy: Boolean,
    update: UpdateState,
    actions: AppActions,
    onOpenChat: (String) -> Unit,
    onMenu: () -> Unit,
    onNewChat: () -> Unit,
    onOpenUpdate: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    var query by rememberSaveable { mutableStateOf("") }
    val searching = query.isNotBlank()

    LaunchedEffect(query) {
        delay(280)
        actions.search(if (query.length >= 2) query else "")
    }

    val chats = state.chats.filter { chat ->
        !searching ||
            chat.contact.displayName.contains(query, true) ||
            chat.contact.username?.contains(query, true) == true ||
            chat.preview.contains(query, true)
    }

    Column(modifier) {
        // Шапка и поиск — верхний слой стекла над лентой чатов.
        Column(
            Modifier.glass(colors, GlassShape.Header, raised = true)
                .statusBarsPadding(),
        ) {
            Row(
                Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onMenu) {
                    Icon(painterResource(R.drawable.ic_menu), "Меню", Modifier.size(24.dp), colors.text)
                }
                Column(Modifier.weight(1f).padding(start = 8.dp)) {
                    Text("Turat", fontSize = 19.sp, fontWeight = FontWeight.Medium, color = colors.text)
                    Text(
                        when {
                            busy -> "обновление…"
                            state.online -> "в сети · сквозное шифрование"
                            else -> "нет связи с Node · только локально"
                        },
                        fontSize = 12.sp,
                        color = if (busy || state.online) colors.accent else colors.hint,
                    )
                }
                IconButton(actions.sync) {
                    Icon(painterResource(R.drawable.ic_sync), "Обновить сейчас", Modifier.size(22.dp), colors.text)
                }
            }
            SearchField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, bottom = 10.dp),
            )
        }

        // Во время поиска строка об обновлении только мешала бы.
        AnimatedVisibility(update.showBanner && !searching) {
            update.available?.let { release -> UpdateBanner(release, onOpenUpdate, actions.dismissUpdate) }
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            val hits = if (searching) state.searchResults else emptyList()
            if (chats.isEmpty() && hits.isEmpty()) {
                EmptyChats(searching || state.chats.isNotEmpty())
            } else {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
                    if (searching && chats.isNotEmpty()) {
                        item { ListSectionHeader("Чаты") }
                    }
                    items(chats, key = { it.contact.userId }) { chat ->
                        ChatRow(
                            chat = chat,
                            selected = chat.contact.userId == state.selectedContactId,
                            actions = actions,
                            onClick = { onOpenChat(chat.contact.userId) },
                        )
                    }
                    if (hits.isNotEmpty()) {
                        item { ListSectionHeader("Сообщения") }
                        items(hits, key = { it.eventId }) { hit ->
                            SearchRow(hit) { onOpenChat(hit.userId) }
                        }
                    }
                }
            }
            FloatingActionButton(
                onClick = onNewChat,
                modifier = Modifier.align(Alignment.BottomEnd).padding(18.dp).navigationBarsPadding()
                    .border(1.dp, colors.glassRim, CircleShape),
                containerColor = colors.accent.copy(alpha = 0.9f),
                contentColor = colors.onAccent,
                shape = CircleShape,
            ) {
                Icon(painterResource(R.drawable.ic_edit), "Новый чат", Modifier.size(23.dp))
            }
        }
    }
}

@Composable
private fun EmptyChats(searched: Boolean) {
    val colors = Telegram.colors
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(painterResource(R.drawable.ic_lock), null, Modifier.size(44.dp), colors.hint.copy(alpha = .6f))
        Spacer(Modifier.height(14.dp))
        Text(
            if (searched) "Ничего не найдено" else "Здесь появятся ваши чаты",
            color = colors.hint,
            fontSize = 15.sp,
        )
    }
}

@Composable
private fun ListSectionHeader(title: String) {
    Text(
        title,
        Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 9.dp),
        color = Telegram.colors.hint,
        fontSize = 13.sp,
        fontWeight = FontWeight.Medium,
    )
}

@Composable
private fun SearchField(value: String, onValueChange: (String) -> Unit, modifier: Modifier = Modifier) {
    val colors = Telegram.colors
    Row(
        modifier.height(42.dp).clip(GlassShape.Capsule)
            .glass(colors, GlassShape.Capsule)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_search), null, Modifier.size(18.dp), colors.hint)
        Spacer(Modifier.width(8.dp))
        Box(Modifier.weight(1f), contentAlignment = Alignment.CenterStart) {
            if (value.isEmpty()) Text("Поиск", color = colors.hint, fontSize = 15.sp)
            BasicTextField(
                value = value,
                onValueChange = onValueChange,
                singleLine = true,
                textStyle = TextStyle(color = colors.text, fontSize = 15.sp),
                cursorBrush = SolidColor(colors.accent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        if (value.isNotEmpty()) {
            IconButton({ onValueChange("") }, Modifier.size(22.dp)) {
                Icon(painterResource(R.drawable.ic_close), "Очистить", Modifier.size(15.dp), colors.hint)
            }
        }
    }
}

/** Строка чата: аватар, имя, время, превью, галочки и счётчик непрочитанных. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ChatRow(chat: Chat, selected: Boolean, actions: AppActions, onClick: () -> Unit) {
    val colors = Telegram.colors
    val contact = chat.contact
    var menu by remember { mutableStateOf(false) }
    var confirmGroupDelete by remember { mutableStateOf(false) }
    Box(Modifier.padding(horizontal = 8.dp, vertical = 2.dp)) {
        Row(
            Modifier.fillMaxWidth()
                .clip(GlassShape.Card)
                .then(if (selected) Modifier.glass(colors, GlassShape.Card, raised = true) else Modifier)
                .combinedClickable(onClick = onClick, onLongClick = { menu = true })
                .padding(start = 10.dp, end = 12.dp, top = 9.dp, bottom = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(
                name = contact.displayName,
                key = contact.userId,
                avatarBase64 = contact.avatarBase64,
                size = 54.dp,
                online = isOnline(contact),
                onlineRing = colors.window,
            )
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        if (chat.isGroup) {
                            Icon(painterResource(R.drawable.ic_group), "Группа", Modifier.size(17.dp), colors.text)
                            Spacer(Modifier.width(4.dp))
                        }
                        Text(
                            contact.displayName,
                            Modifier.weight(1f, fill = false),
                            color = colors.text,
                            fontSize = 16.sp,
                            fontWeight = FontWeight.Medium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (contact.verified) {
                            Spacer(Modifier.width(4.dp))
                            Icon(
                                painterResource(R.drawable.ic_verified),
                                "Fingerprint сверен",
                                Modifier.size(15.dp),
                                colors.accent,
                            )
                        }
                        if (contact.muted) {
                            Spacer(Modifier.width(4.dp))
                            Icon(
                                painterResource(R.drawable.ic_mute),
                                "Без звука",
                                Modifier.size(15.dp),
                                colors.hint,
                            )
                        }
                    }
                    Spacer(Modifier.width(6.dp))
                    if (chat.hasLastMessage && chat.outgoing && contact.draft.isBlank()) {
                        DeliveryTicks(
                            delivered = chat.delivered,
                            read = chat.read,
                            tint = if (chat.delivered) colors.tick else colors.hint,
                            size = 16.dp,
                        )
                        Spacer(Modifier.width(3.dp))
                    }
                    Text(chatListTime(chat.lastActivity), color = colors.hint, fontSize = 12.sp)
                }
                Spacer(Modifier.height(3.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (contact.draft.isNotBlank()) {
                        Text("Черновик: ", color = colors.danger, fontSize = 15.sp, maxLines = 1)
                    }
                    Text(
                        when {
                            contact.draft.isNotBlank() -> contact.draft
                            contact.pending && chat.isGroup -> "Приглашение в группу"
                            contact.pending -> "Хочет начать диалог"
                            chat.preview.isNotBlank() -> chat.preview
                            chat.isGroup -> membersLabel(chat.memberCount)
                            contact.username != null -> "@${contact.username}"
                            else -> shortId(contact.userId)
                        },
                        Modifier.weight(1f),
                        color = if (contact.pending) colors.accent else colors.hint,
                        fontSize = 15.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (contact.pinned && chat.unreadCount == 0 && !contact.manualUnread) {
                        Spacer(Modifier.width(8.dp))
                        Icon(painterResource(R.drawable.ic_pin), "Закреплён", Modifier.size(15.dp), colors.hint)
                    }
                    if (chat.unreadCount > 0) {
                        Spacer(Modifier.width(8.dp))
                        UnreadBadge(chat.unreadCount, if (contact.muted) colors.hint else colors.badge)
                    } else if (contact.manualUnread) {
                        Spacer(Modifier.width(8.dp))
                        Box(
                            Modifier.size(10.dp).clip(CircleShape)
                                .background(if (contact.muted) colors.hint else colors.badge),
                        )
                    }
                }
            }
        }
        DropdownMenu(menu, { menu = false }) {
            ChatMenuItem(
                if (contact.pinned) R.drawable.ic_unpin else R.drawable.ic_pin,
                if (contact.pinned) "Открепить" else "Закрепить",
            ) { menu = false; actions.setPinned(contact.userId, !contact.pinned) }
            ChatMenuItem(
                if (contact.muted) R.drawable.ic_unmute else R.drawable.ic_mute,
                if (contact.muted) "Включить звук" else "Отключить звук",
            ) { menu = false; actions.setMuted(contact.userId, !contact.muted) }
            ChatMenuItem(R.drawable.ic_check, "Отметить непрочитанным") {
                menu = false
                actions.markUnread(contact.userId)
            }
            ChatMenuItem(R.drawable.ic_broom, "Очистить историю") {
                menu = false
                actions.clearHistory(contact.userId)
            }
            if (chat.isGroup && chat.canWrite) {
                ChatMenuItem(R.drawable.ic_close, "Покинуть группу", danger = true) {
                    menu = false
                    actions.command(CoreJson.command("leave_group", "group_id" to contact.userId), null)
                }
            }
            ChatMenuItem(R.drawable.ic_delete, if (chat.isGroup) "Удалить группу" else "Удалить чат", danger = true) {
                menu = false
                // Удаление группы — это ещё и выход из неё, поэтому оно требует подтверждения.
                if (chat.isGroup) confirmGroupDelete = true else actions.deleteContact(contact.userId)
            }
        }
    }
    if (confirmGroupDelete) {
        GroupDeleteDialog(chat, onDismiss = { confirmGroupDelete = false }) {
            actions.deleteContact(contact.userId)
        }
    }
}

@Composable
private fun ChatMenuItem(icon: Int, title: String, danger: Boolean = false, onClick: () -> Unit) {
    val colors = Telegram.colors
    DropdownMenuItem(
        text = { Text(title, color = if (danger) colors.danger else colors.text) },
        onClick = onClick,
        leadingIcon = {
            Icon(painterResource(icon), null, Modifier.size(18.dp), if (danger) colors.danger else colors.hint)
        },
    )
}

/** Найденное сообщение: аватар чата, имя, текст и дата. */
@Composable
private fun SearchRow(hit: SearchHit, onClick: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick)
            .padding(start = 12.dp, end = 14.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(hit.displayName, hit.userId, hit.avatarBase64, 46.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row {
                Text(
                    hit.displayName,
                    Modifier.weight(1f),
                    color = colors.text,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(chatListTime(hit.createdAt), color = colors.hint, fontSize = 12.sp)
            }
            Text(
                (if (hit.outgoing) "Вы: " else "") + hit.text,
                color = colors.hint,
                fontSize = 14.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}
