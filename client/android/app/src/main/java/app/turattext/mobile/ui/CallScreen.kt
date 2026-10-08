package app.turattext.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.keyframes
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.turattext.mobile.R
import app.turattext.mobile.calls.CallPhase
import app.turattext.mobile.calls.CallState
import app.turattext.mobile.calls.formatCallDuration
import app.turattext.mobile.calls.statusLine
import kotlin.math.roundToInt
import kotlin.math.sin

/** Классические цвета трубки: их узнают с первого взгляда в любой теме. */
private val AcceptGreen = Color(0xFF3FBF5F)
private val HangUpRed = Color(0xFFE5484D)

/** Что экран звонка умеет просить у [app.turattext.mobile.calls.CallController]. */
class CallUiActions(
    val accept: () -> Unit,
    val hangUp: () -> Unit,
    val toggleMute: () -> Unit,
    val toggleSpeaker: () -> Unit,
    val setMinimized: (Boolean) -> Unit,
)

/**
 * Слой звонка поверх приложения: полноэкранный экран или, если его свернули, плавающий
 * пузырь с собеседником, за который можно потянуть и который возвращает экран по нажатию.
 */
@Composable
fun CallLayer(
    call: CallState?,
    failure: String?,
    speaker: Boolean,
    minimized: Boolean,
    actions: CallUiActions,
) {
    // Последний звонок держим, пока экран уезжает: иначе анимация закрытия рисовала бы пустоту.
    var shown by remember { mutableStateOf(call) }
    if (call != null) shown = call
    AnimatedVisibility(
        visible = call != null && !minimized,
        enter = fadeIn(tween(260)) + scaleIn(tween(320, easing = FastOutSlowInEasing), initialScale = 0.94f) +
            slideInVertically(tween(320, easing = FastOutSlowInEasing)) { it / 10 },
        exit = fadeOut(tween(220)) + scaleOut(tween(240), targetScale = 0.96f) +
            slideOutVertically(tween(240)) { it / 12 },
    ) {
        shown?.let { CallScreen(it, failure, speaker, actions) }
    }
    AnimatedVisibility(
        visible = call != null && minimized,
        enter = fadeIn() + scaleIn(spring(dampingRatio = Spring.DampingRatioMediumBouncy), initialScale = 0.4f),
        exit = fadeOut() + scaleOut(targetScale = 0.4f),
    ) {
        shown?.let { CallBubble(it) { actions.setMinimized(false) } }
    }
}

