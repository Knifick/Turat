package app.turattext.mobile.ui

import android.os.Build
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.AppSnapshot
import app.turattext.mobile.model.CoreJson
import java.text.DateFormat
import java.util.Date

/** Команда ядру с ответом «получилось или нет»: ошибка тогда в `statusMessage` снимка. */
typealias AccountRunner = (String, (Boolean) -> Unit) -> Unit

private const val MinPasswordLength = 8

/** Имя этого устройства в списке сеансов: «Android · Samsung SM-S918B». */
fun androidDeviceName(): String {
    val maker = Build.MANUFACTURER.orEmpty().replaceFirstChar { it.uppercase() }
    val model = Build.MODEL.orEmpty()
    val device = if (model.startsWith(maker, ignoreCase = true)) model else "$maker $model".trim()
    return "Android · ${device.ifBlank { "телефон" }}"
}

private enum class AccountMode { Welcome, Register, Login, Recover }

/**
 * Вход в Turat: регистрация, вход по username и паролю, восстановление по ключу.
 *
 * Для установки, где переписка была ещё до аккаунтов (`legacy`), экран предлагает защитить её
 * паролем; это можно отложить ([onLater]), но войти в другой аккаунт — только удалив её.
 */
@Composable
fun AccountScreen(state: AppSnapshot, busy: Boolean, run: AccountRunner, onLater: (() -> Unit)?) {
    val colors = Telegram.colors
    val legacy = state.account.state == "legacy"
    var mode by remember { mutableStateOf(if (legacy) AccountMode.Register else AccountMode.Welcome) }
    // Ошибка из ядра приходит в новом снимке (`statusMessage`), а не в ответе на вызов: здесь
    // только отметка, что последняя попытка не удалась, — до следующего изменения ввода.
    var error by remember { mutableStateOf<Boolean?>(null) }
    var name by remember { mutableStateOf(state.profile.displayName) }
    var username by remember { mutableStateOf(state.profile.username) }
    var password by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    var recoveryKey by remember { mutableStateOf("") }
    var confirmDiscard by remember { mutableStateOf<(() -> Unit)?>(null) }

    fun submit(command: String) {
        error = null
        run(command) { ok -> if (!ok) error = true }
    }

    val shownError = if (error == true && !busy) state.statusMessage.ifBlank { "Не получилось. Попробуйте ещё раз." } else null

    Column(
        Modifier.fillMaxSize().background(colors.window).statusBarsPadding().imePadding()
            .verticalScroll(rememberScrollState()).padding(horizontal = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(36.dp))
        Image(
            painter = painterResource(R.drawable.turat_logo),
            contentDescription = "Логотип Turat",
            modifier = Modifier.size(96.dp),
        )
        Spacer(Modifier.height(14.dp))
        Text(
            when (mode) {
                AccountMode.Welcome -> "Turat"
                AccountMode.Register -> if (legacy) "Защитите аккаунт" else "Новый аккаунт"
                AccountMode.Login -> "Вход"
                AccountMode.Recover -> "Восстановление доступа"
            },
            color = colors.text,
            fontSize = 24.sp,
            fontWeight = FontWeight.Medium,
        )
        Spacer(Modifier.height(8.dp))
        Text(
            when (mode) {
                AccountMode.Welcome ->
                    "Мессенджер со сквозным шифрованием. Аккаунт не привязан ни к телефону, ни к почте — только username и пароль."
                AccountMode.Register -> if (legacy) {
                    "Придумайте пароль — с ним вы войдёте в Turat на любом устройстве, а переписка будет синхронизироваться сама. Ваши чаты останутся на месте."
                } else {
                    "Username — это и ваш адрес для собеседников, и логин. Пароль никуда не отправляется: Node хранит только зашифрованные данные."
                }
                AccountMode.Login -> "Введите username и пароль. История переписки загрузится с ваших устройств."
                AccountMode.Recover ->
                    "Введите ключ восстановления, который вы сохранили при регистрации, и придумайте новый пароль."
            },
            Modifier.padding(horizontal = 24.dp),
            color = colors.hint,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        state.account.notice?.takeIf { mode == AccountMode.Welcome || mode == AccountMode.Login }?.let { notice ->
            Spacer(Modifier.height(14.dp))
            Text(
                notice,
                Modifier.fillMaxWidth().padding(horizontal = 14.dp)
                    .glass(colors, GlassShape.Card).padding(14.dp),
                color = colors.text,
                fontSize = 14.sp,
                textAlign = TextAlign.Center,
            )
        }
        Spacer(Modifier.height(20.dp))

        when (mode) {
            AccountMode.Welcome -> {
                TelegramButton(
                    "Создать аккаунт",
                    { mode = AccountMode.Register },
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
                )
                Spacer(Modifier.height(10.dp))
                SecondaryButton("У меня уже есть аккаунт", Modifier.fillMaxWidth().padding(horizontal = 14.dp)) {
                    mode = AccountMode.Login
                }
            }
            AccountMode.Register -> {
                if (!legacy || state.profile.displayName.isBlank()) {
                    TelegramField(name, { name = it; error = null }, "Видимое имя")
                }
                TelegramField(username, { username = it.trim(); error = null }, "Username (логин)")
                TelegramField(password, { password = it; error = null }, "Пароль, не меньше $MinPasswordLength символов", password = true)
                TelegramField(repeat, { repeat = it; error = null }, "Пароль ещё раз", password = true, imeAction = ImeAction.Done)
                PasswordHint(password, repeat)
                Spacer(Modifier.height(16.dp))
                TelegramButton(
                    if (busy) "Создаём…" else "Создать аккаунт",
                    {
                        submit(
                            CoreJson.command(
                                "account_register",
                                "username" to username,
                                "display_name" to name,
                                "password" to password,
                                "device_name" to androidDeviceName(),
                            ),
                        )
                    },
                    enabled = !busy && username.length >= 3 && password.length >= MinPasswordLength &&
                        password == repeat && (legacy && state.profile.displayName.isNotBlank() || name.isNotBlank()),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
                )
            }
            AccountMode.Login -> {
                TelegramField(username, { username = it.trim(); error = null }, "Username")
                TelegramField(password, { password = it; error = null }, "Пароль", password = true, imeAction = ImeAction.Done)
                Spacer(Modifier.height(16.dp))
                TelegramButton(
                    if (busy) "Входим…" else "Войти",
                    {
                        val login = {
                            submit(
                                CoreJson.command(
                                    "account_login",
                                    "username" to username,
                                    "password" to password,
                                    "device_name" to androidDeviceName(),
                                    "discard_local" to legacy,
                                ),
                            )
                        }
                        if (legacy) confirmDiscard = login else login()
                    },
                    enabled = !busy && username.isNotBlank() && password.isNotEmpty(),
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
                )
                Spacer(Modifier.height(10.dp))
                LinkText("Забыли пароль? Восстановить по ключу") { mode = AccountMode.Recover; error = null }
            }
            AccountMode.Recover -> {
                TelegramField(recoveryKey, { recoveryKey = it; error = null }, "Ключ восстановления", singleLine = false)
                TelegramField(password, { password = it; error = null }, "Новый пароль", password = true)
                TelegramField(repeat, { repeat = it; error = null }, "Новый пароль ещё раз", password = true, imeAction = ImeAction.Done)
                PasswordHint(password, repeat)
                Spacer(Modifier.height(16.dp))
                TelegramButton(
                    if (busy) "Проверяем…" else "Восстановить",
                    {
                        val recover = {
                            submit(
                                CoreJson.command(
                                    "account_recover",
                                    "recovery_key" to recoveryKey,
                                    "new_password" to password,
                                    "device_name" to androidDeviceName(),
                                    "discard_local" to legacy,
                                ),
                            )
                        }
                        if (legacy) confirmDiscard = recover else recover()
                    },
                    enabled = !busy && recoveryKey.isNotBlank() && password.length >= MinPasswordLength && password == repeat,
                    modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
                )
            }
        }

        shownError?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, Modifier.padding(horizontal = 24.dp), color = colors.danger, fontSize = 14.sp, textAlign = TextAlign.Center)
        }

        Spacer(Modifier.height(18.dp))
        if (mode != AccountMode.Welcome && !legacy) {
            LinkText("Назад") { mode = AccountMode.Welcome; error = null }
        }
        if (legacy) {
            if (mode == AccountMode.Register) {
                LinkText("Войти в другой аккаунт") { mode = AccountMode.Login; error = null }
            } else {
                LinkText("Создать аккаунт для этой переписки") { mode = AccountMode.Register; error = null }
            }
            onLater?.let { later -> LinkText("Напомнить позже", later) }
        }
        if (mode != AccountMode.Welcome || !legacy) NodePicker(state, busy, run)
        Spacer(Modifier.height(24.dp).navigationBarsPadding())
    }

    confirmDiscard?.let { action ->
        AlertDialog(
            onDismissRequest = { confirmDiscard = null },
            title = { Text("Удалить переписку на этом устройстве?") },
            text = {
                Text("На этом устройстве есть чаты без аккаунта. При входе в другой аккаунт они будут удалены без возможности восстановления.")
            },
            confirmButton = {
                TextButton(onClick = { confirmDiscard = null; action() }) { Text("Удалить и войти", color = colors.danger) }
            },
            dismissButton = { TextButton(onClick = { confirmDiscard = null }) { Text("Отмена") } },
        )
    }
}

