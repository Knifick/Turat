package app.turattext.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Easing
import androidx.compose.animation.core.FastOutLinearInEasing
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animate
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.VerticalDivider
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.Message
import app.turattext.mobile.model.PendingUpload
import app.turattext.mobile.update.UpdateState
import kotlinx.coroutines.launch

/** Единый набор действий над ядром — иначе экранам пришлось бы передавать два десятка лямбд. */
@Stable
class AppActions(
    val selectContact: (String?) -> Unit,
    val addContact: (String, ((Boolean) -> Unit)?) -> Unit,
    val deleteContact: (String) -> Unit,
    val acceptContact: (String) -> Unit,
    val rejectContact: (String) -> Unit,
    val verifyContact: (String, Boolean) -> Unit,
    val send: (String, String, String?) -> Unit,
    val edit: (String, String) -> Unit,
    val deleteMessages: (Set<String>) -> Unit,
    val react: (Set<String>, String) -> Unit,
    val forward: (Set<String>, String) -> Unit,
    val setPinned: (String, Boolean) -> Unit,
    val setMuted: (String, Boolean) -> Unit,
    val saveDraft: (String, String) -> Unit,
    val clearHistory: (String) -> Unit,
    val markUnread: (String) -> Unit,
    /** Отмечает прочитанным диалог, который сейчас перед глазами. */
    val markRead: (String) -> Unit,
    val search: (String) -> Unit,
    val setPresence: (Boolean) -> Unit,
    val sync: () -> Unit,
    val command: (String, ((Boolean) -> Unit)?) -> Unit,
    val pickAttachment: () -> Unit,
    val pickAvatar: () -> Unit,
    /** Выбор фото группы по её GroupID. */
    val pickGroupAvatar: (String) -> Unit,
    /** Сохраняет вложение сообщения в выбранный пользователем файл. */
    val saveAttachment: (Message) -> Unit,
    /** Прерывает начатую передачу вложения по идентификатору задачи. */
    val cancelTransfer: (String) -> Unit,
    val export: (String, String, String) -> Unit,
    val importFile: (String, String, Array<String>) -> Unit,
    val checkUpdates: () -> Unit,
    val installUpdate: () -> Unit,
    val cancelUpdate: () -> Unit,
    /** Скрывает строку об обновлении до следующего запуска. */
    val dismissUpdate: () -> Unit,
    /** Отказ от предложенной версии насовсем. */
    val skipUpdate: () -> Unit,
)

private sealed interface Overlay {
    data object None : Overlay
    data object NewChat : Overlay
    data object Settings : Overlay
    data object Themes : Overlay
    data object Profile : Overlay
    data object Update : Overlay
    data object NewGroup : Overlay
    data object GroupInfo : Overlay
    data object AddMembers : Overlay
    data object NewChannel : Overlay
    data object ChannelInfo : Overlay
    data object PickChannelAdmin : Overlay
    data object InviteToChannel : Overlay
    data object Discussion : Overlay
    data class AdminRights(val userId: String, val displayName: String) : Overlay
    data class Forward(val eventIds: Set<String>) : Overlay
}

