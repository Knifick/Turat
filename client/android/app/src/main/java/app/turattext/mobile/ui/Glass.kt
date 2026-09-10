package app.turattext.mobile.ui

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * Материал Liquid Glass.
 *
 * Стекло видно только тогда, когда сквозь него есть на что смотреть, поэтому за всеми экранами
 * лежит [AuroraBackground] — мягкие цветные пятна, собранные из акцента текущей темы. Панели
 * поверх него полупрозрачны, светлеют к верхнему краю и обведены тонкой светлой кромкой: так
 * читается толщина стекла и его наклон к источнику света.
 *
 * Все значения выводятся из уже существующей палитры, поэтому новая тема получает стекло сама,
 * без отдельного набора цветов.
 */

/** Основная заливка стекла: панели, шапки, поле ввода. */
val TuratPalette.glass: Color
    get() = panel.copy(alpha = if (night) 0.52f else 0.62f)

/** Плотное стекло для приподнятых поверхностей: меню, карточки, всплывающие панели. */
val TuratPalette.glassRaised: Color
    get() = elevated.copy(alpha = if (night) 0.74f else 0.82f)

/** Блик по верхней кромке — «толщина» стекла. */
val TuratPalette.glassRim: Color
    get() = if (night) Color.White.copy(alpha = 0.16f) else Color.White.copy(alpha = 0.78f)

/** Затенение по нижней кромке: без него стекло выглядит наклейкой. */
val TuratPalette.glassEdge: Color
    get() = if (night) Color.Black.copy(alpha = 0.24f) else Color.Black.copy(alpha = 0.05f)

/** Стеклянный пузырь входящего сообщения. */
val TuratPalette.glassBubble: Color
    get() = bubbleIn.copy(alpha = if (night) 0.62f else 0.72f)

/** Сдвиг оттенка: из одного акцента получается разноцветное свечение фона. */
private fun Color.shiftHue(degrees: Float, alpha: Float): Color {
    val hsv = FloatArray(3)
    android.graphics.Color.colorToHSV(toArgb(), hsv)
    hsv[0] = (hsv[0] + degrees + 360f) % 360f
    hsv[1] = (hsv[1] * 0.9f).coerceIn(0f, 1f)
    return Color(android.graphics.Color.HSVToColor(hsv)).copy(alpha = alpha)
}

/**
 * Фон приложения: цвет окна и три размытых цветных пятна. Размытие применяется к самим пятнам,
 * поэтому оно доступно и без backdrop-эффектов; на Android ниже 12 хватает мягких радиальных
 * градиентов.
 */
@Composable
fun AuroraBackground(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val colors = Telegram.colors
    val glowAlpha = if (colors.night) 0.30f else 0.22f
    val blobs = remember(colors) {
        listOf(
            colors.accent.shiftHue(0f, glowAlpha),
            colors.accent.shiftHue(48f, glowAlpha * 0.85f),
            colors.accent.shiftHue(-58f, glowAlpha * 0.7f),
        )
    }
    Box(modifier.background(colors.window)) {
        Box(
            Modifier.fillMaxSize()
                .then(if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) Modifier.blur(72.dp) else Modifier)
                .drawBehind {
                    drawBlob(blobs[0], Offset(size.width * 0.12f, size.height * 0.08f), size.minDimension * 0.95f)
                    drawBlob(blobs[1], Offset(size.width * 0.95f, size.height * 0.34f), size.minDimension * 1.05f)
                    drawBlob(blobs[2], Offset(size.width * 0.30f, size.height * 0.92f), size.minDimension * 1.15f)
                },
        )
        content()
    }
}

private fun DrawScope.drawBlob(
    color: Color,
    center: Offset,
    radius: Float,
) {
    drawCircle(
        brush = Brush.radialGradient(
            listOf(color, color.copy(alpha = color.alpha * 0.35f), Color.Transparent),
            center = center,
            radius = radius,
        ),
        radius = radius,
        center = center,
    )
}

/**
 * Стеклянная поверхность: полупрозрачная заливка с бликом к верхнему краю и тонкая кромка.
 *
 * @param raised плотное стекло для поверхностей, лежащих выше остальных.
 */
fun Modifier.glass(
    palette: TuratPalette,
    shape: Shape,
    raised: Boolean = false,
    rim: Boolean = true,
): Modifier {
    val fill = if (raised) palette.glassRaised else palette.glass
    val sheen = if (palette.night) 0.07f else 0.34f
    return this
        .background(
            brush = Brush.verticalGradient(
                listOf(
                    Color.White.copy(alpha = sheen).compositeOverOpaque(fill),
                    fill,
                    palette.glassEdge.compositeOverOpaque(fill),
                ),
            ),
            shape = shape,
        )
        .then(
            if (!rim) Modifier
            else Modifier.border(
                width = 1.dp,
                brush = Brush.verticalGradient(
                    listOf(palette.glassRim, palette.glassRim.copy(alpha = palette.glassRim.alpha * 0.15f)),
                ),
                shape = shape,
            ),
        )
}

/** Готовая стеклянная панель со скруглением. */
@Composable
fun GlassPanel(
    modifier: Modifier = Modifier,
    radius: Dp = 22.dp,
    raised: Boolean = false,
    content: @Composable () -> Unit,
) {
    Box(modifier.glass(Telegram.colors, RoundedCornerShape(radius), raised)) { content() }
}

/**
 * Наложение цвета с сохранением прозрачности основы: смешиваем блик с заливкой стекла, но не
 * делаем её непрозрачной, иначе фон перестанет просвечивать.
 */
private fun Color.compositeOverOpaque(background: Color): Color {
    val a = alpha
    return Color(
        red = red * a + background.red * (1f - a),
        green = green * a + background.green * (1f - a),
        blue = blue * a + background.blue * (1f - a),
        alpha = background.alpha,
    )
}

/** Скругления Liquid Glass: крупные и концентричные. */
object GlassShape {
    val Panel = RoundedCornerShape(24.dp)
    val Card = RoundedCornerShape(20.dp)
    val Capsule = RoundedCornerShape(percent = 50)

    /** Шапка примыкает к верхнему краю экрана, поэтому скруглены только нижние углы. */
    val Header = RoundedCornerShape(bottomStart = 26.dp, bottomEnd = 26.dp)

    /** Композер и панели, примыкающие к нижнему краю. */
    val Footer = RoundedCornerShape(topStart = 26.dp, topEnd = 26.dp)

    val Sheet = RoundedCornerShape(topEnd = 28.dp, bottomEnd = 28.dp)
}