@Composable
private fun CallScreen(call: CallState, failure: String?, speaker: Boolean, actions: CallUiActions) {
    val colors = Telegram.colors
    BackHandler { actions.setMinimized(true) }
    AuroraBackground(Modifier.fillMaxSize()) {
        // Касания не должны проваливаться в чат под экраном звонка.
        Box(Modifier.fillMaxSize().clickable(interactionSource = null, indication = null) {}) {
            CallGlow(call)
            Column(
                Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().padding(horizontal = 20.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                TopBar(call, onMinimize = { actions.setMinimized(true) })
                Spacer(Modifier.weight(0.8f))
                PeerAvatar(call, diameter = 148.dp)
                Spacer(Modifier.height(26.dp))
                Text(
                    call.peerName,
                    color = colors.text,
                    fontSize = 28.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    textAlign = TextAlign.Center,
                )
                Spacer(Modifier.height(8.dp))
                StatusLine(call, failure)
                Spacer(Modifier.height(18.dp))
                Box(Modifier.height(44.dp), contentAlignment = Alignment.Center) {
                    androidx.compose.animation.AnimatedVisibility(
                        visible = call.phase == CallPhase.Active,
                        enter = fadeIn() + expandVertically(),
                        exit = fadeOut() + shrinkVertically(),
                    ) {
                        VoiceWave(call)
                    }
                }
                Hints(call)
                Spacer(Modifier.weight(1f))
                SafetyCode(call)
                Spacer(Modifier.height(28.dp))
                Controls(call, speaker, actions)
                Spacer(Modifier.height(28.dp))
            }
        }
    }
}

/** Мягкое сияние за собеседником: дышит в такт его голосу. */
@Composable
private fun CallGlow(call: CallState) {
    val colors = Telegram.colors
    val level by animateFloatAsState(
        if (call.phase == CallPhase.Active) call.peerLevel else 0f,
        animationSpec = spring(stiffness = Spring.StiffnessLow),
        label = "glow",
    )
    val breathing = rememberInfiniteTransition(label = "breath")
    val breath by breathing.animateFloat(
        0.85f, 1.05f,
        infiniteRepeatable(tween(2600, easing = FastOutSlowInEasing), RepeatMode.Reverse),
        label = "breath",
    )
    val tint = when (call.phase) {
        CallPhase.Ended -> colors.hint
        CallPhase.Incoming -> AcceptGreen
        else -> colors.accent
    }
    Canvas(Modifier.fillMaxSize()) {
        val center = Offset(size.width / 2f, size.height * 0.36f)
        val radius = size.minDimension * (0.55f + level * 0.25f) * breath
        drawCircle(
            Brush.radialGradient(
                listOf(tint.copy(alpha = if (colors.night) 0.34f else 0.24f), Color.Transparent),
                center = center,
                radius = radius,
            ),
            radius = radius,
            center = center,
        )
    }
}

@Composable
private fun TopBar(call: CallState, onMinimize: () -> Unit) {
    val colors = Telegram.colors
    Row(
        Modifier.fillMaxWidth().height(56.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(40.dp).clip(CircleShape).glass(colors, CircleShape)
                .clickable(role = Role.Button, onClick = onMinimize),
            contentAlignment = Alignment.Center,
        ) {
            Icon(painterResource(R.drawable.ic_arrow_down), "Свернуть", Modifier.size(20.dp), colors.text)
        }
        Spacer(Modifier.weight(1f))
        Row(
            Modifier.clip(GlassShape.Capsule).glass(colors, GlassShape.Capsule)
                .padding(horizontal = 12.dp, vertical = 7.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(painterResource(R.drawable.ic_lock), null, Modifier.size(14.dp), colors.accent)
            Spacer(Modifier.width(6.dp))
            Text("Сквозное шифрование", color = colors.text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        }
        Spacer(Modifier.weight(1f))
        Box(Modifier.size(40.dp), contentAlignment = Alignment.Center) {
            if (call.hasMedia) SignalBars(call.quality)
        }
    }
}

/** Три столбика качества связи: зелёные, жёлтые или красные, как у сотовой сети. */
@Composable
private fun SignalBars(quality: String) {
    val colors = Telegram.colors
    val lit = when (quality) {
        "poor" -> 1
        "fair" -> 2
        else -> 3
    }
    val tint by androidx.compose.animation.animateColorAsState(
        when (quality) {
            "poor" -> HangUpRed
            "fair" -> Color(0xFFE0A63A)
            else -> AcceptGreen
        },
        label = "signal",
    )
    Canvas(Modifier.size(20.dp, 16.dp)) {
        val bar = size.width / 5f
        repeat(3) { index ->
            val height = size.height * (0.4f + 0.3f * index)
            drawRoundRect(
                color = if (index < lit) tint else colors.hint.copy(alpha = 0.35f),
                topLeft = Offset(index * bar * 2f, size.height - height),
                size = Size(bar, height),
                cornerRadius = CornerRadius(bar / 2f),
            )
        }
    }
}

/**
 * Аватар собеседника. Пока звонит — от него расходятся волны, во время разговора вокруг
 * мягко пульсирует кольцо от громкости его голоса, после конца звонка он тускнеет.
 */
@Composable
private fun PeerAvatar(call: CallState, diameter: androidx.compose.ui.unit.Dp) {
    val colors = Telegram.colors
    val ringing = call.ringing
    val tint = if (call.phase == CallPhase.Incoming) AcceptGreen else colors.accent
    val waves = rememberInfiniteTransition(label = "waves")
    val progress by waves.animateFloat(
        0f, 1f,
        infiniteRepeatable(tween(2400, easing = LinearEasing)),
        label = "wave",
    )
    val level by animateFloatAsState(
        if (call.phase == CallPhase.Active) call.peerLevel else 0f,
        animationSpec = spring(dampingRatio = 0.6f, stiffness = Spring.StiffnessMediumLow),
        label = "level",
    )
    val dim by animateFloatAsState(if (call.phase == CallPhase.Ended) 0.55f else 1f, label = "dim")
    val pop = remember { Animatable(0.6f) }
    LaunchedEffect(Unit) { pop.animateTo(1f, spring(dampingRatio = 0.55f, stiffness = Spring.StiffnessLow)) }
    Box(
        Modifier.size(diameter * 1.9f).drawBehind {
            val base = diameter.toPx() / 2f
            if (ringing) {
                repeat(3) { index ->
                    val phase = (progress + index / 3f) % 1f
                    val radius = base * (1f + phase * 0.85f)
                    drawCircle(
                        color = tint.copy(alpha = (1f - phase) * 0.42f),
                        radius = radius,
                        style = Stroke(width = (2.5f + (1f - phase) * 3f).dp.toPx()),
                    )
                }
            } else if (call.phase == CallPhase.Active || call.phase == CallPhase.Connecting) {
                val halo = base * (1.08f + level * 0.32f)
                drawCircle(
                    Brush.radialGradient(
                        listOf(colors.accent.copy(alpha = 0.45f), colors.accent.copy(alpha = 0f)),
                        radius = halo * 1.25f,
                    ),
                    radius = halo * 1.25f,
                )
                drawCircle(
                    color = colors.accent.copy(alpha = 0.55f + level * 0.4f),
                    radius = halo,
                    style = Stroke(width = 2.dp.toPx()),
                )
            }
        },
        contentAlignment = Alignment.Center,
    ) {
        Avatar(
            name = call.peerName,
            key = call.peerUserId,
            avatarBase64 = call.peerAvatarBase64,
            size = diameter,
            modifier = Modifier.graphicsLayer {
                val scale = pop.value * (1f + level * 0.05f)
                scaleX = scale
                scaleY = scale
                alpha = dim
            },
        )
    }
}

@Composable
private fun StatusLine(call: CallState, failure: String?) {
    val colors = Telegram.colors
    val key = when {
        failure != null -> "failure"
        call.phase == CallPhase.Active -> "active"
        else -> call.phase.name + (call.endReason ?: "")
    }
    AnimatedContent(
        targetState = key,
        transitionSpec = {
            (fadeIn(tween(220)) + slideInVertically { it / 2 }) togetherWith (fadeOut(tween(160)) + slideOutVertically { -it / 2 })
        },
        label = "status",
    ) { state ->
        when {
            state == "failure" -> Text(failure.orEmpty(), color = HangUpRed, fontSize = 16.sp, textAlign = TextAlign.Center)
            state == "active" -> Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    formatCallDuration(call.durationMs),
                    color = colors.text.copy(alpha = 0.85f),
                    fontSize = 17.sp,
                    fontWeight = FontWeight.Medium,
                )
                Text("  ·  ", color = colors.hint, fontSize = 15.sp)
                Icon(painterResource(R.drawable.ic_lock), null, Modifier.size(13.dp), colors.accent)
                Spacer(Modifier.width(4.dp))
                Text("E2EE", color = colors.accent, fontSize = 14.sp, fontWeight = FontWeight.SemiBold)
            }
            call.phase == CallPhase.Ended -> Text(call.statusLine(), color = colors.hint, fontSize = 16.sp)
            else -> Row(verticalAlignment = Alignment.Bottom) {
                Text(call.statusLine().trimEnd('…'), color = colors.hint, fontSize = 16.sp)
                AnimatedDots(colors.hint)
            }
        }
    }
}

/** Три точки, бегущие волной: «Вызов…», «Соединение…». */
@Composable
private fun AnimatedDots(color: Color) {
    val transition = rememberInfiniteTransition(label = "dots")
    Row(Modifier.padding(start = 2.dp, bottom = 5.dp)) {
        repeat(3) { index ->
            val lift by transition.animateFloat(
                0f, 0f,
                infiniteRepeatable(
                    keyframes {
                        durationMillis = 1200
                        0f at index * 150
                        -4f at index * 150 + 200
                        0f at index * 150 + 420
                    },
                ),
                label = "dot$index",
            )
            Box(
                Modifier.padding(horizontal = 1.5.dp).offset { IntOffset(0, (lift * density).roundToInt()) }
                    .size(4.dp).clip(CircleShape).background(color),
            )
        }
    }
}

/**
 * Звуковая волна под именем: столбики живут от громкости обоих собеседников — громче
 * говорят, выше поднимаются, тишина укладывает их в ровную линию.
 */
@Composable
private fun VoiceWave(call: CallState) {
    val colors = Telegram.colors
    val target = maxOf(call.peerLevel, if (call.muted) 0f else call.localLevel * 0.7f)
    val level by animateFloatAsState(target, spring(stiffness = Spring.StiffnessMedium), label = "wave")
    var time by remember { mutableFloatStateOf(0f) }
    LaunchedEffect(Unit) {
        val start = withFrameMillis { it }
        while (true) {
            withFrameMillis { time = (it - start) / 1000f }
        }
    }
    Canvas(Modifier.width(196.dp).height(40.dp)) {
        val bars = 21
        val gap = size.width / bars
        val width = gap * 0.46f
        for (index in 0 until bars) {
            val distance = kotlin.math.abs(index - bars / 2) / (bars / 2f)
            val envelope = 1f - distance * 0.7f
            val motion = 0.55f + 0.45f * sin(time * 7.5f + index * 0.85f)
            val height = (width + size.height * level.coerceIn(0f, 1f) * envelope * motion)
                .coerceIn(width, size.height)
            val x = index * gap + (gap - width) / 2f
            drawRoundRect(
                Brush.verticalGradient(
                    listOf(colors.accent, colors.accent.copy(alpha = 0.55f)),
                    startY = (size.height - height) / 2f,
                    endY = (size.height + height) / 2f,
                ),
                topLeft = Offset(x, (size.height - height) / 2f),
                size = Size(width, height),
                cornerRadius = CornerRadius(width / 2f),
            )
        }
    }
}

/** Пояснения под волной: выключенный микрофон собеседника, слабая связь, обходной путь. */
@Composable
private fun Hints(call: CallState) {
    val hint = when {
        call.phase != CallPhase.Active -> null
        call.peerMuted -> "Собеседник выключил микрофон"
        call.quality == "poor" -> "Слабая связь — держим разговор"
        call.muted -> "Ваш микрофон выключен"
        else -> null
    }
    Box(Modifier.height(40.dp), contentAlignment = Alignment.Center) {
        AnimatedContent(
            targetState = hint,
            transitionSpec = { fadeIn(tween(200)) togetherWith fadeOut(tween(200)) },
            label = "hint",
        ) { text -> if (text != null) ServicePill(text) }
    }
}

/**
 * Четыре эмодзи из ключа звонка. Если у собеседника те же самые, между вами никого нет:
 * ключ родился на двух устройствах и Node его не видела.
 */
@Composable
private fun SafetyCode(call: CallState) {
    val colors = Telegram.colors
    var expanded by remember { mutableStateOf(false) }
    val visible = call.safetyCode.size == 4 && call.phase != CallPhase.Ended
    Box(Modifier.height(if (expanded) 132.dp else 56.dp), contentAlignment = Alignment.BottomCenter) {
        androidx.compose.animation.AnimatedVisibility(
            visible = visible,
            enter = fadeIn() + slideInVertically { it / 2 },
            exit = fadeOut(),
        ) {
            Column(
                Modifier.widthIn(max = 340.dp).clip(GlassShape.Card).glass(colors, GlassShape.Card, raised = true)
                    .clickable { expanded = !expanded }
                    .padding(horizontal = 16.dp, vertical = 10.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    call.safetyCode.forEachIndexed { index, emoji ->
                        val appear = remember { Animatable(0f) }
                        LaunchedEffect(Unit) {
                            kotlinx.coroutines.delay(index * 90L)
                            appear.animateTo(1f, spring(dampingRatio = 0.45f, stiffness = Spring.StiffnessMediumLow))
                        }
                        Text(
                            emoji,
                            Modifier.padding(horizontal = 5.dp).scale(appear.value).graphicsLayer { alpha = appear.value.coerceIn(0f, 1f) },
                            fontSize = 24.sp,
                        )
                    }
                }
                AnimatedVisibility(expanded, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
                    Text(
                        "Сравните эмодзи с собеседником. Совпадают — разговор защищён сквозным шифрованием: " +
                            "ключ создан на ваших устройствах, Node и ретранслятор слышат только шум.",
                        Modifier.padding(top = 8.dp),
                        color = colors.hint,
                        fontSize = 13.sp,
                        lineHeight = 17.sp,
                        textAlign = TextAlign.Center,
                    )
                }
            }
        }
    }
}

@Composable
private fun Controls(call: CallState, speaker: Boolean, actions: CallUiActions) {
    AnimatedContent(
        targetState = when (call.phase) {
            CallPhase.Incoming -> 0
            CallPhase.Ended -> 2
            else -> 1
        },
        transitionSpec = {
            (fadeIn(tween(260)) + slideInVertically { it / 3 }) togetherWith (fadeOut(tween(180)) + slideOutVertically { it / 3 })
        },
        label = "controls",
    ) { mode ->
        when (mode) {
            0 -> Row(
                Modifier.fillMaxWidth().padding(horizontal = 18.dp),
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                CallButton(R.drawable.ic_call_end, "Отклонить", HangUpRed, onClick = actions.hangUp)
                CallButton(R.drawable.ic_call, "Ответить", AcceptGreen, invite = true, onClick = actions.accept)
            }
            1 -> Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceEvenly,
            ) {
                ToggleButton(R.drawable.ic_speaker, "Динамик", speaker, onClick = actions.toggleSpeaker)
                ToggleButton(
                    if (call.muted) R.drawable.ic_mic_off else R.drawable.ic_mic,
                    if (call.muted) "Микрофон выкл." else "Микрофон",
                    call.muted,
                    onClick = actions.toggleMute,
                )
                CallButton(R.drawable.ic_call_end, "Завершить", HangUpRed, onClick = actions.hangUp)
            }
            else -> Box(Modifier.fillMaxWidth().height(100.dp))
        }
    }
}

