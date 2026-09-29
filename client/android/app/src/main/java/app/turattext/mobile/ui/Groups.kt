package app.turattext.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.Chat
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.GroupInfo
import app.turattext.mobile.model.GroupMember
import app.turattext.mobile.model.roleRank
import org.json.JSONArray

/** «1 участник», «3 участника», «11 участников». */
fun membersLabel(count: Int): String {
    val tens = count % 100
    val units = count % 10
    val word = when {
        tens in 11..14 -> "участников"
        units == 1 -> "участник"
        units in 2..4 -> "участника"
        else -> "участников"
    }
    return "$count $word"
}

/** Подтверждение необратимого действия с группой. */
private data class Confirmation(val title: String, val text: String, val action: () -> Unit)

@Composable
private fun ConfirmationDialog(confirmation: Confirmation?, onDismiss: () -> Unit) {
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

/** Принятые личные контакты: только их можно пригласить в группу. */
private fun acceptedContacts(state: AppSnapshot): List<Chat> =
    state.chats.filter { !it.isGroup && !it.contact.pending }

/** Строка выбора участника: аватар, имя и круглая отметка справа. */
@Composable
private fun PickRow(chat: Chat, checked: Boolean, onToggle: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onToggle).padding(horizontal = 14.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Avatar(chat.contact.displayName, chat.contact.userId, chat.contact.avatarBase64, 46.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                chat.contact.displayName,
                color = colors.text,
                fontSize = 16.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                chat.contact.username?.let { "@$it" } ?: shortId(chat.contact.userId),
                color = colors.hint,
                fontSize = 13.sp,
                maxLines = 1,
            )
        }
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

/** Новая группа: название и участники из принятых контактов. */
@Composable
fun NewGroupScreen(
    state: AppSnapshot,
    busy: Boolean,
    error: String?,
    onBack: () -> Unit,
    onCreate: (String, List<String>) -> Unit,
) {
    val colors = Telegram.colors
    var name by remember { mutableStateOf("") }
    val selected = remember { mutableStateListOf<String>() }
    val contacts = acceptedContacts(state)
    TelegramScreen("Новая группа", onBack) {
        item {
            Text(
                "Участники — ваши принятые контакты. Каждый получит приглашение и сам решит, вступать ли. " +
                    "Сообщения шифруются отдельно для каждого участника.",
                Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
                color = colors.hint,
                fontSize = 14.sp,
            )
        }
        item { TelegramField(name, { if (it.length <= 64) name = it }, "Название группы", imeAction = ImeAction.Done) }
        item {
            TelegramButton(
                if (busy) "Создание…" else "Создать группу",
                { onCreate(name, selected.toList()) },
                enabled = name.isNotBlank() && selected.isNotEmpty() && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        if (!error.isNullOrBlank()) {
            item { Text(error, Modifier.padding(horizontal = 18.dp), color = colors.danger, fontSize = 14.sp) }
        }
        item { SectionTitle(if (selected.isEmpty()) "Участники" else "Участники · выбрано ${selected.size}") }
        if (contacts.isEmpty()) {
            item {
                Text(
                    "Сначала начните диалог хотя бы с одним собеседником.",
                    Modifier.padding(horizontal = 18.dp),
                    color = colors.hint,
                    fontSize = 14.sp,
                )
            }
        }
        items(contacts, key = { it.contact.userId }) { chat ->
            val id = chat.contact.userId
            PickRow(chat, id in selected) { if (id in selected) selected.remove(id) else selected.add(id) }
        }
    }
}

/** Добавление участников в уже существующую группу. */
@Composable
fun AddMembersScreen(state: AppSnapshot, busy: Boolean, onBack: () -> Unit, onAdd: (List<String>) -> Unit) {
    val colors = Telegram.colors
    val group = state.group ?: return
    val present = remember(group.members) { group.members.map { it.userId }.toSet() }
    val candidates = acceptedContacts(state).filterNot { it.contact.userId in present }
    val selected = remember(group.groupId) { mutableStateListOf<String>() }
    TelegramScreen("Добавить участников", onBack) {
        item {
            TelegramButton(
                when {
                    busy -> "Добавление…"
                    selected.isEmpty() -> "Отметьте, кого добавить"
                    else -> "Добавить: ${selected.size}"
                },
                { onAdd(selected.toList()) },
                enabled = selected.isNotEmpty() && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        if (candidates.isEmpty()) {
            item {
                Text(
                    "Все ваши контакты уже в этой группе.",
                    Modifier.padding(horizontal = 18.dp),
                    color = colors.hint,
                    fontSize = 14.sp,
                )
            }
        }
        items(candidates, key = { it.contact.userId }) { chat ->
            val id = chat.contact.userId
            PickRow(chat, id in selected) { if (id in selected) selected.remove(id) else selected.add(id) }
        }
    }
}

/**
 * Страница группы: данные, права участников, состав и роли.
 *
 * Права здесь только прячут недоступное: решает ядро, а за ним — устройство каждого
 * участника, которое проверяет полномочия автора любого изменения группы.
 */
@Composable
fun GroupInfoScreen(
    state: AppSnapshot,
    actions: AppActions,
    onBack: () -> Unit,
    onAddMembers: () -> Unit,
    onOpenChat: (GroupMember) -> Unit,
) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    val group = state.group ?: return
    var confirmation by remember { mutableStateOf<Confirmation?>(null) }
    var name by remember(group.groupId, group.epoch) { mutableStateOf(group.name) }
    var about by remember(group.groupId, group.epoch) { mutableStateOf(group.about) }
    val active = !group.left && !group.pendingInvite && group.myRole != null

    fun command(name: String, vararg fields: Pair<String, Any?>) =
        actions.command(CoreJson.command(name, "group_id" to group.groupId, *fields), null)

    TelegramScreen("Группа", onBack) {
        item {
            Column(
                Modifier.fillMaxWidth().padding(14.dp)
                    .glass(colors, GlassShape.Panel, raised = true)
                    .padding(vertical = 20.dp, horizontal = 16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(contentAlignment = Alignment.BottomEnd) {
                    Avatar(
                        group.name,
                        group.groupId,
                        group.avatarBase64,
                        96.dp,
                        modifier = if (group.canEditInfo) {
                            Modifier.clickable { actions.pickGroupAvatar(group.groupId) }
                        } else {
                            Modifier
                        },
                    )
                    if (group.canEditInfo) {
                        Box(
                            Modifier.size(30.dp).clip(CircleShape).background(colors.accent)
                                .clickable { actions.pickGroupAvatar(group.groupId) },
                            contentAlignment = Alignment.Center,
                        ) {
                            Icon(painterResource(R.drawable.ic_camera), "Сменить фото", Modifier.size(16.dp), colors.onAccent)
                        }
                    }
                }
                Spacer(Modifier.height(12.dp))
                Text(
                    group.name,
                    color = colors.text,
                    fontSize = 20.sp,
                    fontWeight = FontWeight.Medium,
                    textAlign = TextAlign.Center,
                )
                Text(
                    when {
                        group.left -> "вы не участник группы"
                        group.pendingInvite -> "приглашение ещё не принято"
                        else -> "${membersLabel(group.members.size)} · ${roleTitle(group.myRole)}"
                    },
                    color = colors.hint,
                    fontSize = 14.sp,
                )
                if (group.about.isNotBlank()) {
                    Spacer(Modifier.height(8.dp))
                    Text(group.about, color = colors.text, fontSize = 14.sp, textAlign = TextAlign.Center)
                }
            }
        }

        if (group.canEditInfo) {
            item { SectionTitle("Данные группы") }
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
                                "update_group_info",
                                "name" to name.trim(),
                                "about" to about.trim(),
                                "avatar_base64" to group.avatarBase64,
                            )
                        },
                        enabled = name.isNotBlank() && (name.trim() != group.name || about.trim() != group.about),
                    )
                    if (group.avatarBase64 != null) {
                        SecondaryButton("Убрать фото") {
                            command(
                                "update_group_info",
                                "name" to group.name,
                                "about" to group.about,
                                "avatar_base64" to null,
                            )
                        }
                    }
                }
            }
        }

        // Права участников меняют администраторы — те же, кто может исключать.
        if (group.canRemoveMembers) {
            item { SectionTitle("Права участников") }
            item {
                SectionSwitch(
                    R.drawable.ic_add,
                    "Приглашать участников",
                    if (group.membersCanInvite) "Могут все участники" else "Только администраторы",
                    group.membersCanInvite,
                ) { checked ->
                    command(
                        "set_group_permissions",
                        "members_can_invite" to checked,
                        "members_can_edit_info" to group.membersCanEditInfo,
                    )
                }
            }
            item {
                SectionSwitch(
                    R.drawable.ic_edit,
                    "Менять название и фото",
                    if (group.membersCanEditInfo) "Могут все участники" else "Только администраторы",
                    group.membersCanEditInfo,
                ) { checked ->
                    command(
                        "set_group_permissions",
                        "members_can_invite" to group.membersCanInvite,
                        "members_can_edit_info" to checked,
                    )
                }
            }
        }

        item { SectionTitle(membersLabel(group.members.size).replaceFirstChar { it.uppercase() }) }
        if (group.canInvite) {
            item { SectionRow(R.drawable.ic_add, "Добавить участников", onClick = onAddMembers) }
        }
        items(group.members, key = { it.userId }) { member ->
            MemberRow(
                group = group,
                member = member,
                onOpenChat = { onOpenChat(member) },
                onRole = { role ->
                    command("set_group_role", "user_id" to member.userId, "role" to role)
                },
                onTransfer = {
                    confirmation = Confirmation(
                        "Передать владение?",
                        "${member.displayName} станет владельцем группы, а вы — администратором. " +
                            "Вернуть владение сможет только новый владелец.",
                    ) { command("transfer_group_ownership", "user_id" to member.userId) }
                },
                onRemove = {
                    confirmation = Confirmation(
                        "Исключить участника?",
                        "${member.displayName} перестанет получать новые сообщения группы.",
                    ) { command("remove_group_member", "user_id" to member.userId) }
                },
            )
        }

        item { SectionTitle("Безопасность") }
        item {
            Text(
                "Сообщения группы шифруются отдельно для каждого участника его личным сквозным каналом. " +
                    "Исключённый участник не получит ни одного нового сообщения. Права на каждое изменение " +
                    "состава проверяет устройство каждого участника. «Ещё не подтвердил(а) участие» — " +
                    "от человека не пришло ни одного подписанного события группы.",
                Modifier.padding(horizontal = 18.dp, vertical = 4.dp),
                color = colors.hint,
                fontSize = 13.sp,
            )
        }
        item {
            SectionRow(R.drawable.ic_copy, shortId(group.groupId), "GroupID · нажмите, чтобы скопировать") {
                clipboard.setText(AnnotatedString(group.groupId))
            }
        }

        item { SectionTitle("Группа") }
        if (active) {
            item {
                SectionRow(R.drawable.ic_close, "Покинуть группу", danger = true) {
                    confirmation = Confirmation(
                        "Покинуть группу?",
                        (if (group.myRole == "owner") "Владение перейдёт к старшему из оставшихся участников. " else "") +
                            "История останется на этом устройстве, но новые сообщения приходить не будут.",
                    ) { actions.command(CoreJson.command("leave_group", "group_id" to group.groupId), null) }
                }
            }
        }
        item {
            SectionRow(R.drawable.ic_delete, "Удалить группу", danger = true) {
                confirmation = Confirmation(
                    "Удалить группу?",
                    if (active) "Вы покинете группу, а её история будет удалена с этого устройства."
                    else "История группы будет удалена с этого устройства.",
                ) {
                    onBack()
                    actions.deleteContact(group.groupId)
                }
            }
        }
    }
    ConfirmationDialog(confirmation) { confirmation = null }
}

