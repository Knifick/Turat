package app.turattext.mobile.ui

import android.graphics.BitmapFactory
import android.util.Base64
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.model.Contact
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale
import kotlin.math.abs

/** Семь фирменных градиентов аватарок Telegram. */
private val AvatarGradients = listOf(
    Color(0xFFFF885E) to Color(0xFFFF516A),
    Color(0xFFFFCD6A) to Color(0xFFFFA85C),
    Color(0xFFE0A2F3) to Color(0xFFD669ED),
    Color(0xFFA0DE7E) to Color(0xFF54CB68),
    Color(0xFF53EDD6) to Color(0xFF28C9B7),
    Color(0xFF72D5FD) to Color(0xFF2A9EF1),
    Color(0xFFB694F9) to Color(0xFF6C61DF),
)

fun avatarBrush(key: String): Brush {
    val index = (abs(key.hashCode().toLong()) % AvatarGradients.size).toInt()
    val (top, bottom) = AvatarGradients[index]
    return Brush.verticalGradient(listOf(top, bottom))
}

/** Одна или две буквы, как в аватарках Telegram. */
fun initialsOf(name: String): String {
    val words = name.trim().split(' ', ' ').filter { it.isNotBlank() }
    return when (words.size) {
        0 -> "#"
        1 -> words[0].take(1).uppercase()
        else -> (words.first().take(1) + words.last().take(1)).uppercase()
    }
}