/** Нажатие слегка вдавливает кнопку — как живую. */
@Composable
private fun pressScale(source: MutableInteractionSource): Float {
    val pressed by source.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.9f else 1f, spring(dampingRatio = 0.5f), label = "press")
    return scale
}

@Composable
private fun CallButton(
    icon: Int,
    label: String,
    color: Color,
    invite: Boolean = false,
    onClick: () -> Unit,
) {
    val colors = Telegram.colors
    val source = remember { MutableInteractionSource() }
    val press = pressScale(source)
    val motion = rememberInfiniteTransition(label = "invite")
    // Кнопка «Ответить» зовёт: пульсирует и покачивает трубкой.
    val pulse by motion.animateFloat(
        0f, 1f, infiniteRepeatable(tween(1500, easing = LinearEasing)), label = "pulse",
    )
    val wiggle by motion.animateFloat(
        0f, 0f,
        infiniteRepeatable(
            keyframes {
                durationMillis = 1500
                0f at 0
                -14f at 90
                14f at 180
                -12f at 270
                10f at 360
                0f at 450
            },
        ),
        label = "wiggle",
    )
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(
            Modifier.size(84.dp).drawBehind {
                if (invite) {
                    val radius = size.minDimension / 2f * 0.86f * (1f + pulse * 0.45f)
                    drawCircle(color.copy(alpha = (1f - pulse) * 0.45f), radius)
                }
            },
            contentAlignment = Alignment.Center,
        ) {
            Box(
                Modifier.size(72.dp).scale(press).clip(CircleShape)
                    .background(Brush.verticalGradient(listOf(color.copy(alpha = 0.92f), color)))
                    .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onClick),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painterResource(icon),
                    label,
                    Modifier.size(32.dp).graphicsLayer { rotationZ = if (invite) wiggle else 0f },
                    Color.White,
                )
            }
        }
        Spacer(Modifier.height(4.dp))
        Text(label, color = colors.text.copy(alpha = 0.85f), fontSize = 13.sp)
    }
}