private fun roleTitle(role: String?) = when (role) {
    "owner" -> "вы владелец"
    "admin" -> "вы администратор"
    else -> "вы участник"
}

@Composable
private fun MemberRow(
    group: GroupInfo,
    member: GroupMember,
    onOpenChat: () -> Unit,
    onRole: (String) -> Unit,
    onTransfer: () -> Unit,
    onRemove: () -> Unit,
) {
    val colors = Telegram.colors
    var menu by remember { mutableStateOf(false) }
    val canRemove = group.canRemoveMembers && member.rank < roleRank(group.myRole)
    Box {
        Row(
            Modifier.fillMaxWidth()
                .clickable(enabled = !member.isSelf) { menu = true }
                .padding(horizontal = 14.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Avatar(member.displayName, member.userId, member.avatarBase64, 44.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    member.displayName,
                    color = colors.text,
                    fontSize = 16.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    when {
                        member.isSelf -> "это вы"
                        !member.confirmed -> "ещё не подтвердил(а) участие"
                        member.isContact -> "в ваших контактах"
                        else -> "добавил(а) ${member.addedByName}"
                    },
                    color = if (!member.confirmed && !member.isSelf) colors.danger.copy(alpha = .8f) else colors.hint,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            val role = when (member.role) {
                "owner" -> "владелец"
                "admin" -> "админ"
                else -> null
            }
            if (role != null) {
                Box(
                    Modifier.clip(RoundedCornerShape(10.dp)).background(colors.accentSoft)
                        .padding(horizontal = 8.dp, vertical = 3.dp),
                ) {
                    Text(role, color = colors.text, fontSize = 12.sp, fontWeight = FontWeight.Medium)
                }
            }
        }
        DropdownMenu(menu, { menu = false }) {
            MemberMenuItem("Написать лично") { menu = false; onOpenChat() }
            if (group.canManageAdmins && member.role == "member") {
                MemberMenuItem("Назначить администратором") { menu = false; onRole("admin") }
            }
            if (group.canManageAdmins && member.role == "admin") {
                MemberMenuItem("Снять права администратора") { menu = false; onRole("member") }
            }
            if (group.canManageAdmins && member.role != "owner") {
                MemberMenuItem("Передать владение") { menu = false; onTransfer() }
            }
            if (canRemove) {
                MemberMenuItem("Исключить из группы", danger = true) { menu = false; onRemove() }
            }
        }
    }
}

@Composable
private fun MemberMenuItem(title: String, danger: Boolean = false, onClick: () -> Unit) {
    DropdownMenuItem(
        text = { Text(title, color = if (danger) Telegram.colors.danger else Telegram.colors.text) },
        onClick = onClick,
    )
}

/** Команда создания группы для ядра. */
fun createGroupCommand(name: String, memberIds: List<String>): String = CoreJson.command(
    "create_group",
    "name" to name,
    "about" to "",
    "avatar_base64" to null,
    "member_ids" to JSONArray(memberIds),
)

/** Подтверждение удаления или выхода — общее для списка чатов и шапки диалога. */
@Composable
fun GroupDeleteDialog(chat: Chat?, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    chat ?: return
    val confirmation = if (chat.isChannel) {
        Confirmation(
            "Удалить канал?",
            when {
                chat.channelRole == "owner" && !chat.groupLeft ->
                    "Вы владелец: сначала передайте канал другому администратору или удалите его у всех в карточке канала."
                !chat.groupLeft && !chat.contact.pending ->
                    "Вы отпишетесь, а история канала будет удалена с этого устройства."
                else -> "История канала будет удалена с этого устройства."
            },
            onConfirm,
        )
    } else {
        Confirmation(
            "Удалить группу?",
            if (chat.canWrite) "Вы покинете группу, а её история будет удалена с этого устройства."
            else "История группы будет удалена с этого устройства.",
            onConfirm,
        )
    }
    ConfirmationDialog(confirmation, onDismiss)
}
