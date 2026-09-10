package app.turattext.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
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
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.foundation.text.appendInlineContent
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.Contact
import app.turattext.mobile.model.Message
import kotlinx.coroutines.launch

val Reactions = listOf("❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔")

/** Элемент ленты: разделитель дня либо сообщение со сведениями о группировке. */
sealed interface FeedItem {
    val key: String

    data class Day(val label: String, override val key: String) : FeedItem
    data class Bubble(
        val message: Message,
        val first: Boolean,
        val last: Boolean,
    ) : FeedItem {
        override val key get() = message.eventId
    }
}

/** Telegram склеивает подряд идущие сообщения одного автора в группу с одним «хвостом». */
fun buildFeed(messages: List<Message>): List<FeedItem> {
    val items = mutableListOf<FeedItem>()
    val grouped = { left: Message?, right: Message ->
        left != null && left.outgoing == right.outgoing &&
            left.forwardedFrom == right.forwardedFrom &&
            isSameDay(left.createdAt, right.createdAt) &&
            kotlin.math.abs(right.createdAt - left.createdAt) < 10 * 60_000L
    }
    messages.forEachIndexed { index, message ->
        val previous = messages.getOrNull(index - 1)
        val next = messages.getOrNull(index + 1)
        val newDay = previous == null || !isSameDay(previous.createdAt, message.createdAt)
        if (newDay) {
            items += FeedItem.Day(dateSeparator(message.createdAt), "day-${message.eventId}")
        }
        val first = newDay || !grouped(previous, message) || message.replyToEventId != null
        val last = next == null || !grouped(message, next) || next.replyToEventId != null
        items += FeedItem.Bubble(message, first, last)
    }
    return items
}