@Composable
fun TuratTextApp(
    state: AppSnapshot,
    busy: Boolean,
    uploads: List<PendingUpload>,
    downloads: Map<String, MediaTransfer>,
    update: UpdateState,
    theme: AppTheme,
    font: AppFont,
    onThemeChange: (AppTheme) -> Unit,
    onFontChange: (AppFont) -> Unit,
    actions: AppActions,
) {
    val colors = Telegram.colors
    if (state.onboardingRequired) {
        OnboardingScreen(state.profile) { username, name ->
            actions.command(
                CoreJson.command(
                    "save_profile",
                    "username" to username,
                    "display_name" to name,
                    "about" to "",
                    "avatar_base64" to null,
                ),
                null,
            )
        }
        return
    }

    val drawerState = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var overlay by remember { mutableStateOf<Overlay>(Overlay.None) }
    var newChatSubmitted by remember { mutableStateOf(false) }

    // Свайп вправо внутри диалога должен возвращать к списку чатов, а не открывать меню,
    // поэтому жест шторки живёт только на самом списке.
    val chatListVisible = state.selectedContact == null

    AuroraBackground(Modifier.fillMaxSize()) {
        ModalNavigationDrawer(
            drawerState = drawerState,
            gesturesEnabled = drawerState.isOpen || (chatListVisible && overlay == Overlay.None),
            drawerContent = {
                ModalDrawerSheet(
                    drawerContainerColor = Color.Transparent,
                    drawerContentColor = colors.text,
                    drawerShape = GlassShape.Sheet,
                    // Стандартная максимальная ширина Material drawer — 360 dp.
                    // 252 dp делает служебную карточку ровно на 30% уже.
                    modifier = Modifier.width(252.dp).glass(colors, GlassShape.Sheet, raised = true),
                ) {
                    DrawerContent(
                        profile = state.profile,
                        userId = state.identity.userId,
                        theme = theme,
                        online = state.online,
                        onOpenThemes = { scope.launch { drawerState.close() }; overlay = Overlay.Themes },
                        onNewChat = {
                            scope.launch { drawerState.close() }
                            newChatSubmitted = false
                            overlay = Overlay.NewChat
                        },
                        onNewGroup = {
                            scope.launch { drawerState.close() }
                            newChatSubmitted = false
                            overlay = Overlay.NewGroup
                        },
                        onNewChannel = {
                            scope.launch { drawerState.close() }
                            newChatSubmitted = false
                            overlay = Overlay.NewChannel
                        },
                        onSettings = { scope.launch { drawerState.close() }; overlay = Overlay.Settings },
                        onSync = { scope.launch { drawerState.close() }; actions.sync() },
                    )
                }
            },
        ) {
            BoxWithConstraints(Modifier.fillMaxSize()) {
                val wide = maxWidth >= 720.dp
                val width = with(LocalDensity.current) { maxWidth.toPx() }
                val contactId = state.selectedContact?.userId

                // Диалог лежит поверх списка и ездит по горизонтали: и жест, и открытие с
                // закрытием двигают одно и то же смещение, поэтому переход всегда непрерывен.
                var paneOffset by remember { mutableFloatStateOf(0f) }
                var paneDragging by remember { mutableStateOf(false) }

                suspend fun animatePaneTo(target: Float, duration: Int, easing: Easing) {
                    animate(
                        initialValue = paneOffset,
                        targetValue = target,
                        animationSpec = tween(durationMillis = duration, easing = easing),
                    ) { value, _ -> paneOffset = value.coerceIn(0f, width) }
                    // Фиксируем якорь явно: после отпускания жест никогда не должен остаться
                    // между открытым и закрытым состояниями из-за округления или отмены кадра.
                    paneOffset = target.coerceIn(0f, width)
                }

                LaunchedEffect(contactId, width, wide, paneDragging) {
                    when {
                        wide -> paneOffset = 0f
                        contactId == null -> paneOffset = width
                        paneDragging -> Unit
                        paneOffset != 0f -> animatePaneTo(0f, 260, FastOutSlowInEasing)
                    }
                }

                val closeConversation: () -> Unit = {
                    scope.launch {
                        animatePaneTo(width, 220, FastOutLinearInEasing)
                        actions.selectContact(null)
                    }
                }
                val drag = rememberDraggableState { delta ->
                    // Обновляем позицию синхронно с пальцем. Раньше здесь создавалась корутина на
                    // каждый delta, и запоздавший snapTo мог перезаписать финальную анимацию.
                    paneOffset = (paneOffset + delta).coerceIn(0f, width)
                }

                val chatList: @Composable (Modifier) -> Unit = { modifier ->
                    ChatListPane(
                        state = state,
                        busy = busy,
                        update = update,
                        actions = actions,
                        onOpenChat = actions.selectContact,
                        onMenu = { scope.launch { drawerState.open() } },
                        onNewChat = {
                            newChatSubmitted = false
                            overlay = Overlay.NewChat
                        },
                        onOpenUpdate = { overlay = Overlay.Update },
                        modifier = modifier,
                    )
                }
                // Положение в ленте живёт выше самой панели: панель существует только пока
                // диалог открыт, и если она на миг покинет композицию, прокрутка не должна
                // прыгать к началу переписки.
                val conversationScroll = rememberLazyListState()
                val conversation: @Composable (Modifier, () -> Unit) -> Unit = { modifier, onBack ->
                    ConversationPane(
                        listState = conversationScroll,
                        state = state,
                        actions = actions,
                        uploads = uploads,
                        downloads = downloads,
                        showBack = !wide,
                        onBack = onBack,
                        onOpenProfile = {
                            overlay = when {
                                state.selectedChat?.isChannel == true -> Overlay.ChannelInfo
                                state.selectedChat?.isGroup == true -> Overlay.GroupInfo
                                else -> Overlay.Profile
                            }
                        },
                        onForward = { overlay = Overlay.Forward(it) },
                        modifier = modifier,
                    )
                }

                if (wide) {
                    Row(Modifier.fillMaxSize()) {
                        chatList(Modifier.width(360.dp).fillMaxHeight())
                        VerticalDivider(color = colors.divider, thickness = 1.dp)
                        conversation(Modifier.weight(1f).fillMaxHeight()) { actions.selectContact(null) }
                    }
                } else {
                    val open = contactId != null
                    // 0 — диалог закрывает экран целиком, 1 — он полностью ушёл за правый край.
                    val progress = if (!open || width <= 0f) 1f else (paneOffset / width).coerceIn(0f, 1f)
                    Box(Modifier.fillMaxSize()) {
                        chatList(
                            Modifier.fillMaxSize()
                                .graphicsLayer { translationX = -ListParallax * width * (1f - progress) }
                                .blur((ChatListBlurRadius * (1f - progress)).dp)
                                .blockTouches(open),
                        )
                        if (open) {
                            Box(
                                Modifier.fillMaxSize()
                                    .background(Color.Black.copy(alpha = ScrimAlpha * (1f - progress))),
                            )
                            conversation(
                                Modifier.fillMaxSize()
                                    .graphicsLayer { translationX = paneOffset }
                                    .draggable(
                                        state = drag,
                                        orientation = Orientation.Horizontal,
                                        onDragStarted = { paneDragging = true },
                                        onDragStopped = { velocity ->
                                            val back = paneOffset >= width * BackDistanceFraction ||
                                                velocity > BackVelocity
                                            try {
                                                if (back) {
                                                    animatePaneTo(width, 200, FastOutLinearInEasing)
                                                    actions.selectContact(null)
                                                } else {
                                                    animatePaneTo(0f, 220, FastOutSlowInEasing)
                                                }
                                            } finally {
                                                paneDragging = false
                                            }
                                        },
                                    ),
                                closeConversation,
                            )
                        }
                    }
                }
            }
        }

        OverlayScreen(overlay is Overlay.NewChat) {
            NewChatScreen(
                onBack = { overlay = Overlay.None },
                busy = busy,
                error = if (newChatSubmitted && !busy) state.statusMessage else null,
                onCreate = { query ->
                    newChatSubmitted = true
                    actions.addContact(query) { added ->
                        if (added) overlay = Overlay.None
                    }
                },
                onNewGroup = {
                    newChatSubmitted = false
                    overlay = Overlay.NewGroup
                },
                onNewChannel = {
                    newChatSubmitted = false
                    overlay = Overlay.NewChannel
                },
            )
        }
        OverlayScreen(overlay is Overlay.NewChannel) {
            NewChannelScreen(
                busy = busy,
                error = if (newChatSubmitted && !busy) state.statusMessage else null,
                onBack = { overlay = Overlay.None },
                onCreate = { name, about ->
                    newChatSubmitted = true
                    actions.command(createChannelCommand(name, about)) { created ->
                        if (created) overlay = Overlay.None
                    }
                },
                onSubscribe = { link ->
                    newChatSubmitted = true
                    actions.command(CoreJson.command("subscribe_channel", "link" to link)) { sent ->
                        if (sent) overlay = Overlay.None
                    }
                },
            )
        }
        OverlayScreen(overlay is Overlay.ChannelInfo && state.channel != null) {
            ChannelInfoScreen(
                state = state,
                actions = actions,
                onBack = { overlay = Overlay.None },
                onInvite = { overlay = Overlay.InviteToChannel },
                onAddAdmin = { overlay = Overlay.PickChannelAdmin },
                onEditAdmin = { userId, name -> overlay = Overlay.AdminRights(userId, name) },
                onPickDiscussion = { overlay = Overlay.Discussion },
                onOpenChat = { chatId ->
                    overlay = Overlay.None
                    actions.selectContact(chatId)
                },
            )
        }
        OverlayScreen(overlay is Overlay.PickChannelAdmin && state.channel != null) {
            PickChannelAdminScreen(
                state = state,
                onBack = { overlay = Overlay.ChannelInfo },
                onPick = { userId, name -> overlay = Overlay.AdminRights(userId, name) },
            )
        }
        val rightsTarget = overlay as? Overlay.AdminRights
        OverlayScreen(rightsTarget != null && state.channel != null) {
            rightsTarget?.let { target ->
                AdminRightsScreen(
                    state = state,
                    userId = target.userId,
                    displayName = target.displayName,
                    busy = busy,
                    onBack = { overlay = Overlay.ChannelInfo },
                    onSave = { rights, title ->
                        val channelId = state.channel?.channelId ?: return@AdminRightsScreen
                        actions.command(
                            CoreJson.command(
                                "set_channel_admin",
                                "channel_id" to channelId,
                                "user_id" to target.userId,
                                "rights" to rights.toJson(),
                                "title" to title,
                            ),
                        ) { saved -> if (saved) overlay = Overlay.ChannelInfo }
                    },
                )
            }
        }
        OverlayScreen(overlay is Overlay.InviteToChannel && state.channel != null) {
            InviteToChannelScreen(
                state = state,
                busy = busy,
                onBack = { overlay = Overlay.ChannelInfo },
                onInvite = { ids ->
                    val channelId = state.channel?.channelId ?: return@InviteToChannelScreen
                    actions.command(inviteToChannelCommand(channelId, ids)) { sent ->
                        if (sent) overlay = Overlay.ChannelInfo
                    }
                },
            )
        }
        OverlayScreen(overlay is Overlay.Discussion && state.channel != null) {
            DiscussionPickerScreen(
                state = state,
                onBack = { overlay = Overlay.ChannelInfo },
                onPick = { groupId ->
                    val channelId = state.channel?.channelId ?: return@DiscussionPickerScreen
                    actions.command(
                        CoreJson.command("link_discussion_group", "channel_id" to channelId, "group_id" to groupId),
                    ) { linked -> if (linked) overlay = Overlay.ChannelInfo }
                },
            )
        }
        // Комментарии открывает ядро: ветка живёт в снимке, пока её не закроют.
        val thread = state.channel?.threadPostEventId?.takeIf { state.selectedContactId == state.channel.channelId }
        OverlayScreen(thread != null && overlay == Overlay.None) {
            CommentsScreen(
                state = state,
                actions = actions,
                onBack = { actions.command(CoreJson.command("open_comments", "post_event_id" to null), null) },
            )
        }
        OverlayScreen(overlay is Overlay.NewGroup) {
            NewGroupScreen(
                state = state,
                busy = busy,
                error = if (newChatSubmitted && !busy) state.statusMessage else null,
                onBack = { overlay = Overlay.None },
                onCreate = { name, members ->
                    newChatSubmitted = true
                    actions.command(createGroupCommand(name, members)) { created ->
                        if (created) overlay = Overlay.None
                    }
                },
            )
        }
        OverlayScreen(overlay is Overlay.GroupInfo && state.group != null) {
            GroupInfoScreen(
                state = state,
                actions = actions,
                onBack = { overlay = Overlay.None },
                onAddMembers = { overlay = Overlay.AddMembers },
                onOpenChat = { member ->
                    overlay = Overlay.None
                    if (member.isContact) {
                        actions.selectContact(member.userId)
                    } else {
                        // Незнакомцу уходит обычный запрос на общение в его публичный ящик.
                        actions.command(
                            CoreJson.command("add_contact", "query" to member.userId, "display_name" to member.displayName),
                            null,
                        )
                    }
                },
            )
        }
        OverlayScreen(overlay is Overlay.AddMembers && state.group != null) {
            AddMembersScreen(
                state = state,
                busy = busy,
                onBack = { overlay = Overlay.GroupInfo },
                onAdd = { ids ->
                    val groupId = state.group?.groupId ?: return@AddMembersScreen
                    actions.command(
                        CoreJson.command(
                            "add_group_members",
                            "group_id" to groupId,
                            "user_ids" to org.json.JSONArray(ids),
                        ),
                    ) { added -> if (added) overlay = Overlay.GroupInfo }
                },
            )
        }
        OverlayScreen(overlay is Overlay.Settings) {
            SettingsScreen(
                state = state,
                update = update,
                theme = theme,
                font = font,
                actions = actions,
                onThemeChange = onThemeChange,
                onFontChange = onFontChange,
                onOpenUpdate = { overlay = Overlay.Update },
                onBack = { overlay = Overlay.None },
            )
        }
        OverlayScreen(overlay is Overlay.Update && update.available != null) {
            update.available?.let { release ->
                UpdateScreen(
                    update = update,
                    release = release,
                    onInstall = actions.installUpdate,
                    onCancel = actions.cancelUpdate,
                    onLater = { actions.dismissUpdate(); overlay = Overlay.None },
                    onSkip = { actions.skipUpdate(); overlay = Overlay.None },
                    onBack = { overlay = Overlay.None },
                )
            }
        }
        OverlayScreen(overlay is Overlay.Themes) {
            ThemeScreen(
                current = theme,
                onPick = onThemeChange,
                onBack = { overlay = Overlay.Settings },
            )
        }
        OverlayScreen(overlay is Overlay.Profile && state.selectedContact != null) {
            state.selectedContact?.let { contact ->
                ContactProfileScreen(
                    contact = contact,
                    onBack = { overlay = Overlay.None },
                    onVerify = { actions.verifyContact(contact.userId, it) },
                    onDelete = { overlay = Overlay.None; actions.deleteContact(contact.userId) },
                )
            }
        }
        val forwarding = overlay as? Overlay.Forward
        OverlayScreen(forwarding != null) {
            forwarding?.let { request ->
                ForwardScreen(
                    state = state,
                    onBack = { overlay = Overlay.None },
                    onPick = { userId ->
                        overlay = Overlay.None
                        actions.forward(request.eventIds, userId)
                    },
                )
            }
        }
    }

    BackHandler(enabled = overlay != Overlay.None) {
        overlay = when (overlay) {
            is Overlay.Themes -> Overlay.Settings
            is Overlay.AddMembers -> Overlay.GroupInfo
            is Overlay.PickChannelAdmin, is Overlay.AdminRights,
            is Overlay.InviteToChannel, is Overlay.Discussion -> Overlay.ChannelInfo
            else -> Overlay.None
        }
    }
    val threadOpen = state.channel?.threadPostEventId != null && state.selectedContactId == state.channel.channelId
    BackHandler(enabled = overlay == Overlay.None && threadOpen) {
        actions.command(CoreJson.command("open_comments", "post_event_id" to null), null)
    }
}

