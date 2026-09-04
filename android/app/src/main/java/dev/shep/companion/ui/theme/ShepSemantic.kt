package dev.shep.companion.ui.theme

import androidx.compose.ui.graphics.Color

/**
 * What a state looks like, and what to call it out loud.
 *
 * This is the phone's copy of the table in `docs/DESIGN-LANGUAGE.md`; the
 * desktop's copy is `state_appearance` in `src/ui/status.rs`. They are pinned
 * by matching tests on both sides, because the two surfaces are meant to read
 * as one product and a colour that means different things on each is the bug
 * that keeps coming back.
 *
 * [description] exists because [glyph] and [color] are both invisible to a
 * screen reader — the state was previously carried by a coloured dot with no
 * text alternative at all.
 */
data class StateAppearance(
    val glyph: String,
    val label: String,
    val color: Color,
    val description: String,
)

/**
 * Working, as a filling circle.
 *
 * The desktop spins braille, and this deliberately does not. Two reasons, and
 * the first is not cosmetic: a terminal cell must be exactly one column, and
 * `◐`/`◑` are East-Asian-Ambiguous while `◓`/`◒` are Neutral — on a terminal
 * configured for wide ambiguous glyphs that set would change width every frame
 * and shift the whole row. Braille is uniformly Neutral, so it is the correct
 * choice there. A phone has no column grid, so the constraint does not apply.
 *
 * The second reason is that braille loses at phone sizes. At 13sp those dots
 * render as a scatter of specks beside a solid `●`, which made the one mark
 * that says "this is alive" the faintest thing on the card. A half-filled
 * circle carries the same optical weight as `●` and `○`, so the five states
 * finally read as one family: ring, filling, full, empty, speck.
 */
private val SPINNER = listOf("◐", "◓", "◑", "◒")

/** The frame for an animation tick. Divides by 8, matching the desktop's tick math. */
fun spinnerFrame(tick: Int): String = SPINNER[(tick / 8).mod(SPINNER.size)]

object ShepSemantic {

    /**
     * The agent-state vocabulary.
     *
     * [status] is the label the server sends (`blocked` / `working` / `done` /
     * `idle`), which is already the desktop's own `state_label` output — so the
     * `seen` split that produces "done" versus "idle" has happened server-side
     * and does not need repeating here.
     *
     * Every state has its own glyph. Three of them used to be one filled dot in
     * three colours, which told a colour-blind reader nothing and gave a
     * monochrome notification icon nothing to draw.
     */
    fun agent(status: String, tick: Int = 0): StateAppearance = when (status) {
        "blocked" -> StateAppearance(
            glyph = "◉",
            label = "blocked",
            color = ShepPalette.red,
            description = "blocked, waiting for you",
        )
        "working" -> StateAppearance(
            glyph = spinnerFrame(tick),
            label = "working",
            // Yellow, not copper: copper is focus and selection, and a working
            // agent must not share ink with the row you happen to have selected.
            color = ShepPalette.yellow,
            description = "working",
        )
        "done" -> StateAppearance(
            glyph = "●",
            label = "done",
            color = ShepPalette.blue,
            description = "done, not yet seen",
        )
        "idle" -> StateAppearance(
            glyph = "○",
            label = "idle",
            color = ShepPalette.green,
            description = "idle",
        )
        else -> StateAppearance(
            glyph = "·",
            label = status.ifBlank { "idle" },
            color = ShepPalette.overlay0,
            description = "state unknown",
        )
    }

    /** Just the ink, for the many places that colour a label rather than draw a glyph. */
    fun agentColor(status: String): Color = agent(status).color

    /**
     * Every tier a manual state can name, matching `ManualStateTier::ALL` in
     * src/api/schema/common.rs and `manual_state_appearance` in src/ui/status.rs.
     */
    val MANUAL_TIERS = listOf("stop", "working", "done", "settled", "waiting", "absent", "review")

    /**
     * A state someone set by hand.
     *
     * The tier picks the same ink and shape a detected state of that family
     * would get, and the trailing `·` says "somebody put this here" — the same
     * mark the desktop sidebar draws, so a row that reads `◉·` on the phone
     * reads `◉·` at the desk. The label is the configured one, not the tier
     * name, because "in review" is what the person typed and "review" is not.
     * An unknown tier renders as absent rather than crashing the row: a newer
     * server may know a tier this build does not.
     */
    fun manual(tier: String, label: String, tick: Int = 0): StateAppearance = when (tier) {
        "stop" -> StateAppearance("◉·", label, ShepPalette.red, "$label, set by hand")
        "working" -> StateAppearance(spinnerFrame(tick) + "·", label, ShepPalette.yellow, "$label, set by hand")
        "done" -> StateAppearance("●·", label, ShepPalette.blue, "$label, set by hand")
        "settled" -> StateAppearance("○·", label, ShepPalette.green, "$label, set by hand")
        "waiting" -> StateAppearance("○·", label, ShepPalette.overlay1, "$label, set by hand")
        "review" -> StateAppearance("◆·", label, ShepPalette.mauve, "$label, set by hand")
        else -> StateAppearance("··", label, ShepPalette.overlay0, "$label, set by hand")
    }

    /**
     * Task-queue states, matching `task_appearance` in `src/ui/status.rs`.
     *
     * The shapes tell the same story as an agent's, because they mean the same
     * things: a ring is stopped, movement is working, filled is finished,
     * hollow is waiting, a speck is nothing. Only "done" takes a different
     * colour — green rather than blue — because a task has no notion of your
     * having seen it, so it is simply settled.
     *
     * A queued task is dimmer than a running one on purpose: the queue is a
     * backlog, and the eye should land on what is actually moving.
     */
    fun task(state: String, tick: Int = 0): StateAppearance = when (state) {
        "blocked" -> StateAppearance(
            glyph = "◉",
            label = "blocked",
            color = ShepPalette.red,
            description = "task blocked",
        )
        "running" -> StateAppearance(
            glyph = spinnerFrame(tick),
            label = "running",
            color = ShepPalette.yellow,
            description = "task running",
        )
        "done" -> StateAppearance(
            glyph = "●",
            label = "done",
            color = ShepPalette.green,
            description = "task done",
        )
        "cancelled" -> StateAppearance(
            glyph = "·",
            label = "cancelled",
            color = ShepPalette.overlay0,
            description = "task cancelled",
        )
        else -> StateAppearance(
            glyph = "○",
            label = state.ifBlank { "todo" },
            color = ShepPalette.overlay1,
            description = "task waiting to start",
        )
    }

    /** Just the ink, for the label beside the glyph. */
    fun taskColor(state: String): Color = task(state).color

    /**
     * The review-lifecycle badge, or `null` when there is nothing to say.
     *
     * `◆` is mauve rather than yellow because yellow is the working tier, and
     * `✓` belongs to "approved" alone — it used to be idle's glyph too, which
     * made both ambiguous.
     */
    fun reviewBadge(state: String?): Pair<String, Color>? = when (state) {
        "needs_review" -> "◆" to ShepPalette.mauve
        "changes_requested" -> "↺" to ShepPalette.peach
        "approved" -> "✓" to ShepPalette.green
        else -> null
    }
}