@Composable
fun ConversationPane(
    state: AppSnapshot,
    actions: AppActions,
    showBack: Boolean,
    onBack: () -> Unit,
    onOpenProfile: () -> Unit,
    onForward: (Set<String>) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    val contact = state.selectedContact
    if (contact == null) {
        Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            ServicePill("Выберите, кому написать")
        }
        return
    }

    val selected = remember(contact.userId) { mutableStateListOf<String>() }
    var draft by remember(contact.userId) { mutableStateOf(contact.draft) }
    var editingId by remember(contact.userId) { mutableStateOf<String?>(null) }
    var replyToId by remember(contact.userId) { mutableStateOf<String?>(null) }
    var headerMenu by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val feed = remember(state.messages) { buildFeed(state.messages) }
    val messageById = remember(state.messages) { state.messages.associateBy { it.eventId } }

    // Черновик Telegram переживает выход из чата и виден в списке диалогов.
    val latestDraft by rememberUpdatedState(draft)
    val savedDraft = contact.draft
    val userId = contact.userId
    DisposableEffect(userId) {
        onDispose {
            if (latestDraft.trim() != savedDraft) actions.saveDraft(userId, latestDraft)
        }
    }

    // Открытый диалог показывается с конца; дальше лента уезжает вниз, только если пользователь
    // и так читает последние сообщения — иначе фоновая синхронизация выдёргивала бы его из истории.
    LaunchedEffect(contact.userId) {
        if (feed.isNotEmpty()) listState.scrollToItem(feed.lastIndex)
    }
    LaunchedEffect(state.messages.size) {
        if (feed.isEmpty()) return@LaunchedEffect
        val lastVisible = listState.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
        if (lastVisible >= feed.lastIndex - 2) listState.animateScrollToItem(feed.lastIndex)
    }
    BackHandler(enabled = showBack || selected.isNotEmpty() || replyToId != null || editingId != null) {
        when {
            selected.isNotEmpty() -> selected.clear()
            editingId != null -> { editingId = null; draft = "" }
            replyToId != null -> replyToId = null
            else -> onBack()
        }
    }

    fun scrollTo(eventId: String) {
        val index = feed.indexOfFirst { it is FeedItem.Bubble && it.message.eventId == eventId }
        if (index >= 0) scope.launch { listState.animateScrollToItem(index) }
    }

    fun submit() {
        val value = draft.trim()
        if (value.isEmpty()) return
        val editing = editingId
        if (editing == null) actions.send(contact.userId, value, replyToId) else actions.edit(editing, value)
        draft = ""
        editingId = null
        replyToId = null
        scope.launch { if (feed.isNotEmpty()) listState.animateScrollToItem(feed.lastIndex) }
    }

    Column(
        modifier.fillMaxSize()
            .background(Brush.verticalGradient(listOf(colors.chatTop.copy(alpha = 0.55f), colors.chatBottom))),
    ) {
        Column(Modifier.glass(colors, GlassShape.Header, raised = true).statusBarsPadding()) {
            if (selected.isEmpty()) {
                ConversationHeader(
                    contact = contact,
                    showBack = showBack,
                    onBack = onBack,
                    onOpenProfile = onOpenProfile,
                    menuOpen = headerMenu,
                    onMenu = { headerMenu = it },
                    onClearHistory = { actions.clearHistory(contact.userId) },
                    onMute = { actions.setMuted(contact.userId, !contact.muted) },
                    onDeleteChat = { actions.deleteContact(contact.userId) },
                )
            } else {
                SelectionBar(
                    count = selected.size,
                    canEdit = selected.size == 1 &&
                        state.messages.any { it.eventId == selected.first() && it.outgoing && !it.deleted },
                    onClear = { selected.clear() },
                    onCopy = {
                        val text = state.messages.filter { it.eventId in selected && !it.deleted }
                            .joinToString("\n") { it.text }
                        if (text.isNotBlank()) clipboard.setText(AnnotatedString(text))
                        selected.clear()
                    },
                    onForward = { onForward(selected.toSet()); selected.clear() },
                    onReact = { actions.react(selected.toSet(), it); selected.clear() },
                    onEdit = {
                        val message = state.messages.first { it.eventId == selected.first() }
                        editingId = message.eventId
                        draft = message.text
                        selected.clear()
                    },
                    onDelete = { actions.deleteMessages(selected.toSet()); selected.clear() },
                )
            }
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (feed.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    ServicePill(
                        if (contact.pending) "Собеседник ждёт вашего ответа"
                        else "Сообщения защищены сквозным шифрованием",
                    )
                }
            }
            LazyColumn(
                state = listState,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(horizontal = 8.dp, vertical = 10.dp),
            ) {
                items(feed, key = { it.key }) { item ->
                    when (item) {
                        is FeedItem.Day -> Box(Modifier.fillMaxWidth().padding(vertical = 8.dp)) {
                            ServicePill(item.label, Modifier.align(Alignment.Center))
                        }

                        is FeedItem.Bubble -> MessageRow(
                            item = item,
                            repliedTo = item.message.replyToEventId?.let(messageById::get),
                            contactName = contact.displayName,
                            selectionMode = selected.isNotEmpty(),
                            selected = item.message.eventId in selected,
                            onToggle = {
                                if (item.message.eventId in selected) selected.remove(item.message.eventId)
                                else selected.add(item.message.eventId)
                            },
                            onReply = { replyToId = item.message.eventId },
                            onForward = { onForward(setOf(item.message.eventId)) },
                            onOpenReplied = { id -> scrollTo(id) },
                            onReact = { actions.react(setOf(item.message.eventId), it) },
                            onCopy = { clipboard.setText(AnnotatedString(item.message.text)) },
                            onEdit = {
                                editingId = item.message.eventId
                                draft = item.message.text
                            },
                            onDelete = { actions.deleteMessages(setOf(item.message.eventId)) },
                        )
                    }
                }
            }
            ScrollDownButton(
                visible = listState.canScrollForward,
                onClick = { scope.launch { listState.animateScrollToItem(feed.lastIndex) } },
                modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp),
            )
        }

        if (contact.pending) {
            PendingBar(
                onAccept = { actions.acceptContact(contact.userId) },
                onReject = { actions.rejectContact(contact.userId) },
            )
        } else {
            Column(Modifier.glass(colors, GlassShape.Footer, raised = true).imePadding()) {
                val replied = replyToId?.let(messageById::get)
                if (editingId != null) {
                    ComposerBanner(
                        icon = R.drawable.ic_edit,
                        title = "Редактирование",
                        text = draft.ifBlank { "сообщение" },
                        onCancel = { editingId = null; draft = "" },
                    )
                } else if (replied != null) {
                    ComposerBanner(
                        icon = R.drawable.ic_reply,
                        title = if (replied.outgoing) "Вы" else contact.displayName,
                        text = quoteOf(replied),
                        onCancel = { replyToId = null },
                    )
                }
                Composer(
                    draft = draft,
                    onDraftChange = { draft = it },
                    onAttach = actions.pickAttachment,
                    onSend = ::submit,
                )
            }
        }
    }
}

