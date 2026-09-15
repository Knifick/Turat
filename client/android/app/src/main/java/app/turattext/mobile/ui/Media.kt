package app.turattext.mobile.ui

import android.graphics.BitmapFactory
import android.net.Uri
import android.view.SurfaceView
import androidx.annotation.OptIn
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.BaseDataSource
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.ProgressiveMediaSource
import app.turattext.mobile.R
import app.turattext.mobile.core.NativeCore
import app.turattext.mobile.model.Attachment
import app.turattext.mobile.model.MediaKind
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.io.Closeable
import java.io.IOException
import java.io.InputStream
import java.util.Locale

/**
 * Просмотр вложений.
 *
 * Вложение лежит на диске зашифрованным чанками, поэтому и картинка, и видео читаются через
 * [EncryptedMedia] — тонкую обёртку над потоковым читателем Rust. Ни один кадр не проходит
 * через временный расшифрованный файл: плеер запрашивает у ядра ровно тот отрезок, который
 * ему нужен сейчас, и перемотка в середину часового ролика не читает всё, что было до неё.
 */

/** Открытое вложение: читается по произвольному смещению, расшифровывая только нужные чанки. */
class EncryptedMedia(path: String) : Closeable {
    private var handle: Long = NativeCore.openMedia(path)
    val length: Long = if (handle == 0L) -1 else NativeCore.mediaLength(handle)
    val opened: Boolean get() = handle != 0L && length >= 0

    fun read(offset: Long, buffer: ByteArray, count: Int): Int {
        val current = handle
        return if (current == 0L) -1 else NativeCore.readMedia(current, offset, buffer, count)
    }

    /** Последовательный поток поверх того же читателя — для декодера картинок. */
    fun stream(): InputStream = object : InputStream() {
        private var position = 0L
        private val single = ByteArray(1)

        override fun read(): Int = if (read(single, 0, 1) <= 0) -1 else single[0].toInt() and 0xff

        override fun read(destination: ByteArray, offset: Int, count: Int): Int {
            if (count == 0) return 0
            val scratch = if (offset == 0) destination else ByteArray(count)
            val read = this@EncryptedMedia.read(position, scratch, count)
            if (read <= 0) return -1
            if (scratch !== destination) System.arraycopy(scratch, 0, destination, offset, read)
            position += read
            return read
        }

        override fun skip(count: Long): Long {
            val step = count.coerceAtMost(length - position).coerceAtLeast(0)
            position += step
            return step
        }

        override fun available(): Int = (length - position).coerceIn(0, Int.MAX_VALUE.toLong()).toInt()
    }

    override fun close() {
        val current = handle
        handle = 0
        NativeCore.closeMedia(current)
    }
}

/**
 * Источник данных ExoPlayer поверх зашифрованного вложения.
 *
 * Плеер сам решает, что и когда читать, поэтому «потоковая подгрузка» здесь не эмуляция:
 * `open` отдаёт длину и позицию, `read` расшифровывает очередной кусок, и ни в какой момент
 * весь файл в памяти не оказывается.
 */
@OptIn(markerClass = [UnstableApi::class])
private class EncryptedMediaDataSource(private val path: String) : BaseDataSource(false) {
    private var media: EncryptedMedia? = null
    private var address: Uri? = null
    private var position = 0L
    private var remaining = 0L
    private var scratch = ByteArray(0)

    override fun open(dataSpec: DataSpec): Long {
        transferInitializing(dataSpec)
        val opened = EncryptedMedia(path)
        if (!opened.opened) {
            opened.close()
            throw IOException("Вложение недоступно")
        }
        media = opened
        address = dataSpec.uri
        position = dataSpec.position
        remaining = if (dataSpec.length == C.LENGTH_UNSET.toLong()) {
            opened.length - dataSpec.position
        } else {
            dataSpec.length
        }
        if (remaining < 0) throw IOException("Некорректная позиция во вложении")
        transferStarted(dataSpec)
        return remaining
    }

