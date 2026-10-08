package app.turattext.mobile.calls

import android.annotation.SuppressLint
import android.content.Context
import android.os.PowerManager
import app.turattext.mobile.core.NativeCore
import app.turattext.mobile.model.CoreJson
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Звонок глазами Android: опрашивает состояние в ядре и по его переходам включает звук,
 * гудки, мелодию, датчик приближения, сервис переднего плана и уведомления.
 *
 * Живёт на весь процесс, а не на экран: разговор не должен обрываться от поворота или от
 * того, что приложение свернули.
 */
@SuppressLint("StaticFieldLeak") // Только контекст приложения.
object CallController {
    private val _state = MutableStateFlow<CallState?>(null)
    /** Звонок для экрана; `null` — звонка нет. */
    val state = _state.asStateFlow()

    private val _speaker = MutableStateFlow(false)
    val speaker = _speaker.asStateFlow()

    /** Экран звонка свёрнут в строку над чатами. */
    private val _minimized = MutableStateFlow(false)
    val minimized = _minimized.asStateFlow()

    /** Короткая причина, почему звонок не начался: показывается на самом экране звонка. */
    private val _failure = MutableStateFlow<String?>(null)
    val failure = _failure.asStateFlow()

    /**
     * Команда ядру через очередь экрана, чтобы её снимок сразу попал в ленту. Пока экрана
     * нет, команда идёт в ядро напрямую.
     */
    @Volatile var runner: ((String) -> Unit)? = null

    /** Окно приложения видно: входящий показывается в нём, а не уведомлением. */
    @Volatile private var uiVisible = false

    private lateinit var context: Context
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var poller: Job? = null
    private var dialing: CallState? = null
    /** Сбросили, пока Node ещё создавала комнату: звонок из ядра завершается, едва появится. */
    @Volatile private var dialCancelled = false
    private var last: CallState? = null

    private val audio by lazy { CallAudio(context) }
    private val ringer by lazy { Ringer(context) }
    private val tones by lazy { CallTones() }
    private var proximity: PowerManager.WakeLock? = null
    private var serviceRunning = false

    fun attach(context: Context) {
        if (!::context.isInitialized) this.context = context.applicationContext
        if (poller?.isActive == true) return
        poller = scope.launch { poll() }
    }

    fun setUiVisible(visible: Boolean) {
        uiVisible = visible
        val call = last ?: return
        if (call.phase == CallPhase.Incoming) {
            if (visible) CallNotifications.cancelIncoming(context) else CallNotifications.showIncoming(context, call)
        }
    }

    /** Нажали «Позвонить»: экран открывается сразу, пока ядро создаёт комнату на Node. */
    fun dial(peerUserId: String, peerName: String, avatar: String?) {
        _failure.value = null
        _minimized.value = false
        _speaker.value = false
        dialCancelled = false
        dialing = CallState.dialing(peerUserId, peerName, avatar)
        _state.value = dialing
        audio.prepare(speakerOn = false)
        ensureService()
    }

    /** Ядро отказалось звонить: причина висит на экране пару секунд, потом он закрывается. */
    fun dialFailed(reason: String) {
        val placeholder = dialing ?: return
        dialing = null
        _failure.value = reason
        _state.value = placeholder.copy(phase = CallPhase.Ended, endReason = "failed")
        teardown()
        scope.launch {
            delay(FailureLingerMs)
            if (last == null && dialing == null) {
                _state.value = null
                _failure.value = null
            }
        }
    }

    fun dialStarted() {
        dialing = null
    }

    /** Ответ: звук готовится до того, как ядро подключится к ретранслятору. */
    fun answering() {
        ringer.stop()
        CallNotifications.cancelIncoming(context)
        _minimized.value = false
        audio.prepare(_speaker.value)
        ensureService()
    }

    fun hangUp() {
        if (dialing != null && last == null) {
            // Комната ещё создаётся: звонок завершится, как только ядро его заведёт.
            dialCancelled = true
            dialFailed("Звонок отменён")
            return
        }
        NativeCore.callAction("hangup")
        ringer.stop()
        CallNotifications.cancelIncoming(context)
        settle()
    }

    fun toggleMute() {
        val call = _state.value ?: return
        NativeCore.callAction(if (call.muted) "unmute" else "mute")
    }