/** Круглая кнопка «вниз», появляющаяся при прокрутке вверх — как в Telegram. */
@Composable
private fun ScrollDownButton(visible: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    val colors = Telegram.colors
    AnimatedVisibility(
        visible = visible,
        modifier = modifier,
        enter = fadeIn() + scaleIn(),
        exit = fadeOut() + scaleOut(),
    ) {
        Box(
            Modifier.size(44.dp).clip(CircleShape)
                .glass(colors, CircleShape, raised = true)
                .clickable(onClick = onClick),
            contentAlignment = Alignment.Center,
        ) {
            Icon(painterResource(R.drawable.ic_arrow_down), "Вниз", Modifier.size(20.dp), colors.hint)
        }
    }
}

@Composable
private fun ConversationHeader(
    contact: Contact,
    showBack: Boolean,
    onBack: () -> Unit,
    onOpenProfile: () -> Unit,
    menuOpen: Boolean,
    onMenu: (Boolean) -> Unit,
    onClearHistory: () -> Unit,
    onMute: () -> Unit,
    onDeleteChat: () -> Unit,
) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (showBack) {
            IconButton(onBack) {
                Icon(painterResource(R.drawable.ic_back), "Назад", Modifier.size(22.dp), colors.text)
            }
        } else {
            Spacer(Modifier.width(10.dp))
        }
        Row(
            Modifier.weight(1f).clickable(onClick = onOpenProfile).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(
                name = contact.displayName,
                key = contact.userId,
                avatarBase64 = contact.avatarBase64,
                size = 40.dp,
                online = isOnline(contact),
                onlineRing = colors.panel,
            )
            Spacer(Modifier.width(10.dp))
            Column {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        contact.displayName,
                        Modifier.weight(1f, fill = false),
                        color = colors.text,
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (contact.muted) {
                        Spacer(Modifier.width(5.dp))
                        Icon(painterResource(R.drawable.ic_mute), "Без звука", Modifier.size(14.dp), colors.hint)
                    }
                }
                Text(
                    presenceOf(contact),
                    color = if (isOnline(contact)) colors.accent else colors.hint,
                    fontSize = 13.sp,
                    maxLines = 1,
                )
            }
        }
        Box {
            IconButton({ onMenu(true) }) {
                Icon(painterResource(R.drawable.ic_more), "Ещё", Modifier.size(20.dp), colors.text)
            }
            DropdownMenu(menuOpen, { onMenu(false) }) {
                MenuRow(R.drawable.ic_person, "Профиль") { onMenu(false); onOpenProfile() }
                MenuRow(
                    if (contact.muted) R.drawable.ic_unmute else R.drawable.ic_mute,
                    if (contact.muted) "Включить звук" else "Отключить звук",
                ) { onMenu(false); onMute() }
                MenuRow(R.drawable.ic_broom, "Очистить историю") { onMenu(false); onClearHistory() }
                MenuRow(R.drawable.ic_delete, "Удалить чат", danger = true) { onMenu(false); onDeleteChat() }
            }
        }
    }
}