    override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
        if (length == 0) return 0
        if (remaining == 0L) return C.RESULT_END_OF_INPUT
        val wanted = minOf(length.toLong(), remaining).toInt()
        // Rust пишет с нулевого индекса, поэтому чужой буфер с ненулевым смещением
        // наполняется через свой, переиспользуемый.
        if (offset == 0) {
            val read = media?.read(position, buffer, wanted) ?: -1
            if (read < 0) throw IOException("Не удалось прочитать вложение")
            if (read == 0) return C.RESULT_END_OF_INPUT
            position += read
            remaining -= read
            bytesTransferred(read)
            return read
        }
        if (scratch.size < wanted) scratch = ByteArray(wanted)
        val read = media?.read(position, scratch, wanted) ?: -1
        if (read < 0) throw IOException("Не удалось прочитать вложение")
        if (read == 0) return C.RESULT_END_OF_INPUT
        System.arraycopy(scratch, 0, buffer, offset, read)
        position += read
        remaining -= read
        bytesTransferred(read)
        return read
    }

    override fun getUri(): Uri? = address

    override fun close() {
        media?.close()
        media = null
        if (address != null) {
            address = null
            transferEnded()
        }
    }
}

/** Кадр видео и картинка декодируются в фоне и с уменьшением: пузырю не нужен оригинал 4000×3000. */
private suspend fun decodeAttachment(path: String, maximumEdge: Int): ImageBitmap? =
    withContext(Dispatchers.IO) {
        runCatching {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            EncryptedMedia(path).use { media ->
                if (!media.opened) return@runCatching null
                media.stream().use { BitmapFactory.decodeStream(it, null, bounds) }
            }
            val longest = maxOf(bounds.outWidth, bounds.outHeight)
            if (longest <= 0) return@runCatching null
            var sample = 1
            while (longest / sample > maximumEdge) sample *= 2
            val options = BitmapFactory.Options().apply { inSampleSize = sample }
            EncryptedMedia(path).use { media ->
                if (!media.opened) return@runCatching null
                media.stream().use { BitmapFactory.decodeStream(it, null, options) }
            }?.asImageBitmap()
        }.getOrNull()
    }

/** Мгновенная миниатюра из сообщения: она приходит вместе с ним и не требует расшифровки. */
private fun decodeThumbnail(encoded: String?): ImageBitmap? = encoded?.let { value ->
    runCatching {
        val bytes = android.util.Base64.decode(value, android.util.Base64.DEFAULT)
        BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap()
    }.getOrNull()
}

/** Ход передачи одного вложения, каким его показывает пузырь. */
data class MediaTransfer(
    val jobId: String,
    val done: Long,
    val total: Long,
    val failed: Boolean = false,
    val error: String = "",
) {
    val fraction: Float get() = if (total <= 0) 0f else (done.toFloat() / total).coerceIn(0f, 1f)
}

fun formatDuration(milliseconds: Long): String {
    val totalSeconds = (milliseconds / 1000).coerceAtLeast(0)
    val minutes = totalSeconds / 60
    val seconds = totalSeconds % 60
    return if (minutes >= 60) {
        String.format(Locale.ROOT, "%d:%02d:%02d", minutes / 60, minutes % 60, seconds)
    } else {
        String.format(Locale.ROOT, "%d:%02d", minutes, seconds)
    }
}

/**
 * Кольцо прогресса поверх превью: видно, сколько уже готово, и одним касанием передача
 * отменяется. Без него отправка большого видео выглядела бы зависанием.
 */
@Composable
fun TransferRing(
    fraction: Float,
    modifier: Modifier = Modifier,
    icon: Int = R.drawable.ic_close,
    indeterminate: Boolean = false,
    onClick: (() -> Unit)? = null,
) {
    val animated by animateFloatAsState(fraction, label = "transfer")
    Box(
        modifier
            .size(52.dp)
            .clip(CircleShape)
            .background(Color.Black.copy(alpha = 0.42f))
            .border(1.dp, Color.White.copy(alpha = 0.22f), CircleShape)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.size(44.dp)) {
            val stroke = 3.dp.toPx()
            val inset = stroke / 2
            drawArc(
                color = Color.White.copy(alpha = 0.28f),
                startAngle = 0f,
                sweepAngle = 360f,
                useCenter = false,
                topLeft = Offset(inset, inset),
                size = Size(size.width - stroke, size.height - stroke),
                style = Stroke(width = stroke),
            )
            if (!indeterminate) {
                drawArc(
                    color = Color.White,
                    startAngle = -90f,
                    sweepAngle = 360f * animated,
                    useCenter = false,
                    topLeft = Offset(inset, inset),
                    size = Size(size.width - stroke, size.height - stroke),
                    style = Stroke(width = stroke, cap = androidx.compose.ui.graphics.StrokeCap.Round),
                )
            }
        }
        Icon(painterResource(icon), null, Modifier.size(18.dp), Color.White)
    }
}