@Composable
fun Avatar(
    name: String,
    key: String,
    avatarBase64: String?,
    size: Dp,
    modifier: Modifier = Modifier,
    online: Boolean = false,
    onlineRing: Color = Color.Transparent,
) {
    val bitmap = remember(avatarBase64) {
        avatarBase64?.let {
            runCatching {
                val bytes = Base64.decode(it, Base64.DEFAULT)
                BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
            }.getOrNull()
        }
    }
    Box(modifier.size(size)) {
        Box(
            Modifier.fillMaxSize().clip(CircleShape).background(avatarBrush(key)),
            contentAlignment = Alignment.Center,
        ) {
            if (bitmap != null) {
                Image(bitmap, name, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
            } else {
                Text(
                    initialsOf(name),
                    color = Color.White,
                    fontSize = (size.value * 0.36f).sp,
                    fontWeight = FontWeight.Medium,
                )
            }
        }
        if (online) {
            Box(
                Modifier.align(Alignment.BottomEnd)
                    .size(size * 0.28f)
                    .border(size * 0.05f, onlineRing, CircleShape)
                    .padding(size * 0.05f)
                    .clip(CircleShape)
                    .background(Telegram.colors.online),
            )
        }
    }
}

/** Галочки доставки: одна — отправлено, две — доставлено, синие — прочитано. */
@Composable
fun DeliveryTicks(delivered: Boolean, read: Boolean, tint: Color, size: Dp = 17.dp) {
    Icon(
        painter = painterResource(if (delivered || read) R.drawable.ic_check_double else R.drawable.ic_check),
        contentDescription = null,
        tint = tint,
        modifier = Modifier.size(size),
    )
}

/** Счётчик непрочитанных — синяя пилюля справа в списке чатов. */
@Composable
fun UnreadBadge(count: Int, background: Color = Telegram.colors.badge) {
    Box(
        Modifier.height(20.dp)
            .defaultMinSize(minWidth = 20.dp)
            .clip(RoundedCornerShape(10.dp))
            .background(background)
            .padding(horizontal = 6.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            if (count > 999) "999+" else count.toString(),
            color = Telegram.colors.badgeText,
            fontSize = 12.sp,
            fontWeight = FontWeight.Medium,
        )
    }
}

/** Серая «служебная» пилюля по центру ленты: дата или системная отметка. */
@Composable
fun ServicePill(text: String, modifier: Modifier = Modifier) {
    Row(
        modifier
            .clip(RoundedCornerShape(14.dp))
            .background(Telegram.colors.servicePill)
            .padding(horizontal = 11.dp, vertical = 5.dp),
    ) {
        Text(text, color = Telegram.colors.serviceText, fontSize = 13.sp, fontWeight = FontWeight.Medium)
    }
}

@Composable
fun BoxScope.CenteredHint(text: String) {
    ServicePill(text, Modifier.align(Alignment.Center))
}

private val timeFormat = SimpleDateFormat("HH:mm", Locale.getDefault())

private val months = listOf(
    "января", "февраля", "марта", "апреля", "мая", "июня",
    "июля", "августа", "сентября", "октября", "ноября", "декабря",
)

private val weekdays = listOf("вс", "пн", "вт", "ср", "чт", "пт", "сб")

fun timeOf(value: Long): String = timeFormat.format(Date(value))

private fun calendarOf(value: Long) = Calendar.getInstance().apply { timeInMillis = value }

fun isSameDay(left: Long, right: Long): Boolean {
    val a = calendarOf(left)
    val b = calendarOf(right)
    return a.get(Calendar.YEAR) == b.get(Calendar.YEAR) &&
        a.get(Calendar.DAY_OF_YEAR) == b.get(Calendar.DAY_OF_YEAR)
}

private fun daysBetweenToday(value: Long): Int {
    val day = calendarOf(value).apply {
        set(Calendar.HOUR_OF_DAY, 0); set(Calendar.MINUTE, 0)
        set(Calendar.SECOND, 0); set(Calendar.MILLISECOND, 0)
    }
    val today = Calendar.getInstance().apply {
        set(Calendar.HOUR_OF_DAY, 0); set(Calendar.MINUTE, 0)
        set(Calendar.SECOND, 0); set(Calendar.MILLISECOND, 0)
    }
    return ((today.timeInMillis - day.timeInMillis) / 86_400_000L).toInt()
}

/** Время в списке чатов: сегодня — часы, неделя — день недели, дальше — дата. */
fun chatListTime(value: Long): String {
    if (value <= 0) return ""
    val days = daysBetweenToday(value)
    val calendar = calendarOf(value)
    return when {
        days <= 0 -> timeOf(value)
        days < 7 -> weekdays[calendar.get(Calendar.DAY_OF_WEEK) - 1]
        else -> SimpleDateFormat(
            if (days > 330) "dd.MM.yy" else "dd.MM",
            Locale.getDefault(),
        ).format(Date(value))
    }
}

/** Заголовок-разделитель дня в ленте сообщений. */
fun dateSeparator(value: Long): String {
    val days = daysBetweenToday(value)
    val calendar = calendarOf(value)
    return when {
        days == 0 -> "Сегодня"
        days == 1 -> "Вчера"
        days < 330 -> "${calendar.get(Calendar.DAY_OF_MONTH)} ${months[calendar.get(Calendar.MONTH)]}"
        else -> "${calendar.get(Calendar.DAY_OF_MONTH)} ${months[calendar.get(Calendar.MONTH)]} ${calendar.get(Calendar.YEAR)}"
    }
}

/** Подпись под именем собеседника в шапке диалога. */
fun presenceOf(contact: Contact): String {
    if (contact.pending) return "запрос на общение"
    val seen = contact.lastSeen ?: return "был(а) недавно"
    val delta = System.currentTimeMillis() - seen
    return when {
        delta < 90_000 -> "в сети"
        daysBetweenToday(seen) == 0 -> "был(а) в ${timeOf(seen)}"
        daysBetweenToday(seen) == 1 -> "был(а) вчера в ${timeOf(seen)}"
        else -> "был(а) ${dateSeparator(seen)}"
    }
}

fun isOnline(contact: Contact): Boolean {
    val seen = contact.lastSeen ?: return false
    return System.currentTimeMillis() - seen < 90_000
}

fun shortId(value: String): String =
    if (value.length <= 20) value else value.take(11) + "…" + value.takeLast(6)

fun formatBytes(value: Long): String = when {
    value < 1024 -> "$value Б"
    value < 1024 * 1024 -> "${value / 1024} КБ"
    else -> String.format(Locale.getDefault(), "%.1f МБ", value / 1_048_576.0)
}