    fun toggleSpeaker() {
        _speaker.value = !_speaker.value
        audio.setSpeaker(_speaker.value)
        updateProximity(last)
    }

    fun setMinimized(value: Boolean) {
        _minimized.value = value
    }

    private suspend fun poll() {
        while (scope.isActive) {
            var current = withContext(Dispatchers.IO) { CallState.parse(NativeCore.callStatus()) }
            if (current != null && current.live && current.outgoing && dialCancelled) {
                dialCancelled = false
                NativeCore.callAction("hangup")
                current = withContext(Dispatchers.IO) { CallState.parse(NativeCore.callStatus()) }
            }
            if (current != null) dialing = null
            val shown = current ?: dialing ?: _state.value?.takeIf { _failure.value != null }
            if (shown != _state.value) _state.value = shown
            transition(last, current)
            last = current
            delay(if (current != null || dialing != null) ActivePollMs else IdlePollMs)
        }
    }

    private fun transition(old: CallState?, new: CallState?) {
        if (new == null) {
            if (old != null) {
                teardown()
                settle()
                // Звонок закрыт: следующий снова начнётся на весь экран.
                _minimized.value = false
            }
            return
        }
        val previous = old?.takeIf { it.callId == new.callId }?.phase
        if (previous == new.phase) return
        if (old != null && old.callId != new.callId) teardown()
        when (new.phase) {
            CallPhase.Incoming -> {
                _minimized.value = false
                _speaker.value = false
                ringer.start()
                if (!uiVisible) CallNotifications.showIncoming(context, new)
            }
            CallPhase.Calling -> {
                audio.prepare(_speaker.value)
                ensureService()
            }
            CallPhase.Ringing -> {
                audio.prepare(_speaker.value)
                ensureService()
                tones.ringback()
            }
            CallPhase.Connecting, CallPhase.Active -> {
                ringer.stop()
                CallNotifications.cancelIncoming(context)
                tones.stop()
                audio.prepare(_speaker.value)
                audio.startStreams()
                ensureService()
            }
            CallPhase.Ended -> {
                val talked = previous == CallPhase.Active || previous == CallPhase.Connecting
                teardown()
                when {
                    new.outgoing && new.endReason == "busy" -> tones.busy()
                    talked || previous == CallPhase.Ringing || previous == CallPhase.Calling -> tones.ended()
                }
                settle()
            }
        }
        if (serviceRunning && new.phase != CallPhase.Ended) CallNotifications.updateOngoing(context, new)
        updateProximity(new)
    }

    /** Останавливает всё, что звонок включал на устройстве. Повторный вызов безвреден. */
    private fun teardown() {
        ringer.stop()
        tones.stop()
        audio.release()
        CallNotifications.cancelIncoming(context)
        if (serviceRunning) {
            serviceRunning = false
            CallService.stop(context)
        }
        updateProximity(null)
    }

    private fun ensureService() {
        if (serviceRunning) return
        serviceRunning = true
        CallService.start(context)
    }

    /** Отправить собеседнику конец звонка и записать его в историю — это работа ядра. */
    private fun settle() {
        val command = CoreJson.command("settle_calls")
        val run = runner
        if (run != null) {
            run(command)
        } else {
            scope.launch(Dispatchers.IO) { runCatching { NativeCore.invoke(command) } }
        }
    }

    /** У уха экран гаснет, на громкой связи и с гарнитурой — нет. */
    private fun updateProximity(call: CallState?) {
        val wanted = call != null && call.live && call.phase != CallPhase.Incoming &&
            !_speaker.value && !audio.headsetConnected()
        val lock = proximity ?: run {
            val power = context.getSystemService(PowerManager::class.java)
            if (!power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
            power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "turat:call").also {
                it.setReferenceCounted(false)
                proximity = it
            }
        }
        if (wanted && !lock.isHeld) {
            lock.acquire(MaxCallMs)
        } else if (!wanted && lock.isHeld) {
            lock.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY)
        }
    }

    private const val ActivePollMs = 60L
    private const val IdlePollMs = 400L
    private const val FailureLingerMs = 2_600L
    private const val MaxCallMs = 6 * 60 * 60 * 1000L
}