/**
 * Вложение внутри пузыря: картинка и видео показываются превью с плеером, всё остальное —
 * привычной карточкой с именем и размером.
 */
@Composable
fun AttachmentBlock(
    attachment: Attachment,
    outgoing: Boolean,
    transfer: MediaTransfer?,
    onOpen: () -> Unit,
    onSave: () -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    val visual = attachment.kind == MediaKind.Image || attachment.kind == MediaKind.Video
    if (visual) {
        MediaThumbnail(attachment, transfer, onOpen, onCancel, modifier)
    } else {
        FileCard(attachment, outgoing, transfer, onSave, onCancel, modifier)
    }
    if (transfer?.failed == true) {
        Text(
            transfer.error.ifBlank { "Не удалось передать файл" },
            color = colors.danger,
            fontSize = 12.sp,
            modifier = Modifier.padding(top = 4.dp),
        )
    }
}

@Composable
private fun MediaThumbnail(
    attachment: Attachment,
    transfer: MediaTransfer?,
    onOpen: () -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    val shape = RoundedCornerShape(14.dp)
    // Пока оригинал расшифровывается, место занимает миниатюра из самого сообщения.
    val placeholder = remember(attachment.id) { decodeThumbnail(attachment.thumbnailBase64) }
    var preview by remember(attachment.id) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(attachment.id, transfer?.jobId) {
        // Пока файл ещё шифруется, читать его бессмысленно; у видео превью — присланный кадр.
        if (attachment.kind != MediaKind.Image) return@LaunchedEffect
        if (transfer != null && !transfer.failed) return@LaunchedEffect
        preview = decodeAttachment(attachment.localPath, maximumEdge = 1024)
    }
    Box(
        modifier
            .widthIn(max = 320.dp)
            .fillMaxWidth()
            .aspectRatio(attachment.aspectRatio)
            .clip(shape)
            .background(colors.field)
            .clickable(enabled = transfer == null, onClick = onOpen),
        contentAlignment = Alignment.Center,
    ) {
        val shown = preview ?: placeholder
        if (shown != null) {
            androidx.compose.foundation.Image(
                bitmap = shown,
                contentDescription = attachment.fileName,
                modifier = Modifier.fillMaxSize(),
                contentScale = ContentScale.Crop,
            )
        }
        if (attachment.kind == MediaKind.Image && preview == null && placeholder != null) {
            // Миниатюра мелкая: лёгкая вуаль прячет её зернистость до появления оригинала.
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.18f)))
        }
        when {
            transfer != null && !transfer.failed -> TransferRing(
                fraction = transfer.fraction,
                onClick = onCancel,
            )
            attachment.kind == MediaKind.Video -> Box(
                Modifier.size(54.dp).clip(CircleShape)
                    .background(Color.Black.copy(alpha = 0.40f))
                    .border(1.dp, Color.White.copy(alpha = 0.25f), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(R.drawable.ic_play), "Воспроизвести", Modifier.size(24.dp), Color.White)
            }
            preview == null && placeholder == null ->
                TransferRing(fraction = 0f, indeterminate = true, icon = R.drawable.ic_download)
        }
        // Длительность ролика и вес — как в Telegram, тёмной плашкой в углу.
        val badge = when {
            transfer != null && !transfer.failed ->
                "${formatBytes(transfer.done)} из ${formatBytes(transfer.total)}"
            attachment.kind == MediaKind.Video && attachment.durationMilliseconds > 0 ->
                formatDuration(attachment.durationMilliseconds)
            else -> formatBytes(attachment.size)
        }
        Text(
            badge,
            color = Color.White,
            fontSize = 11.sp,
            fontWeight = FontWeight.Medium,
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(6.dp)
                .clip(RoundedCornerShape(7.dp))
                .background(Color.Black.copy(alpha = 0.42f))
                .padding(horizontal = 6.dp, vertical = 2.dp),
        )
    }
}