/** Свой Node выбирается до входа: аккаунт и username живут на нём. */
@Composable
private fun NodePicker(state: AppSnapshot, busy: Boolean, run: AccountRunner) {
    val colors = Telegram.colors
    var open by remember { mutableStateOf(false) }
    var address by remember { mutableStateOf(state.settings.bootstrapUrl) }
    val host = state.settings.bootstrapUrl.removePrefix("https://").removePrefix("http://").trimEnd('/')
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text("Node: $host", color = colors.hint, fontSize = 12.sp)
        Text(
            "Изменить",
            Modifier.clickable { open = !open; address = state.settings.bootstrapUrl }.padding(8.dp),
            color = colors.accent,
            fontSize = 12.sp,
        )
    }
    if (!open) return
    TelegramField(address, { address = it.trim() }, "Адрес Node", imeAction = ImeAction.Done)
    SecondaryButton(if (busy) "Подключаемся…" else "Подключиться", Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp)) {
        val url = if ("://" in address) address else "https://$address"
        run(CoreJson.command("connect", "bootstrap_url" to url)) { ok -> if (ok) open = false }
    }
}

@Composable
private fun PasswordHint(password: String, repeat: String) {
    val colors = Telegram.colors
    val hint = when {
        password.isNotEmpty() && password.length < MinPasswordLength -> "Пароль слишком короткий"
        repeat.isNotEmpty() && repeat != password -> "Пароли не совпадают"
        else -> null
    } ?: return
    Text(hint, Modifier.fillMaxWidth().padding(horizontal = 30.dp, vertical = 4.dp), color = colors.danger, fontSize = 12.sp)
}

