package app.turattext.mobile.media

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.net.Uri
import androidx.annotation.OptIn
import androidx.exifinterface.media.ExifInterface
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.effect.Presentation
import androidx.media3.transformer.Composition
import androidx.media3.transformer.DefaultEncoderFactory
import androidx.media3.transformer.EditedMediaItem
import androidx.media3.transformer.Effects
import androidx.media3.transformer.ExportException
import androidx.media3.transformer.ExportResult
import androidx.media3.transformer.ProgressHolder
import androidx.media3.transformer.Transformer
import androidx.media3.transformer.VideoEncoderSettings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.coroutines.withContext
import java.io.File
import kotlin.coroutines.resume

/**
 * Сжатие фото и видео перед отправкой.
 *
 * Сжатие с потерями, но умеренное: цель — убрать вес, которого никто не заметит, а не выжать
 * минимальный размер. Кадр уменьшается только если он крупнее нужного, а результат уходит
 * лишь тогда, когда он реально меньше оригинала: иначе отправляется исходный файл.
 *
 * Кому нужен файл байт в байт, выбирает при отправке «как файл» — туда сжатие не заходит.
 */
object Compression {
    /** Длинная сторона фотографии: 2560 точек хватает и для экрана, и чтобы приблизить. */
    private const val ImageLongestEdge = 2560

    /** 85 — порог, ниже которого JPEG начинает мылить лица и градиенты. */
    private const val ImageQuality = 85

    /** Короткая сторона видео: 720p остаётся нормальным качеством на телефоне. */
    private const val VideoShortSide = 720
    private const val VideoBitrate = 2_500_000
    private const val ProgressPollMilliseconds = 200L

    /**
     * Пережимает фотографию в JPEG. Возвращает путь к результату или `null`, если сжимать
     * нечего или не вышло, — вызывающий тогда отправляет оригинал.
     *
     * Картинки с прозрачностью не трогаются: JPEG её не умеет, и прозрачный фон стал бы чёрным.
     */
    suspend fun image(sourcePath: String, targetPath: String): String? = withContext(Dispatchers.IO) {
        runCatching {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(sourcePath, bounds)
            val longest = maxOf(bounds.outWidth, bounds.outHeight)
            if (longest <= 0) return@runCatching null

            // Сначала грубо, прореживанием при чтении: панорама на сто мегапикселей иначе
            // не поместится в память ещё до того, как мы её уменьшим.
            val options = BitmapFactory.Options().apply {
                inSampleSize = sampleSize(longest, ImageLongestEdge)
            }
            val decoded = BitmapFactory.decodeFile(sourcePath, options) ?: return@runCatching null
            if (decoded.hasAlpha()) {
                decoded.recycle()
                return@runCatching null
            }
            val upright = applyExifRotation(sourcePath, scaleToFit(decoded, ImageLongestEdge))
            val target = File(targetPath)
            target.outputStream().use { upright.compress(Bitmap.CompressFormat.JPEG, ImageQuality, it) }
            upright.recycle()

            if (target.length() in 1 until File(sourcePath).length()) {
                targetPath
            } else {
                target.delete()
                null
            }
        }.getOrNull()
    }

