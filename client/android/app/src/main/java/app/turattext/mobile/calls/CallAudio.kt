package app.turattext.mobile.calls

import android.annotation.SuppressLint
import android.app.NotificationManager
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.Ringtone
import android.media.RingtoneManager
import android.media.ToneGenerator
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.AudioEffect
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.os.VibrationAttributes
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.provider.Settings
import android.util.Log
import app.turattext.mobile.core.NativeCore
import kotlin.concurrent.thread

/**
 * Голос звонка: микрофон и динамик.
 *
 * Ядро работает кадрами по 20 мс при 48 кГц, моно. Запись отдаёт ему каждый кадр сразу,
 * а воспроизведение забирает кадр из джиттер-буфера ядра в темпе самого динамика: блокирующая
 * запись в [AudioTrack] и есть часы, по которым буфер решает, ждать пакет или маскировать
 * потерю. Эхо и шум гасит система — источник VOICE_COMMUNICATION плюс эффекты, если
 * устройство их предлагает.
 */
internal class CallAudio(context: Context) {
    private val audioManager = context.getSystemService(AudioManager::class.java)

    @Volatile private var streaming = false
    private var capture: Thread? = null
    private var playback: Thread? = null
    private var prepared = false
    private var previousMode = AudioManager.MODE_NORMAL
    private var focus: AudioFocusRequest? = null
    private var speaker = false
    private val main = Handler(Looper.getMainLooper())

