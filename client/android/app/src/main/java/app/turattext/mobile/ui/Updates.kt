package app.turattext.mobile.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.update.ReleaseInfo
import app.turattext.mobile.update.UpdateState

/** Тихая строка над списком чатов: новая версия есть, но ничего не требует. */
@Composable
fun UpdateBanner(release: ReleaseInfo, onOpen: () -> Unit, onDismiss: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().padding(start = 12.dp, end = 12.dp, top = 8.dp)
            .clip(GlassShape.Card)
            .glass(colors, GlassShape.Card)
            .clickable(onClick = onOpen)
            .padding(start = 14.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_download), null, Modifier.size(20.dp), colors.accent)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                "Доступна версия ${release.version}",
                color = colors.text,
                fontSize = 14.sp,
                fontWeight = FontWeight.Medium,
            )
            Text("Что нового и обновление", color = colors.hint, fontSize = 12.sp)
        }
        IconButton(onDismiss, Modifier.size(40.dp)) {
            Icon(painterResource(R.drawable.ic_close), "Скрыть до следующего запуска", Modifier.size(16.dp), colors.hint)
        }
    }
}

/** Экран обновления: ченджлог релиза и три исхода — обновиться, отложить, пропустить версию. */
@Composable
fun UpdateScreen(
    update: UpdateState,
    release: ReleaseInfo,
    onInstall: () -> Unit,
    onCancel: () -> Unit,
    onLater: () -> Unit,
    onSkip: () -> Unit,
    onBack: () -> Unit,
) {
    val colors = Telegram.colors
    val uriHandler = LocalUriHandler.current
    TelegramScreen("Обновление", onBack) {
        item {
            Column(
                Modifier.fillMaxWidth().padding(14.dp)
                    .glass(colors, GlassShape.Panel, raised = true)
                    .padding(18.dp),
            ) {
                Text("Turat ${release.version}", color = colors.text, fontSize = 22.sp, fontWeight = FontWeight.Medium)
                Spacer(Modifier.height(4.dp))
                Text(
                    "Сейчас установлена ${update.currentVersion} · ${formatBytes(release.size)}",
                    color = colors.hint,
                    fontSize = 13.sp,
                )
                Text(release.title, color = colors.hint, fontSize = 13.sp)
            }
        }
        item { SectionTitle("Что нового") }
        item {
            Text(
                releaseNotes(release.notes),
                Modifier.fillMaxWidth().padding(horizontal = 18.dp, vertical = 4.dp),
                color = colors.text,
                fontSize = 14.sp,
                lineHeight = 20.sp,
            )
        }
        item {
            Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 16.dp)) {
                update.downloading?.let { fraction ->
                    LinearProgressIndicator(
                        progress = { fraction },
                        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
                        color = colors.accent,
                        trackColor = colors.field,
                    )
                    Spacer(Modifier.height(8.dp))
                    Text(
                        "Загрузка… ${formatBytes((fraction * release.size).toLong())} из ${formatBytes(release.size)}",
                        Modifier.padding(horizontal = 4.dp),
                        color = colors.hint,
                        fontSize = 13.sp,
                    )
                    Spacer(Modifier.height(12.dp))
                    SecondaryButton("Отменить загрузку", Modifier.fillMaxWidth(), onCancel)
                } ?: run {
                    update.message?.let {
                        Text(it, Modifier.padding(horizontal = 4.dp), color = colors.hint, fontSize = 13.sp)
                        Spacer(Modifier.height(12.dp))
                    }
                    TelegramButton("Обновить", onInstall, modifier = Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        SecondaryButton("Позже", Modifier.weight(1f), onLater)
                        SecondaryButton("Пропустить версию", Modifier.weight(1f), onSkip)
                    }
                }
            }
        }
        if (release.pageUrl.isNotBlank()) {
            item {
                SectionRow(null, "Открыть релиз на GitHub", release.tag) { uriHandler.openUri(release.pageUrl) }
            }
        }
    }
}

/**
 * Ченджлог релиза пишется в Markdown. Полноценный рендерер ради одного экрана не нужен: заголовки
 * становятся жирными строками, списки — пунктами, таблицы — парами «ячейка — ячейка».
 */
private fun releaseNotes(markdown: String): AnnotatedString = buildAnnotatedString {
    val heading = Regex("""^#{1,6}\s+(.*)$""")
    val bullet = Regex("""^(?:[-*+]|\d+\.)\s+(.*)$""")
    val separator = Regex("""^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?$""")
    val link = Regex("""!?\[([^\]]*)\]\([^)]*\)""")
    fun inline(text: String) = link.replace(text, "$1").replace("**", "").replace("__", "").replace("`", "")

    var pendingBreak = false
    var previousBlank = true
    for (raw in markdown.replace("\r", "").lines()) {
        val line = raw.trim()
        if (line.isEmpty()) {
            if (!previousBlank) pendingBreak = true
            previousBlank = true
            continue
        }
        if (separator.matches(line)) continue
        if (length > 0) append(if (pendingBreak) "\n\n" else "\n")
        pendingBreak = false
        previousBlank = false

        val title = heading.find(line)
        val item = bullet.find(line)
        when {
            title != null -> withStyle(SpanStyle(fontWeight = FontWeight.SemiBold, fontSize = 15.sp)) {
                append(inline(title.groupValues[1]))
            }
            line.startsWith("|") -> append(
                "• " + line.trim('|').split('|').map { inline(it.trim()) }.filter { it.isNotEmpty() }.joinToString(" — "),
            )
            item != null -> append("• " + inline(item.groupValues[1]))
            else -> append(inline(line))
        }
    }
    if (length == 0) append("Автор не приложил описание изменений.")
}