@Composable
private fun MenuRow(icon: Int, title: String, danger: Boolean = false, onClick: () -> Unit) {
    val colors = Telegram.colors
    DropdownMenuItem(
        text = { Text(title, color = if (danger) colors.danger else colors.text) },
        onClick = onClick,
        leadingIcon = {
            Icon(painterResource(icon), null, Modifier.size(18.dp), if (danger) colors.danger else colors.hint)
        },
    )
}

@Composable
private fun SelectionBar(
    count: Int,
    canEdit: Boolean,
    onClear: () -> Unit,
    onCopy: () -> Unit,
    onForward: () -> Unit,
    onReact: (String) -> Unit,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
) {
    val colors = Telegram.colors
    var reactionMenu by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClear) {
            Icon(painterResource(R.drawable.ic_close), "Снять выделение", Modifier.size(20.dp), colors.text)
        }
        Text(
            "$count",
            Modifier.weight(1f).padding(start = 8.dp),
            color = colors.text,
            fontSize = 18.sp,
            fontWeight = FontWeight.Medium,
        )
        Box {
            IconButton({ reactionMenu = true }) {
                Icon(painterResource(R.drawable.ic_reaction), "Реакция", Modifier.size(21.dp), colors.text)
            }
            DropdownMenu(reactionMenu, { reactionMenu = false }) {
                ReactionRow { reactionMenu = false; onReact(it) }
            }
        }
        IconButton(onForward) {
            Icon(painterResource(R.drawable.ic_forward), "Переслать", Modifier.size(20.dp), colors.text)
        }
        IconButton(onCopy) {
            Icon(painterResource(R.drawable.ic_copy), "Копировать", Modifier.size(20.dp), colors.text)
        }
        if (canEdit) {
            IconButton(onEdit) {
                Icon(painterResource(R.drawable.ic_edit), "Изменить", Modifier.size(20.dp), colors.text)
            }
        }
        IconButton(onDelete) {
            Icon(painterResource(R.drawable.ic_delete), "Удалить", Modifier.size(20.dp), colors.danger)
        }
    }
}

