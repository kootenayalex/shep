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

    /**
     * The docket vocabulary, from the docket table in `docs/DESIGN-LANGUAGE.md`
     * (desktop: `docket_appearance` in src/ui/status.rs). A docket item is a
     * reminder, not a process, so it borrows the task table's shapes — hollow
     * for undecided, filled for live, green when settled — and adds one mark,
     * `!`, so an overdue item is never told apart by colour alone. Peach rather
     * than red because red is what a blocked agent gets and an item three days
     * late is a nag, not a stop.
     *
     * [status] is the wire label (`inbox` / `open` / `done` / `discarded`).
     */
    fun docket(status: String, overdue: Boolean = false, dueToday: Boolean = false): StateAppearance =
        when {
            status == "inbox" -> StateAppearance("○", "inbox", ShepPalette.overlay1, "in the inbox, undecided")
            status == "open" && overdue -> StateAppearance("!", "overdue", ShepPalette.peach, "open and overdue")
            status == "open" && dueToday -> StateAppearance("●", "due today", ShepPalette.yellow, "open, due today")
            status == "open" -> StateAppearance("●", "open", ShepPalette.overlay1, "open")
            status == "done" -> StateAppearance("●", "done", ShepPalette.green, "done")
            status == "discarded" -> StateAppearance("·", "discarded", ShepPalette.overlay0, "discarded")
            else -> StateAppearance("·", status.ifBlank { "inbox" }, ShepPalette.overlay0, "state unknown")
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
     * The overseer's own mark, wherever the overseer speaks: the strip on the
     * agents list, the board's `✦ read of the room` heading, and every line it
     * says in the chat.
     *
     * `✦` and mauve, from `docs/DESIGN-LANGUAGE.md:175-178` (desktop:
     * `glyphs::MARKER` in src/ui/glyphs.rs, drawn by src/ui/overseer.rs).
     * Deliberately not `◆`, which is the needs-review badge — one glyph
     * carries one meaning, and the two would otherwise sit on the same screen
     * saying different things.
     */
    val overseer = StateAppearance(
        glyph = "✦",
        label = "overseer",
        color = ShepPalette.mauve,
        description = "the overseer",
    )

    /**
     * One health finding, from `docs/DESIGN-LANGUAGE.md:175-178` and the
     * desktop's `health_facts` (src/ui/overseer.rs:1289).
     *
     * A warning is peach and never red: red is what a blocked agent gets, and
     * a disk at 90% is a nag. A *failed* check does stop you the way a blocked
     * agent does, so it borrows the stop tier whole — glyph and ink both.
     *
     * [level] is the wire spelling (`ok` / `warn` / `fail`), as everything else
     * in this object takes its wire word rather than an enum.
     */
    fun health(level: String): StateAppearance = when (level) {
        "ok" -> StateAppearance("✓", "ok", ShepPalette.green, "healthy")
        "warn" -> StateAppearance("⚠", "warn", ShepPalette.peach, "a warning")
        "fail" -> StateAppearance("◉", "fail", ShepPalette.red, "failing")
        else -> StateAppearance("·", level.ifBlank { "ok" }, ShepPalette.overlay0, "unknown check")
    }

    /**
     * The context gauge's ink: peach at or over [GAUGE_WARM_PERCENT], overlay0
     * below it. Never anything hotter.
     *
     * A gauge is a meter, not a state. It used to warm through yellow at 60 to
     * red at 85, which spent the working and the stop tier on a number — so a
     * full context window shouted louder than a blocked agent. The desktop
     * fixed this in `gauge_color` (src/ui/gauge.rs:40-46) with one threshold,
     * and `docs/DESIGN-LANGUAGE.md:170-173` calls the old ladder the mistake.
     */
    fun gauge(percent: Int): Color =
        if (percent >= GAUGE_WARM_PERCENT) ShepPalette.peach else ShepPalette.overlay0

    /** Where the gauge warms. The desktop's `WARM_PERCENT` (src/ui/gauge.rs:35). */
    const val GAUGE_WARM_PERCENT = 80

    /**
     * The order the state tally reads in, from the titlebar's right slot
     * (`right_run` in src/ui/chrome.rs:212-264, and
     * `docs/DESIGN-LANGUAGE.md:207-214`): what stopped, what is moving, what
     * finished, what is settled. Urgency first, so a glance that only reaches
     * the first fact reaches the right one.
     */
    val TALLY_ORDER = listOf("blocked", "working", "done", "idle")

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