    /**
     * Перекодирует видео в H.264/AAC с ограничением стороны кадра и битрейта.
     *
     * Transformer обязан жить на потоке с Looper, поэтому запускается он на главном; тяжёлая
     * работа всё равно идёт внутри, на аппаратном кодеке.
     */
    @OptIn(markerClass = [UnstableApi::class])
    suspend fun video(
        context: Context,
        sourcePath: String,
        targetPath: String,
        shortSide: Int,
        onProgress: (Int) -> Unit,
    ): String? {
        val scope = MainScope()
        var poller: Job? = null
        val exported = try {
            withContext(Dispatchers.Main) {
                suspendCancellableCoroutine { continuation ->
                    val transformer = Transformer.Builder(context)
                        .setVideoMimeType(MimeTypes.VIDEO_H264)
                        .setAudioMimeType(MimeTypes.AUDIO_AAC)
                        .setEncoderFactory(
                            DefaultEncoderFactory.Builder(context)
                                .setRequestedVideoEncoderSettings(
                                    VideoEncoderSettings.Builder().setBitrate(VideoBitrate).build()
                                )
                                .build()
                        )
                        .addListener(object : Transformer.Listener {
                            override fun onCompleted(composition: Composition, result: ExportResult) {
                                finish(true)
                            }

                            override fun onError(
                                composition: Composition,
                                result: ExportResult,
                                exception: ExportException,
                            ) {
                                finish(false)
                            }

                            private fun finish(success: Boolean) {
                                // Опрос прогресса живёт своей корутиной и сам не узнает, что всё
                                // кончилось: без остановки он крутился бы до конца процесса.
                                poller?.cancel()
                                if (continuation.isActive) continuation.resume(success)
                            }
                        })
                        .build()

                    // Кадр уменьшаем только если он крупнее цели: растянуть 480p до 720p значит
                    // одновременно потерять качество и прибавить вес.
                    val effects = if (shortSide > VideoShortSide) {
                        Effects(emptyList(), listOf(Presentation.createForShortSide(VideoShortSide)))
                    } else {
                        Effects(emptyList(), emptyList())
                    }
                    val item = EditedMediaItem.Builder(
                        MediaItem.fromUri(Uri.fromFile(File(sourcePath)))
                    ).setEffects(effects).build()

                    val holder = ProgressHolder()
                    poller = scope.launch {
                        while (isActive) {
                            if (transformer.getProgress(holder) == Transformer.PROGRESS_STATE_AVAILABLE) {
                                onProgress(holder.progress)
                            }
                            delay(ProgressPollMilliseconds)
                        }
                    }
                    // Обработчик отмены допускается ровно один: второй вызов бросает исключение
                    // сразу, ещё до всякой отмены.
                    continuation.invokeOnCancellation {
                        poller?.cancel()
                        transformer.cancel()
                    }
                    transformer.start(item, targetPath)
                }
            }
        } finally {
            scope.cancel()
        }

        val target = File(targetPath)
        // Ролик, снятый телефоном, часто уже хорошо сжат: перекодировать его в файл потяжелее —
        // худшее из возможного, поэтому в таком случае уходит оригинал.
        if (!exported || target.length() !in 1 until File(sourcePath).length()) {
            target.delete()
            return null
        }
        return targetPath
    }

    private fun sampleSize(longestEdge: Int, target: Int): Int {
        var sample = 1
        while (longestEdge / (sample * 2) >= target) sample *= 2
        return sample
    }

    private fun scaleToFit(source: Bitmap, longestEdge: Int): Bitmap {
        val longest = maxOf(source.width, source.height)
        if (longest <= longestEdge) return source
        val ratio = longestEdge.toDouble() / longest
        val scaled = Bitmap.createScaledBitmap(
            source,
            (source.width * ratio).toInt().coerceAtLeast(1),
            (source.height * ratio).toInt().coerceAtLeast(1),
            true,
        )
        if (scaled !== source) source.recycle()
        return scaled
    }

    /** Поворот снимка лежит в EXIF, а не в пикселях: без него портрет ляжет набок. */
    private fun applyExifRotation(sourcePath: String, bitmap: Bitmap): Bitmap {
        val orientation = runCatching {
            ExifInterface(sourcePath).getAttributeInt(
                ExifInterface.TAG_ORIENTATION,
                ExifInterface.ORIENTATION_NORMAL,
            )
        }.getOrDefault(ExifInterface.ORIENTATION_NORMAL)
        val matrix = Matrix()
        when (orientation) {
            ExifInterface.ORIENTATION_ROTATE_90 -> matrix.postRotate(90f)
            ExifInterface.ORIENTATION_ROTATE_180 -> matrix.postRotate(180f)
            ExifInterface.ORIENTATION_ROTATE_270 -> matrix.postRotate(270f)
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.postScale(-1f, 1f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.postScale(1f, -1f)
            else -> return bitmap
        }
        val rotated = Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true)
        if (rotated !== bitmap) bitmap.recycle()
        return rotated
    }
}