@Composable
private fun ToggleButton(icon: Int, label: String, active: Boolean, onClick: () -> Unit) {
    val colors = Telegram.colors
    val source = remember { MutableInteractionSource() }
    val press = pressScale(source)
    val fill by androidx.compose.animation.animateColorAsState(
        if (active) colors.text else Color.Transparent,
        tween(220),
        label = "fill",
    )
    val tint by androidx.compose.animation.animateColorAsState(
        if (active) colors.window else colors.text,
        tween(220),
        label = "tint",
    )
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(84.dp), contentAlignment = Alignment.Center) {
            Box(
                Modifier.size(64.dp).scale(press).clip(CircleShape).glass(colors, CircleShape, raised = true)
                    .background(fill, CircleShape)
                    .clickable(interactionSource = source, indication = null, role = Role.Button, onClick = onClick),
                contentAlignment = Alignment.Center,
            ) {
                Icon(painterResource(icon), label, Modifier.size(26.dp), tint)
            }
        }
        Spacer(Modifier.height(4.dp))
        Text(label, color = colors.text.copy(alpha = 0.85f), fontSize = 13.sp)
    }
}

/**
 * Свёрнутый звонок: аватар в пульсирующем кольце и таймер. Его можно перетащить к любому
 * краю, нажатие разворачивает экран обратно.
 */
