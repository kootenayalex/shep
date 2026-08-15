package dev.shep.companion.ui.theme

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.unit.dp

/**
 * How far apart things sit.
 *
 * Seven steps on a 2dp base, named for what they separate rather than for how
 * big they are — a token called `md` tells you nothing at the call site, and
 * that is how the app ended up with 296 inline `.dp` literals across 23
 * distinct values, several of which differed by a single pixel for no reason.
 *
 * Two rules keep this honest:
 *  - **A container's own padding, not a shared column.** A screen header is
 *    full-bleed and pads itself by [screen]; a card is inset by [listGutter]
 *    and pads itself by [card]. They are different containers, so their text
 *    does not share a left edge, and that is correct.
 *  - **Pick the nearest step, never a new one.** If nothing here fits, the
 *    layout is probably wrong before the spacing is.
 */
object ShepSpace {
    /** No gap. Named so a component can be told to add none. */
    val none = 0.dp

    /** Two facts on one line-run; the gap under a heading before its subtitle. */
    val hair = 2.dp

    /** Between stacked lines inside one card. */
    val tight = 4.dp

    /** A pill's vertical padding; a bar's own height. */
    val snug = 6.dp

    /** Glyph to the word it belongs to; one card to the next. */
    val small = 8.dp

    /** Block to block inside a sheet; a list's gutter; a pill's side padding. */
    val medium = 12.dp

    /** A screen header's padding; a sheet's padding. */
    val screen = 16.dp

    /** Between one section of a settings screen and the next. */
    val section = 24.dp

    /** A card's inner padding. One step in from [listGutter], which insets it. */
    val card = 12.dp

    /** Between the edge of the screen and the cards in a list. */
    val listGutter = 12.dp

    /** Each level of the spaces tree steps in by this much. */
    val indent = 20.dp
}

/**
 * Fixed sizes that are not spacing: things with an intrinsic dimension.
 */
object ShepSize {
    /**
     * The smallest thing a finger may be asked to hit.
     *
     * Android's own floor, and the app was well under it — "remove", "edit" and
     * the pane back arrow were only as tall as their 13sp text.
     */
    val touchTarget = 48.dp

    /** The context-window bar on a board card. */
    val gaugeWidth = 44.dp
    val gaugeHeight = 4.dp

    /** The memory-cap bar, which spans its screen. */
    val meterHeight = 6.dp

    /** A full-width primary button. */
    val buttonHeight = 52.dp

    /** A hairline border. */
    val border = 1.dp

    /** The ring around a focused terminal, which has to read at a glance. */
    val focusRing = 2.dp

    /**
     * At or above this, the board and a pane sit side by side.
     *
     * Compared against the *smallest* width the window can have, not the
     * current one — a phone turned landscape is still a phone, and comparing
     * `maxWidth` put it in the tablet layout with a 200dp-wide board.
     */
    val twoPaneWidth = 720.dp
}

/**
 * Corner radii, as shapes rather than numbers so a call site cannot invent one.
 *
 * The ladder is the hierarchy: the smaller the thing, the tighter its corner.
 * [pill] is a percentage so a chip is a true stadium at any height — the app
 * previously approximated it with 14dp, 16dp and 999dp in three different files.
 */
object ShepShape {
    /** A gauge or meter fill. */
    val bar = RoundedCornerShape(2.dp)

    /** A key on the terminal key bar. */
    val key = RoundedCornerShape(6.dp)

    /** A button, and anything else that reads as pressable and rectangular. */
    val button = RoundedCornerShape(8.dp)

    /** A text field, a tool row, a nested row inside a sheet. */
    val field = RoundedCornerShape(10.dp)

    /** A card in a list. */
    val card = RoundedCornerShape(12.dp)

    /** A sheet, and the ring around a focused terminal. */
    val sheet = RoundedCornerShape(16.dp)

    /** A chip. Always a stadium, whatever the text does to its height. */
    val pill = RoundedCornerShape(percent = 50)
}

/**
 * How long things take.
 *
 * Motion here has one job: make a change that happened *elsewhere* legible.
 * This is a companion to a screen you are not looking at, so a card that
 * silently swaps colour or teleports up the list is a change you missed. It is
 * not decoration, and the desktop deliberately has none of it — a TUI over
 * mosh cannot afford to repaint for the sake of feeling nice.
 */
object ShepMotion {
    /**
     * One spinner frame. Four half-circles at 180ms is about two-thirds of a
     * second per turn, which reads as work getting done; the desktop's ten
     * braille frames at 125ms come to the same cadence.
     */
    const val SPINNER_FRAME_MS = 180L

    /** A colour settling into its new tier. Long enough to catch, short
     *  enough that a board of six agents changing at once is not a light show. */
    const val QUICK_MS = 220

    /** A meter filling, a card moving up the list. */
    const val STANDARD_MS = 320

    /** A tab arriving. */
    const val ENTER_MS = 180

    /**
     * How long a notice stays before it clears itself.
     *
     * Three of the six hand-rolled notices could not be dismissed at all, so
     * they sat on screen until the next one replaced them — "renamed to x"
     * still showing ten minutes later is a lie about what is happening now.
     */
    const val NOTICE_MS = 4000L
}

/*
 * On elevation: there is none, deliberately.
 *
 * Depth here is a surface ladder, not a shadow — `rootBg` behind `panelBg`
 * behind `surfaceDim` behind `surface0` behind `surface1`. Those are already
 * named tokens in `ShepPalette`, and a terminal has no shadows either. An
 * elevation scale would be a second, contradictory way to say the same thing.
 */