@Composable
private fun LinkText(text: String, onClick: () -> Unit) {
    Text(
        text,
        Modifier.clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        color = Telegram.colors.accent,
        fontSize = 14.sp,
        fontWeight = FontWeight.Medium,
    )
}

/**
 * Ключ восстановления показывается один раз — сразу после регистрации или восстановления.
 * Закрыть экран можно, только подтвердив, что ключ сохранён.
 */
@Composable
fun RecoveryKeyScreen(key: String, busy: Boolean, run: AccountRunner) {
    val colors = Telegram.colors
    val clipboard = LocalClipboardManager.current
    var saved by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxSize().background(colors.window).statusBarsPadding()
            .verticalScroll(rememberScrollState()).padding(horizontal = 22.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(40.dp))
        Text("Ключ восстановления", color = colors.text, fontSize = 24.sp, fontWeight = FontWeight.Medium)
        Spacer(Modifier.height(10.dp))
        Text(
            "Это единственный способ вернуть доступ к аккаунту, если вы забудете пароль. Ни Turat, ни Node не знают ни ключа, ни пароля и восстановить их не смогут.",
            color = colors.hint,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(20.dp))
        Column(
            Modifier.fillMaxWidth().glass(colors, GlassShape.Panel, raised = true).padding(vertical = 18.dp, horizontal = 12.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            key.split('-').chunked(4).forEach { row ->
                Text(
                    row.joinToString("  "),
                    color = colors.text,
                    fontSize = 20.sp,
                    fontFamily = FontFamily.Monospace,
                    fontWeight = FontWeight.Medium,
                )
            }
        }
        Spacer(Modifier.height(12.dp))
        SecondaryButton(if (copied) "Скопировано" else "Скопировать ключ", Modifier.fillMaxWidth()) {
            clipboard.setText(AnnotatedString(key))
            copied = true
        }
        Spacer(Modifier.height(18.dp))
        Text(
            "Запишите ключ на бумаге или сохраните в менеджере паролей. Не храните его рядом с паролем и не отправляйте в чаты.",
            color = colors.hint,
            fontSize = 13.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(10.dp))
        Row(
            Modifier.fillMaxWidth().clickable { saved = !saved }.padding(vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Checkbox(
                saved,
                { saved = it },
                colors = CheckboxDefaults.colors(checkedColor = colors.accent, uncheckedColor = colors.hint),
            )
            Text("Я сохранил(а) ключ в надёжном месте", color = colors.text, fontSize = 14.sp)
        }
        Spacer(Modifier.height(10.dp))
        TelegramButton(
            "Готово",
            { run(CoreJson.command("account_confirm_recovery_key")) {} },
            enabled = saved && !busy,
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(24.dp).navigationBarsPadding())
    }
}

/** Username занят на текущем Node (например, после переезда на другой Node) — нужен другой. */
@Composable
fun UsernameConflictScreen(state: AppSnapshot, busy: Boolean, run: AccountRunner, onLater: () -> Unit) {
    val colors = Telegram.colors
    var username by remember { mutableStateOf("") }
    var error by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxSize().background(colors.window).statusBarsPadding().imePadding()
            .verticalScroll(rememberScrollState()).padding(horizontal = 8.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.height(48.dp))
        Text("Выберите новый username", color = colors.text, fontSize = 22.sp, fontWeight = FontWeight.Medium)
        Spacer(Modifier.height(10.dp))
        Text(
            "Username @${state.account.username} на этом Node уже занят другим человеком. Придумайте другой — по нему вас найдут собеседники, и с ним вы будете входить в аккаунт.",
            Modifier.padding(horizontal = 24.dp),
            color = colors.hint,
            fontSize = 14.sp,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.height(18.dp))
        TelegramField(username, { username = it.trim(); error = false }, "Новый username", imeAction = ImeAction.Done)
        Spacer(Modifier.height(16.dp))
        TelegramButton(
            if (busy) "Сохраняем…" else "Сохранить",
            {
                error = false
                run(
                    CoreJson.command(
                        "save_profile",
                        "username" to username,
                        "display_name" to state.profile.displayName,
                        "about" to state.profile.about,
                        "avatar_base64" to state.profile.avatarBase64,
                    ),
                ) { ok ->
                    if (ok) run(CoreJson.command("publish_profile")) { published -> error = !published } else error = true
                }
            },
            enabled = !busy && username.length >= 3,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp),
        )
        if (error && !busy && state.statusMessage.isNotBlank()) {
            Spacer(Modifier.height(10.dp))
            Text(state.statusMessage, Modifier.padding(horizontal = 24.dp), color = colors.danger, fontSize = 14.sp, textAlign = TextAlign.Center)
        }
        Spacer(Modifier.height(12.dp))
        LinkText("Позже", onLater)
    }
}