@Composable
private fun CallBubble(call: CallState, onOpen: () -> Unit) {
    val colors = Telegram.colors
    val density = LocalDensity.current
    BoxWithConstraints(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
        val bubble = 76.dp
        val maxX = with(density) { (maxWidth - bubble - 12.dp).toPx() }
        val maxY = with(density) { (maxHeight - bubble - 40.dp).toPx() }
        var x by remember { mutableFloatStateOf(maxX) }
        var y by remember { mutableFloatStateOf(with(density) { 84.dp.toPx() }) }
        var dragging by remember { mutableStateOf(false) }
        // Отпущенный пузырь прилипает к ближайшему краю.
        val settledX by animateDpAsState(
            with(density) { (if (dragging) x else if (x < maxX / 2) 12.dp.toPx() else maxX).toDp() },
            animationSpec = if (dragging) spring(stiffness = Spring.StiffnessHigh) else spring(dampingRatio = 0.7f),
            label = "bubble",
        )
        val pulse = rememberInfiniteTransition(label = "bubble-pulse")
        val ring by pulse.animateFloat(
            0f, 1f, infiniteRepeatable(tween(1800, easing = LinearEasing)), label = "ring",
        )
        val tint = if (call.phase == CallPhase.Ended) HangUpRed else if (call.phase == CallPhase.Incoming) AcceptGreen else colors.accent
        Column(
            Modifier.offset { IntOffset(settledX.roundToPx(), y.roundToInt()) }
                .pointerInput(maxX, maxY) {
                    detectDragGestures(
                        onDragStart = { dragging = true },
                        onDragEnd = {
                            dragging = false
                            x = if (x < maxX / 2) 12.dp.toPx() else maxX
                        },
                        onDragCancel = { dragging = false },
                    ) { change, amount ->
                        change.consume()
                        x = (x + amount.x).coerceIn(0f, maxX)
                        y = (y + amount.y).coerceIn(0f, maxY)
                    }
                }
                .clickable(interactionSource = null, indication = null, onClick = onOpen),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Box(
                Modifier.size(bubble).drawBehind {
                    val base = size.minDimension / 2f * 0.84f
                    drawCircle(tint.copy(alpha = (1f - ring) * 0.5f), base * (1f + ring * 0.22f), style = Stroke(3.dp.toPx()))
                },
                contentAlignment = Alignment.Center,
            ) {
                Box(
                    Modifier.size(60.dp).clip(CircleShape).glass(colors, CircleShape, raised = true).padding(3.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Avatar(call.peerName, call.peerUserId, call.peerAvatarBase64, size = 54.dp)
                }
            }
            Text(
                if (call.phase == CallPhase.Active) formatCallDuration(call.durationMs) else call.statusLine().trimEnd('…'),
                Modifier.clip(GlassShape.Capsule).glass(colors, GlassShape.Capsule, raised = true)
                    .padding(horizontal = 8.dp, vertical = 2.dp),
                color = colors.text,
                fontSize = 12.sp,
                fontWeight = FontWeight.Medium,
                maxLines = 1,
            )
        }
    }
}

/** Иконка трубки для шапки диалога. */
@Composable
fun CallIconButton(enabled: Boolean, onClick: () -> Unit) {
    val colors = Telegram.colors
    Box(
        Modifier.size(44.dp).clip(CircleShape).clickable(enabled = enabled, role = Role.Button, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            painterResource(R.drawable.ic_call),
            "Позвонить",
            Modifier.size(21.dp),
            if (enabled) colors.text else colors.hint.copy(alpha = 0.5f),
        )
    }
}