@Composable
fun ReactionRow(onPick: (String) -> Unit) {
    Row(Modifier.padding(horizontal = 8.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        Reactions.forEach { value ->
            Box(
                Modifier.size(38.dp).clip(CircleShape).clickable { onPick(value) },
                contentAlignment = Alignment.Center,
            ) {
                Text(value, fontSize = 21.sp)
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MessageRow(
    item: FeedItem.Bubble,
    repliedTo: Message?,
    contactName: String,
    selectionMode: Boolean,
    selected: Boolean,
    onToggle: () -> Unit,
    onReply: () -> Unit,
    onForward: () -> Unit,
    onOpenReplied: (String) -> Unit,
    onReact: (String) -> Unit,
    onCopy: () -> Unit,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
) {
    val colors = Telegram.colors
    val message = item.message
    var menu by remember { mutableStateOf(false) }
    Box(
        Modifier.fillMaxWidth()
            .background(if (selected) colors.accent.copy(alpha = .18f) else Color.Transparent)
            .padding(top = if (item.first) 8.dp else 2.dp)
            .padding(horizontal = 2.dp),
    ) {
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = if (message.outgoing) Arrangement.End else Arrangement.Start,
            verticalAlignment = Alignment.Bottom,
        ) {
            if (selectionMode) {
                Box(
                    Modifier.padding(end = 6.dp, bottom = 6.dp).size(22.dp).clip(CircleShape)
                        .background(if (selected) colors.accent else Color.Transparent)
                        .border(1.5.dp, if (selected) colors.accent else colors.hint, CircleShape)
                        .clickable(onClick = onToggle),
                    contentAlignment = Alignment.Center,
                ) {
                    if (selected) {
                        Icon(painterResource(R.drawable.ic_check), null, Modifier.size(13.dp), colors.onAccent)
                    }
                }
            }
            Box {
                MessageBubble(
                    message = message,
                    repliedTo = repliedTo,
                    contactName = contactName,
                    first = item.first,
                    last = item.last,
                    onOpenReplied = onOpenReplied,
                    modifier = Modifier.combinedClickable(
                        onClick = { if (selectionMode) onToggle() },
                        onLongClick = { menu = true },
                    ),
                )
                DropdownMenu(menu, { menu = false }) {
                    if (!message.deleted) ReactionRow { menu = false; onReact(it) }
                    if (!message.deleted) {
                        MenuRow(R.drawable.ic_reply, "Ответить") { menu = false; onReply() }
                        MenuRow(R.drawable.ic_forward, "Переслать") { menu = false; onForward() }
                        MenuRow(R.drawable.ic_copy, "Копировать") { menu = false; onCopy() }
                    }
                    if (message.outgoing && !message.deleted) {
                        MenuRow(R.drawable.ic_edit, "Изменить") { menu = false; onEdit() }
                    }
                    MenuRow(R.drawable.ic_check, "Выделить") { menu = false; onToggle() }
                    if (message.outgoing && !message.deleted) {
                        MenuRow(R.drawable.ic_delete, "Удалить", danger = true) { menu = false; onDelete() }
                    }
                }
            }
        }
    }
}

fun quoteOf(message: Message): String = when {
    message.deleted -> "Сообщение удалено"
    message.text.isNotBlank() -> message.text
    message.attachment != null -> "📎 ${message.attachment.fileName}"
    else -> "Сообщение"
}

/** Пузырь Telegram: скруглённые углы, «хвост» у последнего в группе, время внутри. */
@Composable
private fun MessageBubble(
    message: Message,
    repliedTo: Message?,
    contactName: String,
    first: Boolean,
    last: Boolean,
    onOpenReplied: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    val outgoing = message.outgoing
    // Liquid Glass: пузыри скруглены крупнее, «хвост» остаётся заметно острее остальных углов.
    val big = 20.dp
    val small = 7.dp
    val tail = 5.dp
    val shape = if (outgoing) {
        RoundedCornerShape(
            topStart = big,
            topEnd = if (first) big else small,
            bottomEnd = if (last) tail else small,
            bottomStart = big,
        )
    } else {
        RoundedCornerShape(
            topStart = if (first) big else small,
            topEnd = big,
            bottomEnd = big,
            bottomStart = if (last) tail else small,
        )
    }
    val body = if (message.deleted) "Сообщение удалено" else message.text
    val textColor = when {
        message.deleted -> if (outgoing) colors.bubbleOutMeta else colors.bubbleInMeta
        outgoing -> colors.bubbleOutText
        else -> colors.bubbleInText
    }
    val metaColor = if (outgoing) colors.bubbleOutMeta else colors.bubbleInMeta
    val accentInBubble = if (outgoing) colors.bubbleOutMeta else colors.accent
    val inlineMeta = message.reactions.isEmpty() && body.isNotBlank()
    val metaWidth = (
        34f + (if (message.edited) 26f else 0f) + (if (outgoing) 20f else 0f)
        ).sp

    // Входящий пузырь — стекло, исходящий — акцент под тем же бликом: оба слоя пропускают фон.
    val bubbleFill = if (outgoing) {
        Brush.verticalGradient(
            listOf(colors.bubbleOut.copy(alpha = 0.94f), colors.bubbleOut.copy(alpha = 0.82f)),
        )
    } else {
        Brush.verticalGradient(listOf(colors.glassRaised, colors.glassBubble))
    }
    Box(
        modifier
            .widthIn(max = 480.dp)
            .clip(shape)
            .background(bubbleFill)
            .border(1.dp, colors.glassRim.copy(alpha = colors.glassRim.alpha * 0.7f), shape)
            .padding(start = 12.dp, end = 12.dp, top = 7.dp, bottom = 7.dp),
    ) {
        Column {
            message.forwardedFrom?.let { author ->
                Text(
                    "Переслано от $author",
                    color = accentInBubble,
                    fontSize = 13.sp,
                    fontWeight = FontWeight.Medium,
                    modifier = Modifier.padding(bottom = 3.dp),
                )
            }
            if (repliedTo != null) {
                Row(
                    Modifier.padding(bottom = 4.dp)
                        .clip(RoundedCornerShape(4.dp))
                        .background(
                            if (outgoing) colors.bubbleOutText.copy(alpha = .14f)
                            else colors.accent.copy(alpha = .10f),
                        )
                        .clickable { onOpenReplied(repliedTo.eventId) }
                        .padding(end = 6.dp),
                ) {
                    Box(Modifier.width(2.dp).height(34.dp).background(accentInBubble))
                    Column(Modifier.padding(start = 6.dp, top = 2.dp, bottom = 2.dp)) {
                        Text(
                            if (repliedTo.outgoing) "Вы" else contactName,
                            color = accentInBubble,
                            fontSize = 13.sp,
                            fontWeight = FontWeight.Medium,
                            maxLines = 1,
                        )
                        Text(
                            quoteOf(repliedTo),
                            color = metaColor,
                            fontSize = 13.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
            message.attachment?.let { attachment ->
                Row(
                    Modifier.padding(vertical = 2.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier.size(40.dp).clip(CircleShape)
                            .background(
                                if (outgoing) colors.bubbleOutText.copy(alpha = .20f)
                                else colors.accent.copy(alpha = .18f),
                            ),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_file),
                            null,
                            Modifier.size(20.dp),
                            if (outgoing) colors.bubbleOutText else colors.accent,
                        )
                    }
                    Spacer(Modifier.width(10.dp))
                    Column {
                        Text(
                            attachment.fileName,
                            color = textColor,
                            fontSize = 15.sp,
                            fontWeight = FontWeight.Medium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(formatBytes(attachment.size), color = metaColor, fontSize = 12.sp)
                    }
                }
            }
            if (body.isNotBlank()) {
                if (inlineMeta) {
                    Text(
                        text = buildAnnotatedString {
                            append(body)
                            appendInlineContent("meta", " ")
                        },
                        color = textColor,
                        fontSize = 16.sp,
                        inlineContent = mapOf(
                            "meta" to InlineTextContent(
                                Placeholder(metaWidth, 1.sp, PlaceholderVerticalAlign.TextBottom),
                            ) {},
                        ),
                    )
                } else {
                    Text(body, color = textColor, fontSize = 16.sp)
                }
            }
            if (message.reactions.isNotEmpty()) {
                Spacer(Modifier.height(4.dp))
                Row(
                    horizontalArrangement = Arrangement.spacedBy(4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    message.reactions.forEach { reaction ->
                        Box(
                            Modifier.clip(RoundedCornerShape(11.dp))
                                .background(
                                    if (outgoing) colors.bubbleOutText.copy(alpha = .18f)
                                    else colors.accent.copy(alpha = .14f),
                                )
                                .padding(horizontal = 7.dp, vertical = 2.dp),
                        ) {
                            Text(reaction, fontSize = 14.sp)
                        }
                    }
                    Spacer(Modifier.width(2.dp))
                    MessageMeta(message, metaColor, colors.tick, outgoing)
                }
            }
            if (!inlineMeta && message.reactions.isEmpty()) {
                Row(Modifier.align(Alignment.End).padding(top = 2.dp)) {
                    MessageMeta(message, metaColor, colors.tick, outgoing)
                }
            }
        }
        if (inlineMeta) {
            Row(Modifier.align(Alignment.BottomEnd)) {
                MessageMeta(message, metaColor, colors.tick, outgoing)
            }
        }
    }
}

@Composable
private fun MessageMeta(message: Message, metaColor: Color, tick: Color, outgoing: Boolean) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        if (message.edited) {
            Text("изм.", color = metaColor, fontSize = 11.sp)
            Spacer(Modifier.width(4.dp))
        }
        Text(timeOf(message.createdAt), color = metaColor, fontSize = 11.sp)
        if (outgoing) {
            Spacer(Modifier.width(3.dp))
            DeliveryTicks(
                delivered = message.delivered,
                read = message.delivered,
                tint = if (message.delivered) tick else metaColor,
                size = 15.dp,
            )
        }
    }
}

/** Панель над полем ввода: редактирование или ответ на сообщение. */
@Composable
private fun ComposerBanner(icon: Int, title: String, text: String, onCancel: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().padding(start = 12.dp, end = 6.dp, top = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(icon), null, Modifier.size(18.dp), colors.accent)
        Spacer(Modifier.width(10.dp))
        Box(Modifier.width(2.dp).height(32.dp).background(colors.accent))
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.accent, fontSize = 13.sp, fontWeight = FontWeight.Medium)
            Text(text, color = colors.hint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        IconButton(onCancel) {
            Icon(painterResource(R.drawable.ic_close), "Отменить", Modifier.size(18.dp), colors.hint)
        }
    }
}

@Composable
private fun Composer(
    draft: String,
    onDraftChange: (String) -> Unit,
    onAttach: () -> Unit,
    onSend: () -> Unit,
) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 6.dp, vertical = 6.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        IconButton(onAttach, Modifier.size(44.dp)) {
            Icon(painterResource(R.drawable.ic_attach), "Прикрепить файл", Modifier.size(22.dp), colors.hint)
        }
        Box(
            Modifier.weight(1f).heightIn(min = 44.dp)
                .clip(GlassShape.Capsule)
                .glass(colors, GlassShape.Capsule)
                .padding(horizontal = 14.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            if (draft.isEmpty()) {
                Text("Отправить сообщение", color = colors.hint, fontSize = 16.sp)
            }
            BasicTextField(
                value = draft,
                onValueChange = onDraftChange,
                textStyle = TextStyle(color = colors.text, fontSize = 16.sp),
                cursorBrush = SolidColor(colors.accent),
                maxLines = 6,
                modifier = Modifier.fillMaxWidth().padding(vertical = 10.dp),
            )
        }
        AnimatedVisibility(
            visible = draft.isNotBlank(),
            enter = scaleIn() + fadeIn(),
            exit = scaleOut() + fadeOut(),
        ) {
            Box(
                Modifier.size(44.dp).clip(CircleShape)
                    .background(colors.accent.copy(alpha = 0.92f))
                    .border(1.dp, colors.glassRim, CircleShape)
                    .clickable(onClick = onSend),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(R.drawable.ic_send), "Отправить", Modifier.size(21.dp), colors.onAccent)
            }
        }
    }
}

@Composable
private fun PendingBar(onAccept: () -> Unit, onReject: () -> Unit) {
    val colors = Telegram.colors
    Column(Modifier.fillMaxWidth().glass(colors, GlassShape.Footer, raised = true).navigationBarsPadding()) {
        Text(
            "Этот пользователь хочет начать переписку",
            Modifier.fillMaxWidth().padding(top = 10.dp),
            color = colors.hint,
            fontSize = 13.sp,
            textAlign = TextAlign.Center,
        )
        Row(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
            Box(
                Modifier.weight(1f).height(48.dp).clickable(onClick = onReject),
                contentAlignment = Alignment.Center,
            ) {
                Text("ОТКЛОНИТЬ", color = colors.danger, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            }
            Box(
                Modifier.weight(1f).height(48.dp).clickable(onClick = onAccept),
                contentAlignment = Alignment.Center,
            ) {
                Text("ПРИНЯТЬ", color = colors.accent, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            }
        }
    }
}