private val deviceDate = DateFormat.getDateInstance(DateFormat.MEDIUM)

/**
 * Раздел настроек «Аккаунт и устройства»: пароль, ключ восстановления, сеансы и выход.
 * Файлы больше не нужны: новое устройство входит по username и паролю.
 */
fun LazyListScope.accountSettings(
    state: AppSnapshot,
    busy: Boolean,
    run: AccountRunner,
    onSetUpAccount: () -> Unit,
) {
    val account = state.account
    if (!account.signedIn) {
        item { SectionTitle("Аккаунт") }
        item {
            SectionRow(
                R.drawable.ic_lock,
                "Создать аккаунт",
                "Пароль для входа на других устройствах и синхронизация переписки",
                onClick = onSetUpAccount,
            )
        }
        return
    }
    item { SectionTitle("Аккаунт") }
    item {
        SectionRow(R.drawable.ic_person, "@${account.username}", "логин · Node ${account.node.removePrefix("https://")}") {}
    }
    item { ChangePasswordBlock(busy, run) }
    item { NewRecoveryKeyBlock(busy, run) }

    item { SectionTitle("Устройства") }
    item {
        Text(
            "На всех устройствах — одна переписка. Новое устройство входит по username и паролю, завершённый сеанс перестаёт получать сообщения сразу.",
            Modifier.padding(horizontal = 18.dp, vertical = 2.dp),
            color = Telegram.colors.hint,
            fontSize = 13.sp,
        )
    }
    account.devices.forEach { device ->
        item(key = "device-${device.deviceId}") { DeviceRow(device, busy, run) }
    }
    item { RenameDeviceBlock(account.devices.firstOrNull { it.current }?.name.orEmpty(), busy, run) }
    item { LogoutRow(busy, run) }
}

@Composable
private fun DeviceRow(device: app.turattext.mobile.model.AccountDevice, busy: Boolean, run: AccountRunner) {
    val colors = Telegram.colors
    var confirm by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(device.name, color = colors.text, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            Text(
                if (device.current) "это устройство" else "вход ${deviceDate.format(Date(device.addedAt))}",
                color = if (device.current) colors.accent else colors.hint,
                fontSize = 13.sp,
            )
        }
        if (!device.current) {
            Text(
                "Завершить",
                Modifier.clickable(enabled = !busy) { confirm = true }.padding(8.dp),
                color = colors.danger,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
            )
        }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Завершить сеанс?") },
            text = { Text("«${device.name}» выйдет из аккаунта, переписка на нём будет удалена, и новые сообщения туда приходить перестанут.") },
            confirmButton = {
                TextButton(onClick = {
                    confirm = false
                    run(CoreJson.command("revoke_device", "device_id" to device.deviceId)) {}
                }) { Text("Завершить", color = colors.danger) }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Отмена") } },
        )
    }
}

