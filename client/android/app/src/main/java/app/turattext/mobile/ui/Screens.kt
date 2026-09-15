package app.turattext.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.foundation.Image
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchColors
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.Contact
import app.turattext.mobile.model.CoreJson
import app.turattext.mobile.model.Profile
import app.turattext.mobile.update.UpdateState

/** Боковое меню Telegram: градиентная шапка профиля и список разделов. */
@Composable
fun DrawerContent(
    profile: Profile,
    userId: String,
    theme: AppTheme,
    online: Boolean,
    onOpenThemes: () -> Unit,
    onNewChat: () -> Unit,
    onSettings: () -> Unit,
    onSync: () -> Unit,
) {
    val colors = Telegram.colors
    Column(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxWidth()
                .background(
                    Brush.linearGradient(
                        listOf(colors.accent.copy(alpha = 0.88f), colors.accentSoft.copy(alpha = 0.75f)),
                    ),
                )
                .statusBarsPadding()
                .padding(16.dp),
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Avatar(
                    name = profile.displayName.ifBlank { "Turat" },
                    key = userId,
                    avatarBase64 = profile.avatarBase64,
                    size = 62.dp,
                )
                Spacer(Modifier.weight(1f))
                Box(
                    Modifier.size(38.dp).clip(CircleShape).clickable(onClick = onOpenThemes),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(
                        painterResource(if (colors.night) R.drawable.ic_sun else R.drawable.ic_moon),
                        "Сменить тему",
                        Modifier.size(22.dp),
                        colors.onAccent,
                    )
                }
            }
            Spacer(Modifier.height(12.dp))
            Text(
                profile.displayName.ifBlank { "Без имени" },
                color = colors.onAccent,
                fontSize = 16.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (profile.username.isBlank()) shortId(userId) else "@${profile.username}",
                color = colors.onAccent.copy(alpha = .75f),
                fontSize = 13.sp,
                maxLines = 1,
            )
        }
        DrawerRow(R.drawable.ic_edit, "Новый диалог", onNewChat)
        DrawerRow(
            R.drawable.ic_sync,
            if (online) "Обновить сейчас" else "Подключиться к Node",
            onSync,
        )
        DrawerRow(R.drawable.ic_settings, "Настройки", onSettings)
        HorizontalDivider(Modifier.padding(vertical = 8.dp), color = colors.divider)
        DrawerRow(
            if (colors.night) R.drawable.ic_sun else R.drawable.ic_moon,
            "Тема: " + theme.title,
            onOpenThemes,
        )
    }
}

/** Выбор темы: карточки с настоящими цветами палитры — вид темы понятен до её включения. */
@Composable
fun ThemeScreen(current: AppTheme, onPick: (AppTheme) -> Unit, onBack: () -> Unit) {
    TelegramScreen("Тема", onBack) {
        item {
            Text(
                "Оформление применяется сразу и сохраняется на этом устройстве.",
                Modifier.padding(start = 18.dp, end = 18.dp, top = 16.dp, bottom = 4.dp),
                color = Telegram.colors.hint,
                fontSize = 14.sp,
            )
        }
        items(AppTheme.entries.chunked(2)) { pair ->
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 5.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                pair.forEach { theme ->
                    ThemeCard(theme, theme == current, Modifier.weight(1f)) { onPick(theme) }
                }
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}

/** Карточка темы: градиент окна, две «пилюли» акцентов и подпись. */
@Composable
private fun ThemeCard(
    theme: AppTheme,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val palette = theme.palette
    val shape = GlassShape.Card
    Column(
        modifier
            .clip(shape)
            .background(
                Brush.linearGradient(
                    listOf(palette.window, palette.accentSoft.copy(alpha = 0.55f), palette.chatBottom),
                ),
            )
            .border(
                if (selected) 2.dp else 1.dp,
                if (selected) Telegram.colors.accent else palette.glassRim,
                shape,
            )
            .clickable(onClick = onClick)
            .padding(12.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.width(30.dp).height(12.dp).clip(RoundedCornerShape(6.dp)).background(palette.accent))
            Spacer(Modifier.width(6.dp))
            Box(Modifier.width(16.dp).height(12.dp).clip(RoundedCornerShape(6.dp)).background(palette.accentSoft))
        }
        Spacer(Modifier.height(16.dp))
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                theme.title,
                Modifier.weight(1f),
                color = palette.text,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (selected) {
                Box(Modifier.size(8.dp).clip(CircleShape).background(palette.accent))
            }
        }
    }
}

