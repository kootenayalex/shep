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
 * Braille spinner frames, matching `SPINNERS` in `src/ui.rs`.
 *
 * The same rotation on both surfaces means a working agent looks like the same
 * agent whichever screen you are looking at.
 */
private val SPINNER = listOf("⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏")

/** The frame for an animation tick. Divides by 8 for ~8 updates/sec, as the desktop does. */
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
     * Task-queue states, matching `task_state_color` in `src/ui/board.rs`.
     *
     * A queued task is dimmer than a running one on purpose: the queue is a
     * backlog, and the eye should land on what is actually moving.
     */
    fun task(state: String): Color = when (state) {
        "blocked" -> ShepPalette.red
        "running" -> ShepPalette.yellow
        "done" -> ShepPalette.green
        "cancelled" -> ShepPalette.overlay0
        else -> ShepPalette.overlay1 // todo
    }

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