    /** Наушники подключили или вынули посреди разговора — голос переезжает следом. */
    private val devices = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(addedDevices: Array<out AudioDeviceInfo>) = route()
        override fun onAudioDevicesRemoved(removedDevices: Array<out AudioDeviceInfo>) = route()
    }

    /** Режим связи, фокус и маршрут — до первого гудка, чтобы и гудки шли в ухо. */
    fun prepare(speakerOn: Boolean) {
        speaker = speakerOn
        if (prepared) {
            route()
            return
        }
        prepared = true
        previousMode = audioManager.mode
        requestFocus()
        audioManager.mode = AudioManager.MODE_IN_COMMUNICATION
        audioManager.registerAudioDeviceCallback(devices, main)
        route()
    }

    fun setSpeaker(on: Boolean) {
        speaker = on
        if (prepared) route()
    }

    /** Есть ли гарнитура: тогда датчик приближения не должен гасить экран. */
    fun headsetConnected(): Boolean =
        audioManager.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in HeadsetTypes }

    fun startStreams() {
        if (streaming) return
        streaming = true
        playback = thread(name = "turat-call-playback") { playLoop() }
        capture = thread(name = "turat-call-capture") { captureLoop() }
    }

    fun stopStreams() {
        streaming = false
        capture?.join(400)
        playback?.join(400)
        capture = null
        playback = null
    }

    fun release() {
        stopStreams()
        if (!prepared) return
        prepared = false
        runCatching { audioManager.unregisterAudioDeviceCallback(devices) }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            audioManager.clearCommunicationDevice()
        } else {
            @Suppress("DEPRECATION")
            audioManager.isSpeakerphoneOn = false
        }
        audioManager.mode = previousMode.takeIf { it != AudioManager.MODE_IN_COMMUNICATION } ?: AudioManager.MODE_NORMAL
        abandonFocus()
    }

    private fun route() {
        if (!prepared) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            val available = audioManager.availableCommunicationDevices
            val target = if (speaker) {
                available.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }
            } else {
                available.firstOrNull { it.type in HeadsetTypes }
                    ?: available.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_EARPIECE }
            }
            if (target == null || !audioManager.setCommunicationDevice(target)) {
                audioManager.clearCommunicationDevice()
            }
        } else {
            @Suppress("DEPRECATION")
            audioManager.isSpeakerphoneOn = speaker
        }
    }

    private fun requestFocus() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
                .setAudioAttributes(VoiceAttributes)
                .build()
            focus = request
            audioManager.requestAudioFocus(request)
        } else {
            @Suppress("DEPRECATION")
            audioManager.requestAudioFocus(null, AudioManager.STREAM_VOICE_CALL, AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        }
    }

    private fun abandonFocus() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focus?.let(audioManager::abandonAudioFocusRequest)
            focus = null
        } else {
            @Suppress("DEPRECATION")
            audioManager.abandonAudioFocus(null)
        }
    }

    @SuppressLint("MissingPermission")
    private fun captureLoop() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val minimum = AudioRecord.getMinBufferSize(SampleRate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val record = try {
            AudioRecord(
                MediaRecorder.AudioSource.VOICE_COMMUNICATION,
                SampleRate,
                AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT,
                // Запас на 200 мс: если поток записи на миг задержится (сборка мусора, смена
                // маршрута звука), система не потеряет отсчёты — разрыв в них слышен треском.
                maxOf(minimum, FrameSamples * 2 * 10),
            )
        } catch (error: Exception) {
            Log.w(Tag, "Микрофон недоступен", error)
            return
        }
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            Log.w(Tag, "Микрофон не инициализировался")
            record.release()
            return
        }
        // Автоусиление (AGC) сверх системной обработки не включаем: в тишине оно поднимает
        // остаток эха и шум до громкости речи, и на громкой связи звук начинает «заводиться» —
        // отсюда нарастающие резкие шумы у собеседника. Источник VOICE_COMMUNICATION уже
        // выравнивает громкость сам.
        val effects = buildList<AudioEffect> {
            if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(record.audioSessionId)?.let(::add)
            if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(record.audioSessionId)?.let(::add)
        }
        effects.forEach { runCatching { it.enabled = true } }
        val frame = ShortArray(FrameSamples)
        try {
            record.startRecording()
            while (streaming) {
                var filled = 0
                while (filled < FrameSamples && streaming) {
                    val read = record.read(frame, filled, FrameSamples - filled)
                    if (read < 0) {
                        Log.w(Tag, "Чтение микрофона: $read")
                        return
                    }
                    filled += read
                }
                if (filled == FrameSamples) NativeCore.callPush(frame)
            }
        } catch (error: Exception) {
            Log.w(Tag, "Запись прервалась", error)
        } finally {
            runCatching { record.stop() }
            effects.forEach { runCatching { it.release() } }
            record.release()
        }
    }

    private fun playLoop() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
        val minimum = AudioTrack.getMinBufferSize(SampleRate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val builder = AudioTrack.Builder()
            .setAudioAttributes(VoiceAttributes)
            .setAudioFormat(
                AudioFormat.Builder()
                    .setSampleRate(SampleRate)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .build(),
            )
            .setBufferSizeInBytes(maxOf(minimum, FrameSamples * 2 * 3))
            .setTransferMode(AudioTrack.MODE_STREAM)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            builder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
        }
        val track = try {
            builder.build()
        } catch (error: Exception) {
            Log.w(Tag, "Динамик недоступен", error)
            return
        }
        val frame = ShortArray(FrameSamples)
        try {
            track.play()
            while (streaming) {
                if (!NativeCore.callPull(frame)) frame.fill(0)
                var written = 0
                while (written < FrameSamples && streaming) {
                    val result = track.write(frame, written, FrameSamples - written)
                    if (result < 0) return
                    written += result
                }
            }
        } catch (error: Exception) {
            Log.w(Tag, "Воспроизведение прервалось", error)
        } finally {
            runCatching { track.pause(); track.flush() }
            track.release()
        }
    }

    companion object {
        const val SampleRate = 48_000
        const val FrameSamples = 960
        private const val Tag = "TuratCall"
        private val VoiceAttributes: AudioAttributes = AudioAttributes.Builder()
            .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
            .build()
        private val HeadsetTypes = buildSet {
            add(AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
            add(AudioDeviceInfo.TYPE_WIRED_HEADSET)
            add(AudioDeviceInfo.TYPE_WIRED_HEADPHONES)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) add(AudioDeviceInfo.TYPE_USB_HEADSET)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) add(AudioDeviceInfo.TYPE_BLE_HEADSET)
        }
    }
}

/**
 * Мелодия и вибрация входящего вызова.
 *
 * Мелодия играет только в обычном режиме звука. Вибрация — в любом режиме, в том числе
 * «Без звука»: пропустить звонок только потому, что телефон беззвучный, обидно. Исключение
 * одно — «Не беспокоить»: тогда звонок лишь показывается на экране, без звука и вибрации.
 */
internal class Ringer(private val context: Context) {
    private var ringtone: Ringtone? = null
    private var vibrator: Vibrator? = null
    private val main = Handler(Looper.getMainLooper())