@Composable
private fun DrawerRow(icon: Int, title: String, onClick: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 18.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(icon), null, Modifier.size(21.dp), colors.hint)
        Spacer(Modifier.width(20.dp))
        Text(title, color = colors.text, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

/** Полноэкранный раздел с шапкой — так открываются экраны в Telegram. */
@Composable
fun TelegramScreen(
    title: String,
    onBack: (() -> Unit)?,
    modifier: Modifier = Modifier,
    action: @Composable () -> Unit = {},
    content: LazyListScope.() -> Unit,
) {
    val colors = Telegram.colors
    Column(modifier.fillMaxSize()) {
        Column(Modifier.glass(colors, GlassShape.Header, raised = true).statusBarsPadding()) {
            Row(
                Modifier.fillMaxWidth().height(56.dp).padding(horizontal = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (onBack != null) {
                    IconButton(onBack) {
                        Icon(painterResource(R.drawable.ic_back), "Назад", Modifier.size(22.dp), colors.text)
                    }
                    Spacer(Modifier.width(6.dp))
                } else {
                    Spacer(Modifier.width(14.dp))
                }
                Text(
                    title,
                    Modifier.weight(1f),
                    color = colors.text,
                    fontSize = 19.sp,
                    fontWeight = FontWeight.Medium,
                )
                action()
            }
        }
        LazyColumn(
            Modifier.fillMaxSize().imePadding(),
            contentPadding = PaddingValues(bottom = 32.dp),
            content = content,
        )
    }
}

@Composable
fun SectionTitle(text: String) {
    Text(
        text,
        Modifier.fillMaxWidth().padding(start = 18.dp, end = 18.dp, top = 18.dp, bottom = 6.dp),
        color = Telegram.colors.accent,
        fontSize = 14.sp,
        fontWeight = FontWeight.Medium,
    )
}

@Composable
fun SectionRow(icon: Int?, title: String, subtitle: String? = null, danger: Boolean = false, onClick: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 18.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(painterResource(icon), null, Modifier.size(20.dp), if (danger) colors.danger else colors.hint)
            Spacer(Modifier.width(18.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, color = if (danger) colors.danger else colors.text, fontSize = 15.sp)
            if (subtitle != null) {
                Text(subtitle, color = colors.hint, fontSize = 13.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

@Composable
fun SectionSwitch(icon: Int, title: String, subtitle: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().clickable { onChange(!checked) }.padding(horizontal = 18.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(icon), null, Modifier.size(20.dp), colors.hint)
        Spacer(Modifier.width(18.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.text, fontSize = 15.sp)
            Text(subtitle, color = colors.hint, fontSize = 13.sp)
        }
        Switch(checked = checked, onCheckedChange = onChange, colors = turatSwitchColors())
    }
}

/**
 * Цвета переключателя. Material3 по умолчанию красит выключенный бегунок и дорожку почти
 * одинаковым серым, и на тёмной панели переключатель выглядит выключенным «в никуда».
 */
@Composable
fun turatSwitchColors(): SwitchColors {
    val colors = Telegram.colors
    return SwitchDefaults.colors(
        checkedThumbColor = colors.onAccent,
        checkedTrackColor = colors.accent,
        checkedBorderColor = colors.accent,
        uncheckedThumbColor = colors.hint,
        uncheckedTrackColor = colors.field,
        uncheckedBorderColor = colors.divider,
        disabledCheckedThumbColor = colors.onAccent.copy(alpha = .6f),
        disabledCheckedTrackColor = colors.accent.copy(alpha = .4f),
        disabledUncheckedThumbColor = colors.hint.copy(alpha = .5f),
        disabledUncheckedTrackColor = colors.field.copy(alpha = .5f),
    )
}

@Composable
fun TelegramField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    singleLine: Boolean = true,
    password: Boolean = false,
    imeAction: ImeAction = ImeAction.Next,
) {
    val colors = Telegram.colors
    TextField(
        value = value,
        onValueChange = onValueChange,
        label = { Text(label) },
        singleLine = singleLine,
        visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(imeAction = imeAction),
        modifier = modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 2.dp),
        colors = TextFieldDefaults.colors(
            focusedContainerColor = Color.Transparent,
            unfocusedContainerColor = Color.Transparent,
            disabledContainerColor = Color.Transparent,
            focusedIndicatorColor = colors.accent,
            unfocusedIndicatorColor = colors.divider,
            focusedLabelColor = colors.accent,
            unfocusedLabelColor = colors.hint,
            focusedTextColor = colors.text,
            unfocusedTextColor = colors.text,
            cursorColor = colors.accent,
        ),
    )
}

@Composable
fun TelegramButton(text: String, onClick: () -> Unit, enabled: Boolean = true, modifier: Modifier = Modifier) {
    Button(
        onClick = onClick,
        enabled = enabled,
        shape = GlassShape.Capsule,
        colors = ButtonDefaults.buttonColors(
            containerColor = Telegram.colors.accent.copy(alpha = 0.92f),
            contentColor = Telegram.colors.onAccent,
            disabledContainerColor = Telegram.colors.accent.copy(alpha = .35f),
            disabledContentColor = Telegram.colors.onAccent.copy(alpha = .6f),
        ),
        modifier = modifier,
    ) {
        Text(text, fontSize = 15.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
fun SecondaryButton(text: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val colors = Telegram.colors
    Box(
        modifier.clip(GlassShape.Capsule)
            .glass(colors, GlassShape.Capsule)
            .clickable(onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 11.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, color = colors.accent, fontSize = 14.sp, fontWeight = FontWeight.Medium)
    }
}

/** Экран нового диалога: поиск по @username или полному UserID. */
@Composable
fun NewChatScreen(
    onBack: () -> Unit,
    busy: Boolean,
    error: String?,
    onCreate: (String) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    TelegramScreen("Новый диалог", onBack) {
        item {
            Text(
                "Введите @username собеседника или его полный UserID — Turat найдёт его в сети и создаст защищённый диалог.",
                Modifier.padding(18.dp),
                color = Telegram.colors.hint,
                fontSize = 14.sp,
            )
        }
        item {
            TelegramField(
                value = query,
                onValueChange = { query = it },
                label = "@username или UserID",
                imeAction = ImeAction.Done,
            )
        }
        item {
            TelegramButton(
                if (busy) "Поиск…" else "Начать диалог",
                { onCreate(query) },
                enabled = query.isNotBlank() && !busy,
                modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 18.dp),
            )
        }
        if (!error.isNullOrBlank()) {
            item {
                Text(
                    error,
                    Modifier.padding(horizontal = 18.dp),
                    color = Telegram.colors.danger,
                    fontSize = 14.sp,
                )
            }
        }
    }
}

/** Выбор чата для пересылки сообщений. */
@Composable
fun ForwardScreen(state: AppSnapshot, onBack: () -> Unit, onPick: (String) -> Unit) {
    val colors = Telegram.colors
    val targets = state.chats.filterNot { it.contact.pending }
    TelegramScreen("Переслать", onBack) {
        if (targets.isEmpty()) {
            item {
                Text(
                    "Нет принятых диалогов, куда можно переслать сообщения.",
                    Modifier.padding(18.dp),
                    color = colors.hint,
                    fontSize = 14.sp,
                )
            }
        }
        items(targets, key = { it.contact.userId }) { chat ->
            Row(
                Modifier.fillMaxWidth().clickable { onPick(chat.contact.userId) }
                    .padding(horizontal = 14.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Avatar(chat.contact.displayName, chat.contact.userId, chat.contact.avatarBase64, 48.dp)
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
            }
        }
    }
}

/** Первый запуск: имя и username, как в приветственном экране Telegram. */
@Composable
fun OnboardingScreen(profile: Profile, onSave: (String, String) -> Unit) {
    val colors = Telegram.colors
    var username by remember { mutableStateOf(profile.username) }
    var name by remember { mutableStateOf(profile.displayName) }
    Column(
        Modifier.fillMaxSize().background(colors.window).statusBarsPadding().imePadding()
            .padding(horizontal = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(48.dp))
        Image(
            painter = painterResource(R.drawable.turat_logo),
            contentDescription = "Логотип Turat",
            modifier = Modifier.size(112.dp),
        )
        Spacer(Modifier.height(18.dp))
        Text("Turat", color = colors.text, fontSize = 24.sp, fontWeight = FontWeight.Medium)
        Spacer(Modifier.height(8.dp))
        Text(
            "Придумайте видимое имя и username — их увидят собеседники. Ключи остаются на устройстве.",
            Modifier.padding(horizontal = 24.dp),
            color = colors.hint,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(22.dp))
        TelegramField(name, { name = it }, "Видимое имя")
        TelegramField(username, { username = it }, "Username", imeAction = ImeAction.Done)
        Spacer(Modifier.height(22.dp))
        TelegramButton(
            "Продолжить",
            { onSave(username, name) },
            enabled = name.isNotBlank(),
            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
        )
    }
}

/** Профиль собеседника: шапка с аватаром и разделы с данными. */
@Composable
fun ContactProfileScreen(
    contact: Contact,
    onBack: () -> Unit,
    onVerify: (Boolean) -> Unit,
    onDelete: () -> Unit,
) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    TelegramScreen(contact.displayName, onBack) {
        item {
            Column(
                Modifier.fillMaxWidth().padding(14.dp)
                    .glass(colors, GlassShape.Panel, raised = true)
                    .padding(vertical = 20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Avatar(contact.displayName, contact.userId, contact.avatarBase64, 96.dp)
                Spacer(Modifier.height(12.dp))
                Text(contact.displayName, color = colors.text, fontSize = 20.sp, fontWeight = FontWeight.Medium)
                Text(
                    presenceOf(contact),
                    color = if (isOnline(contact)) colors.accent else colors.hint,
                    fontSize = 14.sp,
                )
            }
        }
        item { SectionTitle("Данные") }
        if (contact.username != null) {
            item {
                SectionRow(R.drawable.ic_person, "@${contact.username}", "username") {
                    clipboard.setText(AnnotatedString("@${contact.username}"))
                }
            }
        }
        item {
            SectionRow(R.drawable.ic_copy, shortId(contact.userId), "UserID · нажмите, чтобы скопировать") {
                clipboard.setText(AnnotatedString(contact.userId))
            }
        }
        if (!contact.about.isNullOrBlank()) {
            item { SectionRow(null, contact.about, "о себе") {} }
        }
        item { SectionTitle("Безопасность") }
        item {
            SectionSwitch(
                R.drawable.ic_verified,
                "Fingerprint сверен",
                if (contact.verified) "Ключи собеседника подтверждены вручную"
                else "Сверьте ключи лично, чтобы исключить подмену",
                contact.verified,
                onVerify,
            )
        }
        item { SectionTitle("Диалог") }
        item { SectionRow(R.drawable.ic_delete, "Удалить чат", danger = true, onClick = onDelete) }
    }
}

private enum class SettingsCategory(val title: String) {
    Profile("Профиль"),
    Appearance("Оформление"),
    Connection("Приватность и сеть"),
    Data("Данные и устройства"),
}

/** Компактные стеклянные сегменты вместо длинного непрерывного экрана настроек. */
@Composable
private fun SettingsCategoryPicker(selected: SettingsCategory, onSelect: (SettingsCategory) -> Unit) {
    val colors = Telegram.colors
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 12.dp)
            .glass(colors, GlassShape.Panel, raised = true)
            .padding(6.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        SettingsCategory.entries.chunked(2).forEach { pair ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                pair.forEach { category ->
                    val active = category == selected
                    Box(
                        Modifier.weight(1f).clip(GlassShape.Capsule)
                            .background(if (active) colors.accentSoft else Color.Transparent)
                            .border(
                                1.dp,
                                if (active) colors.accent.copy(alpha = .75f) else colors.glassRim.copy(alpha = .5f),
                                GlassShape.Capsule,
                            )
                            .clickable { onSelect(category) }
                            .padding(horizontal = 12.dp, vertical = 11.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            category.title,
                            color = if (active) colors.text else colors.hint,
                            fontSize = 13.sp,
                            fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
            }
        }
    }
}

/** Превью гарнитуры остаётся стеклянным и показывает реальный вид текста до выбора. */
@Composable
private fun FontCard(
    font: AppFont,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val colors = Telegram.colors
    val shape = GlassShape.Card
    Column(
        // glass() уже рисует собственную кромку. Отключаем её здесь и оставляем
        // один общий контур, чтобы выбранная карточка не выглядела вложенной в рамку.
        modifier.clip(shape).glass(colors, shape, raised = true, rim = false)
            .border(if (selected) 2.dp else 1.dp, if (selected) colors.accent else colors.glassRim, shape)
            .clickable(onClick = onClick)
            .padding(13.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                font.title,
                Modifier.weight(1f),
                color = colors.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                fontFamily = font.family,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text("Aa Бб", color = colors.accent, fontSize = 16.sp, fontFamily = font.family)
        }
    }
}

/** Настройки: четыре категории вместо одного перегруженного полотна. */
@Composable
fun SettingsScreen(
    state: AppSnapshot,
    update: UpdateState,
    theme: AppTheme,
    font: AppFont,
    actions: AppActions,
    onThemeChange: (AppTheme) -> Unit,
    onFontChange: (AppFont) -> Unit,
    onOpenUpdate: () -> Unit,
    onBack: () -> Unit,
) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    var username by remember { mutableStateOf(state.profile.username) }
    var name by remember { mutableStateOf(state.profile.displayName) }
    var about by remember { mutableStateOf(state.profile.about) }
    var node by remember { mutableStateOf(state.settings.bootstrapUrl) }
    var passphrase by remember { mutableStateOf("") }
    var revokeId by remember { mutableStateOf("") }
    var category by remember { mutableStateOf(SettingsCategory.Profile) }

    TelegramScreen("Настройки", onBack) {
        // Категории — обычный элемент списка: они прокручиваются вместе с остальными
        // настройками и не перекрывают содержимое закреплённой шапкой.
        item { SettingsCategoryPicker(category) { category = it } }

        if (category == SettingsCategory.Profile) {
        item {
            Column(
                Modifier.fillMaxWidth().padding(14.dp)
                    .glass(colors, GlassShape.Panel, raised = true)
                    .padding(vertical = 20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Box(contentAlignment = Alignment.BottomEnd) {
                    Avatar(
                        state.profile.displayName.ifBlank { "Turat" },
                        state.identity.userId,
                        state.profile.avatarBase64,
                        92.dp,
                        modifier = Modifier.clickable(onClick = actions.pickAvatar),
                    )
                    Box(
                        Modifier.size(30.dp).clip(CircleShape).background(colors.accent)
                            .clickable(onClick = actions.pickAvatar),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(
                            painterResource(R.drawable.ic_camera),
                            "Сменить фото",
                            Modifier.size(16.dp),
                            colors.onAccent,
                        )
                    }
                }
                Spacer(Modifier.height(12.dp))
                Text(
                    state.profile.displayName.ifBlank { "Без имени" },
                    color = colors.text,
                    fontSize = 20.sp,
                    fontWeight = FontWeight.Medium,
                )
                Text(
                    if (state.online) "в сети · сквозное шифрование" else "локальный режим",
                    color = if (state.online) colors.accent else colors.hint,
                    fontSize = 13.sp,
                )
            }
        }

        item { SectionTitle("Профиль") }
        item {
            SectionRow(R.drawable.ic_copy, shortId(state.identity.userId), "ваш UserID · скопировать") {
                clipboard.setText(AnnotatedString(state.identity.userId))
            }
        }
        item { TelegramField(name, { name = it }, "Видимое имя") }
        item { TelegramField(username, { username = it }, "Username") }
        item { TelegramField(about, { about = it }, "О себе", singleLine = false) }
        item {
            TelegramButton(
                "Сохранить и опубликовать",
                {
                    actions.command(
                        CoreJson.command(
                            "save_profile",
                            "username" to username,
                            "display_name" to name,
                            "about" to about,
                            "avatar_base64" to state.profile.avatarBase64,
                        ),
                    ) { saved -> if (saved) actions.command(CoreJson.command("publish_profile"), null) }
                },
                modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
            )
        }
        }

        if (category == SettingsCategory.Appearance) {
        item { SectionTitle("Тема") }
        items(AppTheme.entries.chunked(2)) { pair ->
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 5.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                pair.forEach { preview ->
                    ThemeCard(preview, preview == theme, Modifier.weight(1f)) { onThemeChange(preview) }
                }
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }

        item { SectionTitle("Шрифт") }
        items(AppFont.entries.chunked(2)) { pair ->
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 5.dp),
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                pair.forEach { choice ->
                    FontCard(choice, choice == font, Modifier.weight(1f)) { onFontChange(choice) }
                }
                if (pair.size == 1) Spacer(Modifier.weight(1f))
            }
        }
        }

        if (category == SettingsCategory.Connection) {
        item { SectionTitle("Конфиденциальность") }
        item {
            SectionSwitch(
                R.drawable.ic_person,
                "Последняя активность",
                if (state.settings.publishPresence)
                    "Собеседники видят «в сети» и время последнего визита"
                else "Скрыта: у собеседников всегда «был(а) недавно»",
                state.settings.publishPresence,
                actions.setPresence,
            )
        }

        item { SectionTitle("Сеть") }
        item {
            Text(
                if (state.online) "Клиент подключается к Node сам и обновляет диалоги в фоне."
                else "Нет связи с Node. Проверьте адрес и подключение — клиент повторит попытку сам.",
                Modifier.padding(horizontal = 18.dp, vertical = 2.dp),
                color = if (state.online) colors.hint else colors.danger,
                fontSize = 13.sp,
            )
        }
        item { TelegramField(node, { node = it }, "Адрес Mailbox Node") }
        item {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                TelegramButton(
                    "Сохранить Node",
                    { actions.command(CoreJson.command("connect", "bootstrap_url" to node), null) },
                    enabled = node.isNotBlank() && node != state.settings.bootstrapUrl,
                )
                SecondaryButton("Метаданные: ${state.settings.metadataProtection}") {
                    actions.command(CoreJson.command("cycle_metadata_protection"), null)
                }
            }
        }
        }

        if (category == SettingsCategory.Data) {
        item { SectionTitle("Устройства и резервные копии") }
        item { TelegramField(passphrase, { passphrase = it }, "Пароль пакета", password = true) }
        item {
            TwoButtons(
                "Создать backup", { actions.export("create_backup", passphrase, "Turat.ttbackup") },
                "Восстановить", { actions.importFile("restore_backup", passphrase, arrayOf("*/*")) },
            )
        }
        item {
            TwoButtons(
                "Связать устройство", { actions.export("create_device_link", passphrase, "Turat.ttlink") },
                "Импорт связи", { actions.importFile("import_device_link", passphrase, arrayOf("*/*")) },
            )
        }
        item { TelegramField(revokeId, { revokeId = it }, "DeviceID потерянного устройства") }
        item {
            SectionRow(
                R.drawable.ic_delete,
                "Отозвать устройство",
                if (revokeId.isBlank()) "Сначала введите DeviceID" else null,
                danger = true,
            ) {
                if (revokeId.isNotBlank()) {
                    actions.command(CoreJson.command("revoke_device", "device_id" to revokeId), null)
                    revokeId = ""
                }
            }
        }

        item { SectionTitle("Передача без прямого подключения") }
        item {
            TwoButtons(
                "Экспорт сообщений", { actions.export("export_portable", "", "messages.ttenv") },
                "Доставить пакет", { actions.importFile("import_portable", "", arrayOf("*/*")) },
            )
        }
        item {
            TwoButtons(
                "Экспорт сети", { actions.export("export_discovery", "", "network.ttbridge") },
                "Импорт сети", { actions.importFile("import_discovery", "", arrayOf("*/*")) },
            )
        }

        item { SectionTitle("Обновления") }
        item {
            SectionRow(
                R.drawable.ic_sync,
                if (update.checking) "Проверяем…" else "Проверить обновления",
                listOfNotNull("Установлена версия ${update.currentVersion}", update.message).joinToString(" · "),
            ) { actions.checkUpdates() }
        }
        update.available?.let { release ->
            item {
                SectionRow(
                    R.drawable.ic_download,
                    "Доступна версия ${release.version}",
                    "Что нового и обновление",
                    onClick = onOpenUpdate,
                )
            }
        }
        }

        item {
            Text(
                state.statusMessage,
                Modifier.fillMaxWidth().padding(18.dp).navigationBarsPadding(),
                color = colors.hint,
                fontSize = 12.sp,
                textAlign = TextAlign.Center,
            )
        }
        item {
            Text(
                "UserID: ${state.identity.userId}",
                Modifier.fillMaxWidth().padding(horizontal = 18.dp),
                color = colors.hint.copy(alpha = .7f),
                fontSize = 10.sp,
                fontFamily = FontFamily.Monospace,
                textAlign = TextAlign.Center,
            )
        }
    }
}

@Composable
private fun TwoButtons(left: String, onLeft: () -> Unit, right: String, onRight: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        SecondaryButton(left, Modifier.weight(1f), onLeft)
        SecondaryButton(right, Modifier.weight(1f), onRight)
    }
}