@Composable
private fun ChangePasswordBlock(busy: Boolean, run: AccountRunner) {
    var open by remember { mutableStateOf(false) }
    var current by remember { mutableStateOf("") }
    var fresh by remember { mutableStateOf("") }
    var repeat by remember { mutableStateOf("") }
    SectionRow(R.drawable.ic_lock, "Сменить пароль", if (open) null else "нужен текущий пароль") { open = !open }
    if (!open) return
    Column {
        TelegramField(current, { current = it }, "Текущий пароль", password = true)
        TelegramField(fresh, { fresh = it }, "Новый пароль", password = true)
        TelegramField(repeat, { repeat = it }, "Новый пароль ещё раз", password = true, imeAction = ImeAction.Done)
        PasswordHint(fresh, repeat)
        TelegramButton(
            "Сменить пароль",
            {
                run(CoreJson.command("account_change_password", "old_password" to current, "new_password" to fresh)) { ok ->
                    if (ok) {
                        open = false
                        current = ""; fresh = ""; repeat = ""
                    }
                }
            },
            enabled = !busy && current.isNotEmpty() && fresh.length >= MinPasswordLength && fresh == repeat,
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
        )
    }
}

@Composable
private fun NewRecoveryKeyBlock(busy: Boolean, run: AccountRunner) {
    var open by remember { mutableStateOf(false) }
    var password by remember { mutableStateOf("") }
    SectionRow(R.drawable.ic_sync, "Новый ключ восстановления", if (open) null else "старый перестанет действовать") { open = !open }
    if (!open) return
    Column {
        TelegramField(password, { password = it }, "Пароль", password = true, imeAction = ImeAction.Done)
        TelegramButton(
            "Создать новый ключ",
            {
                run(CoreJson.command("account_new_recovery_key", "password" to password)) { ok ->
                    if (ok) {
                        open = false
                        password = ""
                    }
                }
            },
            enabled = !busy && password.isNotEmpty(),
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
        )
    }
}

@Composable
private fun RenameDeviceBlock(current: String, busy: Boolean, run: AccountRunner) {
    var name by remember(current) { mutableStateOf(current) }
    TelegramField(name, { name = it }, "Имя этого устройства", imeAction = ImeAction.Done)
    if (name.isNotBlank() && name != current) {
        TelegramButton(
            "Сохранить имя",
            { run(CoreJson.command("account_rename_device", "name" to name)) {} },
            enabled = !busy,
            modifier = Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
        )
    }
}

@Composable
private fun LogoutRow(busy: Boolean, run: AccountRunner) {
    val colors = Telegram.colors
    var confirm by remember { mutableStateOf(false) }
    SectionRow(R.drawable.ic_close, "Выйти из аккаунта", "переписка на этом устройстве будет удалена", danger = true) {
        if (!busy) confirm = true
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Выйти из аккаунта?") },
            text = {
                Text("Все чаты и файлы на этом устройстве будут удалены. На других ваших устройствах всё останется, а войти снова можно по username и паролю.")
            },
            confirmButton = {
                TextButton(onClick = {
                    confirm = false
                    run(CoreJson.command("account_logout")) {}
                }) { Text("Выйти", color = colors.danger) }
            },
            dismissButton = { TextButton(onClick = { confirm = false }) { Text("Отмена") } },
        )
    }
}
