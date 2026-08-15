package dev.shep.companion.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextAlign
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import dev.shep.companion.ui.theme.ShepMotion
import kotlinx.coroutines.delay

/**
 * The one line at the top of a tab.
 *
 * Six screens each built their own, and they disagreed about the status text's
 * colour, the gap before the actions, and whether the title had a subtitle
 * slot at all.
 *
 * [actions] is a `RowScope`, so a screen adds its affordances without also
 * having to remember the background, the padding, or that the title comes
 * first — the three things that made six copies drift.
 */
@Composable
fun ScreenHeader(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Row(
        modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(title, style = ShepType.screenTitle)
        subtitle?.let {
            Spacer(Modifier.width(ShepSpace.small))
            Text(it, style = ShepType.meta)
        }
        Spacer(Modifier.weight(1f))
        actions()
    }
}

/** How loud a [Notice] is. */
enum class NoticeTone {
    /** Something happened and you may want to know. The common case. */
    Info,

    /** Something is wrong enough to interrupt: a protocol mismatch, a failure. */
    Alert,
}

/**
 * A one-line banner under a header.
 *
 * Six hand-rolled versions existed, three of which could not be dismissed and
 * so simply stayed on screen until the next one replaced them. Tapping now
 * always clears it when the caller can.
 */
@Composable
fun Notice(
    text: String,
    modifier: Modifier = Modifier,
    tone: NoticeTone = NoticeTone.Info,
    onDismiss: (() -> Unit)? = null,
) {
    // An Info notice is a receipt — "renamed to x", "queued task" — and a
    // receipt has a shelf life. It clears itself after
    // [ShepMotion.NOTICE_MS] so the screen stops asserting something that
    // stopped being true minutes ago. An Alert does not: a protocol mismatch
    // is a standing condition, not an event.
    if (onDismiss != null && tone == NoticeTone.Info) {
        LaunchedEffect(text) {
            delay(ShepMotion.NOTICE_MS)
            onDismiss()
        }
    }
    val background = if (tone == NoticeTone.Alert) ShepPalette.peach else Color.Transparent
    val ink = if (tone == NoticeTone.Alert) ShepPalette.panelBg else ShepPalette.peach
    Text(
        text,
        style = ShepType.meta.copy(color = ink),
        modifier = modifier
            .fillMaxWidth()
            .background(background)
            .then(if (onDismiss == null) Modifier else Modifier.clickable { onDismiss() })
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.snug),
    )
}

/**
 * What a screen says when it has nothing to show.
 *
 * Nine of these existed and every one was a bare `Text` with no style, which
 * resolved to Material's `bodyLarge` — Roboto 16. They were the loudest
 * non-shep typography in the app, on the screens with the least to say.
 */
@Composable
fun EmptyState(text: String, modifier: Modifier = Modifier) {
    Box(
        modifier.fillMaxSize().padding(ShepSpace.section),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = ShepType.emptyState, textAlign = TextAlign.Center)
    }
}

/** The same, while shep is still finding out. */
@Composable
fun LoadingState(label: String, detail: String? = null, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            CircularProgressIndicator(color = ShepPalette.accent)
            Text(label, style = ShepType.emptyState)
            detail?.let { Text(it, style = ShepType.meta.copy(color = ShepPalette.peach)) }
        }
    }
}

/**
 * A horizontal bar showing how full something is.
 *
 * Two boxes rather than a `LinearProgressIndicator`: Material's draws a stop
 * indicator at the track end, so an empty meter showed a mark at 100% and read
 * as full. [width] is null for a bar that spans its parent.
 */
@Composable
fun Meter(
    fraction: Float,
    color: Color,
    modifier: Modifier = Modifier,
    height: androidx.compose.ui.unit.Dp = ShepSize.meterHeight,
) {
    // Animated because a meter that jumps reads as a redraw, and one that
    // fills reads as a measurement — and these two both report a number that
    // matters exactly when it is climbing.
    val filled by animateFloatAsState(
        targetValue = fraction.coerceIn(0f, 1f),
        animationSpec = tween(ShepMotion.STANDARD_MS),
        label = "meter",
    )
    val ink by animateColorAsState(
        targetValue = color,
        animationSpec = tween(ShepMotion.QUICK_MS),
        label = "meter-ink",
    )
    Box(
        modifier
            .height(height)
            .clip(ShepShape.bar)
            .background(ShepPalette.surface1),
    ) {
        Box(
            Modifier
                .fillMaxWidth(filled)
                .height(height)
                .clip(ShepShape.bar)
                .background(ink),
        )
    }
}

/**
 * A bottom sheet with shep's chrome already on it.
 *
 * Every sheet in the app repeated the container colour, the padding, the IME
 * padding and a bold title, and one of the six forgot `imePadding` so its
 * field sat under the keyboard.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShepSheet(
    title: String,
    onDismiss: () -> Unit,
    modifier: Modifier = Modifier,
    titleAction: @Composable RowScope.() -> Unit = {},
    content: @Composable ColumnScope.() -> Unit,
) {
    val sheetState = rememberModalBottomSheetState()
    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = ShepPalette.surfaceDim,
    ) {
        Column(
            modifier
                .fillMaxWidth()
                .padding(horizontal = ShepSpace.screen)
                .padding(bottom = ShepSpace.screen)
                .imePadding(),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, style = ShepType.sheetTitle)
                Spacer(Modifier.weight(1f))
                titleAction()
            }
            Spacer(Modifier.height(ShepSpace.medium))
            content()
        }
    }
}
