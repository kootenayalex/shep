package dev.shep.companion.ui.components

import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import dev.shep.companion.ui.theme.ShepMotion
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepType
import dev.shep.companion.ui.theme.StateAppearance
import kotlinx.coroutines.delay
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween

/**
 * One agent's state, as the mark the desktop draws for it.
 *
 * This replaced four separate filled circles that differed only in hue, which
 * meant the single most important fact on the board was carried by colour alone
 * — invisible to a colour-blind reader and to anyone glancing at a phone in
 * sunlight. Shape now carries the same story: `◉` stopped, spinner moving,
 * `●` finished, `○` settled, `·` nothing known.
 *
 * The glyph also announces itself, which the coloured dot never did.
 *
 * [style] is a token, not a size: [ShepType.stateGlyph] on a card, and
 * [ShepType.stateGlyphSmall] one level in — a tab or pane in the spaces tree,
 * or a pane's own title bar. Two steps are enough to carry the hierarchy; the
 * tree used to ask for four different diameters.
 */
@Composable
fun StateGlyph(
    status: String,
    modifier: Modifier = Modifier,
    style: TextStyle = ShepType.stateGlyph,
) {
    Glyph(
        // Only a working agent needs a ticking clock, so only a working agent
        // gets one — an idle board does no work at all.
        appearance = if (status == "working") {
            val tick by rememberSpinnerTick()
            ShepSemantic.agent(status, tick)
        } else {
            ShepSemantic.agent(status)
        },
        modifier = modifier,
        style = style,
    )
}

/**
 * The same, for a queued task rather than a running agent.
 *
 * Separate from [StateGlyph] because the vocabularies are separate: a task goes
 * `todo → running → done`, and "done" there means settled rather than the
 * agent board's "finished, and you have not looked yet".
 */
@Composable
fun TaskGlyph(
    state: String,
    modifier: Modifier = Modifier,
    style: TextStyle = ShepType.stateGlyph,
) {
    Glyph(
        appearance = if (state == "running") {
            val tick by rememberSpinnerTick()
            ShepSemantic.task(state, tick)
        } else {
            ShepSemantic.task(state)
        },
        modifier = modifier,
        style = style,
    )
}

@Composable
private fun Glyph(appearance: StateAppearance, modifier: Modifier, style: TextStyle) {
    // The colour eases; the glyph does not. A shape that morphs is a
    // distraction, but a shape that changes while its colour slides tells you
    // *which* card just moved on a board where six of them look alike — which
    // is the whole reason to animate anything on a screen you glance at.
    val ink by animateColorAsState(
        targetValue = appearance.color,
        animationSpec = tween(ShepMotion.QUICK_MS),
        label = "state-ink",
    )
    Text(
        appearance.glyph,
        style = style.copy(color = ink),
        textAlign = TextAlign.Center,
        modifier = modifier.semantics { contentDescription = appearance.description },
    )
}

/**
 * One frame every [ShepMotion.SPINNER_FRAME_MS].
 *
 * `spinnerFrame` divides by eight, so stepping by eight advances exactly one
 * frame.
 */
@Composable
private fun rememberSpinnerTick(): State<Int> = produceState(0) {
    while (true) {
        delay(ShepMotion.SPINNER_FRAME_MS)
        value += 8
    }
}