/** Насколько список подтягивается из-за левого края, пока диалог уезжает вправо. */
private const val ListParallax = 0.3f
private const val ScrimAlpha = 0.32f
private const val ChatListBlurRadius = 24f

/** Порог возврата: половина ширины либо заметный бросок вправо. */
private const val BackDistanceFraction = 0.5f
private const val BackVelocity = 900f

/**
 * Пока диалог лежит поверх списка, касания не должны доходить до карточек под ним: фон сам
 * по себе не перехватывает ввод, и нажатие «сквозь» диалог открыло бы чужой чат.
 */
private fun Modifier.blockTouches(blocked: Boolean): Modifier =
    if (!blocked) this else this.pointerInput(Unit) {
        awaitPointerEventScope {
            while (true) {
                awaitPointerEvent(PointerEventPass.Initial).changes.forEach { it.consume() }
            }
        }
    }

@Composable
private fun OverlayScreen(visible: Boolean, content: @Composable () -> Unit) {
    AnimatedVisibility(
        visible = visible,
        enter = slideInHorizontally { it / 3 } + fadeIn(),
        exit = slideOutHorizontally { it / 3 } + fadeOut(),
    ) {
        // Полноэкранный раздел — тоже слой стекла: сияние фона продолжает просвечивать.
        AuroraBackground(Modifier.fillMaxSize()) { content() }
    }
}
