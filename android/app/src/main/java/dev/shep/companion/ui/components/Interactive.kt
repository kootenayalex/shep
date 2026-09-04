package dev.shep.companion.ui.components

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.unit.Dp
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/*
 * Things you can tap.
 *
 * Two rules the hand-rolled versions kept getting wrong, in eight places each:
 *
 *  1. **Clip before clickable.** `.background(shape).clickable{}` draws a
 *     rectangular ripple over rounded chrome, so every terminal key, both mode
 *     chips and the queue/send buttons flashed square corners on touch.
 *  2. **Padding inside the touch target.** `.clickable{}.padding(...)` puts the
 *     padding *outside* the ripple and outside the hit area, so "remove",
 *     "edit", "history", "review" and the pane back arrow were only as tall as
 *     their 13sp text — about 18dp against a 48dp guideline.
 *
 * Everything below is minimum-size → clip → background → clickable → padding,
 * in that order, so both are structural rather than remembered.
 *
 * `minimumInteractiveComponentSize` rather than a `defaultMinSize`: it reports
 * 48dp to the parent and intercepts out-of-bounds pointer events, but places
 * its child at the child's own size. So the target is 48dp and the *pill* stays
 * the size of its word. Forcing the visual to 48dp instead turned every chip
 * into a near-circle and made the spaces tree twice as tall as it needs to be —
 * paying for reach with density is not the trade to make when the framework
 * offers both.
 */

/**
 * A word that does something: "remove", "go to", "split", "clear done".
 *
 * The smallest affordance in the app, and the one that was least tappable.
 * [ShepSize.touchTarget] is enforced as a minimum *height* only — a minimum
 * width would put 48dp of dead space between two adjacent actions in the
 * spaces tree, where five of them share a row.
 */
@Composable
fun ActionText(
    text: String,
    modifier: Modifier = Modifier,
    style: TextStyle = ShepType.actionQuiet,
    description: String? = null,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .minimumInteractiveComponentSize()
            .clip(ShepShape.button)
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.snug, vertical = ShepSpace.tight),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            style = style,
            modifier = if (description == null) {
                Modifier
            } else {
                Modifier.semantics { contentDescription = description }
            },
        )
    }
}

/**
 * A filled button. [tone] is the whole of its meaning:
 * copper for the thing you probably want, surface for the alternative, red for
 * the one that destroys something.
 */
enum class ButtonTone { Primary, Quiet, Danger }

@Composable
fun ShepButton(
    text: String,
    modifier: Modifier = Modifier,
    tone: ButtonTone = ButtonTone.Primary,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    val background = when {
        !enabled -> ShepPalette.surface0
        tone == ButtonTone.Primary -> ShepPalette.accent
        else -> ShepPalette.surface0
    }
    val ink = when {
        !enabled -> ShepPalette.overlay0
        tone == ButtonTone.Primary -> ShepPalette.panelBg
        tone == ButtonTone.Danger -> ShepPalette.red
        else -> ShepPalette.subtext0
    }
    Box(
        modifier
            .clip(ShepShape.button)
            .background(background)
            .clickable(enabled = enabled, onClick = onClick)
            .defaultMinSize(minHeight = ShepSize.touchTarget)
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.small),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = ShepType.action.copy(color = ink))
    }
}

/** The app's one chip shape. Selected means copper; nothing else changes. */
@Composable
fun ShepChip(
    text: String,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .minimumInteractiveComponentSize()
            .clip(ShepShape.pill)
            .background(if (selected) ShepPalette.accent else ShepPalette.surface0)
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            style = ShepType.chip.copy(
                color = if (selected) ShepPalette.panelBg else ShepPalette.subtext0,
                fontWeight = if (selected) {
                    androidx.compose.ui.text.font.FontWeight.SemiBold
                } else {
                    androidx.compose.ui.text.font.FontWeight.Normal
                },
            ),
        )
    }
}

/**
 * A card in a list.
 *
 * Every card in the app is [ShepPalette.surface0] on [ShepPalette.panelBg].
 * Tasks and memory used to be `surfaceDim` — the same colour as the header
 * directly above them — so those two lists had no separation from their own
 * chrome.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ShepCard(
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
    background: Color = ShepPalette.surface0,
    shape: Shape = ShepShape.card,
    padding: Dp = ShepSpace.card,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(background)
            .then(
                when {
                    onClick == null && onLongClick == null -> Modifier
                    else -> Modifier.combinedClickable(
                        onLongClick = onLongClick,
                        onClick = onClick ?: {},
                    )
                }
            )
            .padding(padding),
        content = content,
    )
}