@Composable
private fun FileCard(
    attachment: Attachment,
    outgoing: Boolean,
    transfer: MediaTransfer?,
    onSave: () -> Unit,
    onCancel: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = Telegram.colors
    val textColor = if (outgoing) colors.bubbleOutText else colors.bubbleInText
    val metaColor = if (outgoing) colors.bubbleOutMeta else colors.bubbleInMeta
    val tint = if (outgoing) colors.bubbleOutText else colors.accent
    Row(
        modifier.padding(vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(42.dp).clip(CircleShape)
                .background(
                    if (outgoing) colors.bubbleOutText.copy(alpha = .20f)
                    else colors.accent.copy(alpha = .18f),
                )
                .clickable { if (transfer == null) onSave() else onCancel() },
            contentAlignment = Alignment.Center,
        ) {
            if (transfer != null && !transfer.failed) {
                val animated by animateFloatAsState(transfer.fraction, label = "file")
                Canvas(Modifier.size(38.dp)) {
                    val stroke = 2.5f.dp.toPx()
                    drawArc(
                        color = tint,
                        startAngle = -90f,
                        sweepAngle = 360f * animated,
                        useCenter = false,
                        topLeft = Offset(stroke / 2, stroke / 2),
                        size = Size(size.width - stroke, size.height - stroke),
                        style = Stroke(width = stroke, cap = androidx.compose.ui.graphics.StrokeCap.Round),
                    )
                }
                Icon(painterResource(R.drawable.ic_close), "Отменить", Modifier.size(16.dp), tint)
            } else {
                Icon(
                    painterResource(if (attachment.kind == MediaKind.Audio) R.drawable.ic_play else R.drawable.ic_file),
                    "Сохранить файл",
                    Modifier.size(19.dp),
                    tint,
                )
            }
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
            Text(
                if (transfer != null && !transfer.failed) {
                    "${formatBytes(transfer.done)} из ${formatBytes(transfer.total)}"
                } else {
                    formatBytes(attachment.size)
                },
                color = metaColor,
                fontSize = 12.sp,
            )
        }
    }
}

/**
 * Полноэкранный просмотр: картинка с масштабированием щипком, видео — с плеером,
 * который читает файл потоково.
 */
@Composable
fun MediaViewer(attachment: Attachment, onClose: () -> Unit, onSave: () -> Unit) {
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.94f))) {
        if (attachment.kind == MediaKind.Video || attachment.kind == MediaKind.Audio) {
            VideoSurface(attachment, Modifier.align(Alignment.Center))
        } else {
            ZoomableImage(attachment, onClose)
        }
        Row(
            Modifier.align(Alignment.TopCenter).statusBarsPadding().fillMaxWidth()
                .padding(horizontal = 6.dp, vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClose, Modifier.size(44.dp)) {
                Icon(painterResource(R.drawable.ic_close), "Закрыть", Modifier.size(22.dp), Color.White)
            }
            Column(Modifier.weight(1f).padding(horizontal = 4.dp)) {
                Text(
                    attachment.fileName,
                    color = Color.White,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(formatBytes(attachment.size), color = Color.White.copy(alpha = 0.65f), fontSize = 12.sp)
            }
            IconButton(onSave, Modifier.size(44.dp)) {
                Icon(painterResource(R.drawable.ic_download), "Сохранить", Modifier.size(22.dp), Color.White)
            }
        }
    }
}

@Composable
private fun BoxScope.ZoomableImage(attachment: Attachment, onClose: () -> Unit) {
    var image by remember(attachment.id) { mutableStateOf<ImageBitmap?>(null) }
    var scale by remember(attachment.id) { mutableStateOf(1f) }
    var offset by remember(attachment.id) { mutableStateOf(Offset.Zero) }
    LaunchedEffect(attachment.id) {
        image = decodeAttachment(attachment.localPath, maximumEdge = 2560)
    }
    val current = image
    if (current == null) {
        TransferRing(fraction = 0f, indeterminate = true, icon = R.drawable.ic_download, modifier = Modifier.align(Alignment.Center))
        return
    }
    androidx.compose.foundation.Image(
        bitmap = current,
        contentDescription = attachment.fileName,
        contentScale = ContentScale.Fit,
        modifier = Modifier
            .fillMaxSize()
            .pointerInput(attachment.id) {
                detectTransformGestures { _, pan, zoom, _ ->
                    scale = (scale * zoom).coerceIn(1f, 6f)
                    offset = if (scale <= 1f) Offset.Zero else offset + pan
                }
            }
            .pointerInput(attachment.id) {
                detectTapGestures(
                    onDoubleTap = {
                        scale = if (scale > 1f) 1f else 2.5f
                        offset = Offset.Zero
                    },
                    onTap = { if (scale <= 1f) onClose() },
                )
            }
            .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
    )
}

/**
 * Плеер поверх зашифрованного вложения. Управление рисует Compose, чтобы плеер не выбивался
 * из стеклянного оформления остального приложения.
 */