    /** До Android 9 у мелодии нет повтора: перезапускаем её сами, пока звонок не принят. */
    private val loop = object : Runnable {
        override fun run() {
            val current = ringtone ?: return
            if (!current.isPlaying) current.play()
            main.postDelayed(this, 1_000)
        }
    }

    fun start() {
        if (ringtone != null || vibrator != null) return
        if (doNotDisturb()) return
        val audio = context.getSystemService(AudioManager::class.java)
        val mode = audio.ringerMode
        if (mode == AudioManager.RINGER_MODE_NORMAL) {
            val uri = RingtoneManager.getActualDefaultRingtoneUri(context, RingtoneManager.TYPE_RINGTONE)
                ?: Settings.System.DEFAULT_RINGTONE_URI
            ringtone = runCatching { RingtoneManager.getRingtone(context, uri) }.getOrNull()?.apply {
                audioAttributes = AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                    .build()
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) isLooping = true
                runCatching { play() }
            }
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) main.postDelayed(loop, 1_000)
        }
        vibrate(silent = mode == AudioManager.RINGER_MODE_SILENT)
    }

    /** Включён ли «Не беспокоить» — в любом его виде (только важные, только будильники, полная тишина). */
    private fun doNotDisturb(): Boolean {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return false
        val filter = manager.currentInterruptionFilter
        return filter != NotificationManager.INTERRUPTION_FILTER_ALL &&
            filter != NotificationManager.INTERRUPTION_FILTER_UNKNOWN
    }

    /**
     * Вибрация вызова. Назначение важно: вибрацию «мелодии» и уведомлений система глушит в
     * режиме «Без звука», а вибрацию приложения в фоне без назначения — всегда. Поэтому вызов
     * помечен как запрос на связь, а в беззвучном режиме — как будильник: эту вибрацию режим
     * «Без звука» не отключает. «Не беспокоить» проверено выше.
     */
    private fun vibrate(silent: Boolean) {
        val device = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            context.getSystemService(VibratorManager::class.java)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            context.getSystemService(Vibrator::class.java)
        }
        val target = device?.takeIf { it.hasVibrator() } ?: return
        val pattern = longArrayOf(0, 700, 900)
        val legacyUsage = if (silent) AudioAttributes.USAGE_ALARM else AudioAttributes.USAGE_NOTIFICATION_COMMUNICATION_REQUEST
        val legacyAttributes = AudioAttributes.Builder()
            .setUsage(legacyUsage)
            .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
            .build()
        runCatching {
            when {
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU -> target.vibrate(
                    VibrationEffect.createWaveform(pattern, 0),
                    VibrationAttributes.createForUsage(
                        if (silent) VibrationAttributes.USAGE_ALARM else VibrationAttributes.USAGE_COMMUNICATION_REQUEST,
                    ),
                )
                Build.VERSION.SDK_INT >= Build.VERSION_CODES.O ->
                    target.vibrate(VibrationEffect.createWaveform(pattern, 0), legacyAttributes)
                else -> {
                    @Suppress("DEPRECATION")
                    target.vibrate(pattern, 0, legacyAttributes)
                }
            }
            vibrator = target
        }.onFailure { Log.w("TuratCall", "Вибрация вызова недоступна", it) }
    }

    fun stop() {
        main.removeCallbacks(loop)
        ringtone?.let { runCatching { it.stop() } }
        ringtone = null
        vibrator?.cancel()
        vibrator = null
    }
}

/** Гудки на стороне звонящего: длинные, «занято» и короткий сигнал конца разговора. */
internal class CallTones {
    private var generator: ToneGenerator? = null
    private val main = Handler(Looper.getMainLooper())

    fun ringback() = play(ToneGenerator.TONE_SUP_RINGTONE, -1)
    fun busy() = play(ToneGenerator.TONE_SUP_BUSY, 2_400)
    fun ended() = play(ToneGenerator.TONE_PROP_PROMPT, 300)
    fun connected() = play(ToneGenerator.TONE_PROP_BEEP2, 200)

    private fun play(tone: Int, durationMs: Int) {
        stop()
        val created = runCatching { ToneGenerator(AudioManager.STREAM_VOICE_CALL, 70) }.getOrNull() ?: return
        generator = created
        created.startTone(tone, durationMs)
        if (durationMs > 0) {
            main.postDelayed({ if (generator === created) stop() }, durationMs + 200L)
        }
    }

    fun stop() {
        generator?.let {
            runCatching { it.stopTone() }
            it.release()
        }
        generator = null
    }
}
