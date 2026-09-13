package dev.shep.companion.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import dev.shep.companion.SessionTotals
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/*
 * The desktop's chrome, on a phone.
 *
 * `src/ui/chrome.rs::titlebar_layout` puts three things in one row at the desk:
 * the session's name in the centre, the state tally and the `desktop | board`
 * pill on the right. The phone already has the name in its `ScreenHeader`, so
 * what moves across is the tally and the pill — and they move as a pair,
 * because the tally is *what is happening* and the pill is *where to look at
 * it*, and they were the two facts the phone's old words-and-counts strip said
 * least well.
 *
 * What is deliberately not here: the load ladders. `DashboardStrip` used to
 * warm `load 82%` yellow and `mem 91%` red, which spent the working and stop
 * tiers on a host vital — so a busy laptop shouted louder than a blocked
 * agent. Those vitals now sit dim in the board's header row, which is where
 * the desktop keeps them too (`header_facts`, src/ui/overseer.rs:146).
 */

/**
 * The state tally: `◉ 2  ◐ 1  ● 3  ○ 4`, in the desktop's order
 * (`right_run`, src/ui/chrome.rs:212-264, and `docs/DESIGN-LANGUAGE.md:207`).
 *
 * Zero counts are absent rather than drawn as a zero — `blocked 0 · done 0`
 * was three quarters of the old strip on a quiet session, and it pushed the
 * states that *were* happening off the right edge. Blocked is bold as well as
 * red, because it is the only one of the four that is a call to action.
 *
 * One `AnnotatedString` rather than a `Row` of `Text`s: this sits in a fixed
 * slot beside the pill, and eight separately-measured children wrap at a large
 * font scale where one string simply ellipsises.
 */
@Composable
fun StateTally(totals: SessionTotals, tick: Int = 0): Unit = Text(
    tallyText(totals, tick),
    style = ShepType.metaSmall,
    maxLines = 1,
    overflow = TextOverflow.Ellipsis,
)

/** The tally's text, so the glyph-and-ink table is asked once per state. */
private fun tallyText(totals: SessionTotals, tick: Int): AnnotatedString = buildAnnotatedString {
    val counts = mapOf(
        "blocked" to totals.blocked,
        "working" to totals.working,
        "done" to totals.done,
        "idle" to totals.idle,
    )
    ShepSemantic.TALLY_ORDER.forEach { status ->
        val count = counts[status] ?: 0
        if (count == 0) return@forEach
        if (length > 0) append("  ")
        val look = ShepSemantic.agent(status, tick)
        withStyle(
            SpanStyle(
                color = look.color,
                fontWeight = if (status == "blocked") FontWeight.Bold else FontWeight.Normal,
            )
        ) {
            append("${look.glyph} $count")
        }
    }
}

/** Which half of the [ViewPill] is lit — which of the two views you are on. */
enum class PillHalf(val label: String) {
    Desktop("desktop"),
    Board("board"),
}

/**
 * The `desktop | board` pill, the one switch between the session and the
 * overseer's board.
 *
 * `docs/DESIGN-LANGUAGE.md:216` — the one place `accent` paints a background.
 * The lit half is panel-bg on accent, bold; the unlit half is subtext on
 * surface1. Focus tier, because the pill is a selection between two views and
 * not a state: a lit `board` and a working agent must not share ink.
 *
 * This is two halves of one chip, not a new colour rule — [ShepChip] already
 * paints its selected state in accent (`Interactive.kt:143`), and the pill
 * differs only in being a single stadium cut in two so the pair reads as one
 * control rather than as two buttons that happen to be adjacent.
 *
 * [proposals] rides on the unlit `board` half as ` N` in teal — the queued
 * tier, because a proposal is waiting for you and nothing has happened yet.
 */
@Composable
fun ViewPill(current: PillHalf, proposals: Int, onSelect: (PillHalf) -> Unit) {
    Row(Modifier.clip(ShepShape.pill), verticalAlignment = Alignment.CenterVertically) {
        PillHalf.entries.forEach { half ->
            val lit = half == current
            PillHalfBox(
                half = half,
                lit = lit,
                proposals = if (!lit && half == PillHalf.Board) proposals else 0,
                onClick = { onSelect(half) },
            )
        }
    }
}

@Composable
private fun PillHalfBox(half: PillHalf, lit: Boolean, proposals: Int, onClick: () -> Unit) {
    Row(
        Modifier
            .minimumInteractiveComponentSize()
            .background(if (lit) ShepPalette.accent else ShepPalette.surface1)
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            half.label,
            style = ShepType.chip.copy(
                color = if (lit) ShepPalette.panelBg else ShepPalette.subtext0,
                fontWeight = if (lit) FontWeight.Bold else FontWeight.Normal,
            ),
        )
        if (proposals > 0) {
            Text(
                " $proposals",
                style = ShepType.badge.copy(color = ShepPalette.teal),
            )
        }
    }
}

/**
 * The chrome row: the tally on the left, the pill pinned right.
 *
 * Sits directly under the `ScreenHeader` on both views it switches between, so
 * the control that moves you between them never moves itself.
 */
@Composable
fun ChromeRow(
    totals: SessionTotals,
    proposals: Int,
    current: PillHalf,
    onSelect: (PillHalf) -> Unit,
) {
    val tick by rememberSpinnerTick()
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        StateTally(totals, tick)
        Spacer(Modifier.weight(1f))
        ViewPill(current, proposals, onSelect)
    }
}

/**
 * The overseer's one row outside the board: `✦ overseer · <first sentence> ·
 * hh:mm`, on surface0, the whole row opening the board.
 *
 * `docs/DESIGN-LANGUAGE.md:242` — one row and no buttons. The desktop's
 * equivalent is `render_overseer_strip` (src/ui/overseer.rs); both hide
 * themselves entirely when the overseer has not spoken, because a strip that
 * says nothing is a row of chrome charging rent.
 */
@Composable
fun OverseerStrip(firstSentence: String, tickAt: String?, onOpen: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surface0)
            .minimumInteractiveComponentSize()
            .clickable(onClick = onOpen)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            "${ShepSemantic.overseer.glyph} overseer",
            style = ShepType.metaSmall.copy(
                color = ShepSemantic.overseer.color,
                fontWeight = FontWeight.Bold,
            ),
        )
        Text(" · ", style = ShepType.metaSmall)
        Text(
            firstSentence,
            style = ShepType.metaSmall.copy(color = ShepPalette.subtext0),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        tickAt?.let {
            Spacer(Modifier.width(ShepSpace.small))
            Text(it, style = ShepType.metaSmall)
        }
    }
}
