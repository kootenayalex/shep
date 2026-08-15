package dev.shep.companion.ui.components

import androidx.compose.runtime.Composable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import androidx.compose.material3.Text
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.delay

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
 */
@Composable
fun StateGlyph(
    status: String,
    modifier: Modifier = Modifier,
    fontSize: TextUnit = 13.sp,
) {
    // Only a working agent needs a ticking clock, so only a working agent gets
    // one — an idle board does no work at all.
    val appearance = if (status == "working") {
        val tick by rememberSpinnerTick()
        ShepSemantic.agent(status, tick)
    } else {
        ShepSemantic.agent(status)
    }
    Text(
        appearance.glyph,
        style = ShepType.badge.copy(color = appearance.color, fontSize = fontSize),
        textAlign = TextAlign.Center,
        modifier = modifier.semantics { contentDescription = appearance.description },
    )
}

/**
 * One frame every 180ms — about two-thirds of a second per turn.
 *
 * `spinnerFrame` divides by eight, so stepping by eight advances exactly one
 * frame. The desktop runs ten braille frames at 125ms; four half-circles at
 * that rate would spin twice a second, which reads as agitation rather than
 * as work getting done.
 */
@Composable
private fun rememberSpinnerTick(): State<Int> = produceState(0) {
    while (true) {
        delay(180)
        value += 8
    }
}