@OptIn(markerClass = [UnstableApi::class])
@Composable
private fun BoxScope.VideoSurface(attachment: Attachment, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val path = attachment.localPath
    val player = remember(path) {
        ExoPlayer.Builder(context).build().apply {
            val factory = DataSource.Factory { EncryptedMediaDataSource(path) }
            setMediaSource(
                ProgressiveMediaSource.Factory(factory)
                    .createMediaSource(MediaItem.fromUri(Uri.parse("turat://${attachment.id}"))),
            )
            prepare()
            playWhenReady = true
        }
    }
    var playing by remember(path) { mutableStateOf(true) }
    var buffering by remember(path) { mutableStateOf(true) }
    var position by remember(path) { mutableStateOf(0L) }
    var duration by remember(path) { mutableStateOf(attachment.durationMilliseconds) }
    var failure by remember(path) { mutableStateOf<String?>(null) }

    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onIsPlayingChanged(value: Boolean) {
                playing = value
            }

            override fun onPlaybackStateChanged(state: Int) {
                buffering = state == Player.STATE_BUFFERING
                if (state == Player.STATE_READY && player.duration > 0) duration = player.duration
            }

            override fun onPlayerError(error: androidx.media3.common.PlaybackException) {
                failure = "Не удалось воспроизвести файл"
            }
        }
        player.addListener(listener)
        onDispose {
            player.removeListener(listener)
            player.release()
        }
    }
    LaunchedEffect(player) {
        while (true) {
            position = player.currentPosition
            delay(200)
        }
    }

    Box(modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        AndroidView(
            factory = { SurfaceView(it).also(player::setVideoSurfaceView) },
            modifier = Modifier
                .fillMaxWidth()
                .aspectRatio(if (attachment.kind == MediaKind.Audio) 1.6f else attachment.aspectRatio),
        )
        AnimatedVisibility(buffering && failure == null, enter = fadeIn(), exit = fadeOut()) {
            TransferRing(fraction = 0f, indeterminate = true, icon = R.drawable.ic_download)
        }
        failure?.let {
            Text(it, color = Color.White, fontSize = 15.sp, textAlign = TextAlign.Center)
        }
    }
    PlayerControls(
        playing = playing,
        position = position,
        duration = duration,
        onToggle = { if (player.isPlaying) player.pause() else player.play() },
        onSeek = { fraction -> if (duration > 0) player.seekTo((duration * fraction).toLong()) },
    )
}

@Composable
private fun BoxScope.PlayerControls(
    playing: Boolean,
    position: Long,
    duration: Long,
    onToggle: () -> Unit,
    onSeek: (Float) -> Unit,
) {
    val colors = Telegram.colors
    Row(
        Modifier
            .align(Alignment.BottomCenter)
            .navigationBarsPadding()
            .padding(horizontal = 12.dp, vertical = 12.dp)
            .fillMaxWidth()
            .clip(RoundedCornerShape(22.dp))
            .background(
                Brush.verticalGradient(
                    listOf(Color.White.copy(alpha = 0.14f), Color.White.copy(alpha = 0.07f)),
                ),
            )
            .border(1.dp, Color.White.copy(alpha = 0.18f), RoundedCornerShape(22.dp))
            .padding(horizontal = 8.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        IconButton(onToggle, Modifier.size(40.dp)) {
            Icon(
                painterResource(if (playing) R.drawable.ic_pause else R.drawable.ic_play),
                if (playing) "Пауза" else "Воспроизвести",
                Modifier.size(20.dp),
                Color.White,
            )
        }
        Text(formatDuration(position), color = Color.White, fontSize = 12.sp)
        // Полоса перемотки: касание в любой точке переносит воспроизведение туда же.
        Box(
            Modifier
                .weight(1f)
                .height(24.dp)
                .pointerInput(duration) {
                    detectTapGestures { point -> onSeek(point.x / size.width.toFloat()) }
                },
            contentAlignment = Alignment.CenterStart,
        ) {
            Box(
                Modifier.fillMaxWidth().height(3.dp).clip(CircleShape)
                    .background(Color.White.copy(alpha = 0.28f)),
            )
            val played = if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
            Box(
                Modifier.fillMaxWidth(played).height(3.dp).clip(CircleShape).background(colors.accent),
            )
        }
        Text(formatDuration(duration), color = Color.White.copy(alpha = 0.7f), fontSize = 12.sp)
    }
}
