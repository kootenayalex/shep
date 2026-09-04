package dev.shep.companion.ui.theme

import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import dev.shep.companion.R

/**
 * Type for a terminal companion: monospace carries the identity.
 *
 * Anything that is shep's own vocabulary — agent names, pane ids, states,
 * badges, numbers, paths, the terminal itself — is mono. Prose is sans, and
 * prose here means only three things: what an agent said, what a task asks for,
 * and what a memory entry records. Everything else on these screens is shep
 * talking about itself, and shep talks in mono.
 *
 * JetBrains Mono is bundled rather than using [FontFamily.Monospace], which on
 * Android resolves to Droid Sans Mono and reads visibly wrong against the
 * desktop TUI.
 *
 * **Every size in the app comes from this file.** A JVM test fails the build on
 * an `.sp` literal anywhere else — see `ThemeTokensTest`. Before this pass the
 * app used twelve font sizes across 126 inline literals, and the bundled font
 * was reaching only the pane views: the board, groups, tasks, memory, settings
 * and pairing screens all rendered in Roboto, which is the single loudest way
 * the companion failed to look like shep.
 */
val JetBrainsMono = FontFamily(
    Font(R.font.jetbrains_mono_regular, FontWeight.Normal),
    Font(R.font.jetbrains_mono_bold, FontWeight.Bold),
)

object ShepType {
    /** The two families. Roles below are built from these; call sites use roles. */
    val mono = TextStyle(fontFamily = JetBrainsMono)
    val sans = TextStyle(fontFamily = FontFamily.Default)

    // ── Headings ────────────────────────────────────────────────────────────

    /** The one word at the top of a tab: "board", "agents", "tasks", "memory". */
    val screenTitle = mono.copy(
        fontSize = 22.sp,
        fontWeight = FontWeight.Bold,
        color = ShepPalette.text,
    )

    /** A bottom sheet's or dialog's first line. */
    val sheetTitle = mono.copy(
        fontSize = 18.sp,
        fontWeight = FontWeight.Bold,
        color = ShepPalette.text,
    )

    /** Names a group of controls: "notify me about", "delivery", "run". */
    val sectionLabel = mono.copy(
        fontSize = 13.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.text,
    )

    /** Lowercase view label inside a pane, e.g. "output" / "transcript". */
    val viewTitle = mono.copy(
        fontSize = 12.sp,
        letterSpacing = 1.sp,
        color = ShepPalette.overlay0,
    )

    /** Pairing's one big word — the only place shep shouts. */
    val hero = mono.copy(
        fontSize = 40.sp,
        fontWeight = FontWeight.Bold,
        color = ShepPalette.accent,
    )

    /** The `shep` mark in a title bar. */
    val wordmark = mono.copy(
        fontSize = 16.sp,
        fontWeight = FontWeight.ExtraBold,
        letterSpacing = 0.5.sp,
    )

    // ── Naming things ───────────────────────────────────────────────────────

    /** The primary name on a surface: a board card's agent, a group's label. */
    val agentName = mono.copy(
        fontSize = 15.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.text,
    )

    /** A name one level in: a row inside a sheet, a picker entry. */
    val itemName = mono.copy(
        fontSize = 14.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.text,
    )

    /** A name two levels in: a tab or pane inside the groups tree. */
    val itemLabel = mono.copy(fontSize = 13.sp, color = ShepPalette.subtext0)

    /** An identifier shep generated rather than a person chose. */
    val paneId = mono.copy(fontSize = 11.sp, color = ShepPalette.overlay0)

    // ── Facts ───────────────────────────────────────────────────────────────

    /** An agent's own status line. Coloured by the caller from its state. */
    val state = mono.copy(fontSize = 13.sp)

    /**
     * A state glyph on a card. See `StateGlyph`.
     *
     * Deliberately the same size as [agentName] beside it: `◉` and `○` carry
     * far less ink than a capital letter, so a glyph set one step down reads
     * as a speck next to the name — which made the most important fact on the
     * card the smallest thing on it.
     */
    val stateGlyph = mono.copy(fontSize = 15.sp, fontWeight = FontWeight.Bold)

    /** A state glyph one level in: the groups tree, a title bar, a task row. */
    val stateGlyphSmall = mono.copy(fontSize = 13.sp, fontWeight = FontWeight.Bold)

    /** Ids, ages, paths, counts — the quiet second line of nearly every row. */
    val meta = mono.copy(fontSize = 12.sp, color = ShepPalette.overlay0)

    /** The same, one step quieter: a cwd under a name, a hint under a field. */
    val metaSmall = mono.copy(fontSize = 11.sp, color = ShepPalette.overlay0)

    /** `⇥3`, `⑂`, `84%`, "same repo" — a short fact that must be picked out. */
    val badge = mono.copy(fontSize = 11.sp, fontWeight = FontWeight.Bold)

    /** Text inside a pill. */
    val chip = mono.copy(fontSize = 12.sp)

    /** The first line of what a screen says when it has nothing to show. */
    val emptyTitle = mono.copy(
        fontSize = 14.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.subtext0,
    )

    /** The rest of it, and the same voice anywhere else a screen explains itself. */
    val emptyState = mono.copy(fontSize = 13.sp, color = ShepPalette.overlay0)

    /** The question on a collapsed explain row: "how to read this list". */
    val explainLabel = mono.copy(
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.overlay1,
    )

    /** The `glyph =` half of a line inside an open explain row. */
    val explainTerm = mono.copy(
        fontSize = 12.sp,
        fontWeight = FontWeight.Bold,
        color = ShepPalette.subtext0,
    )

    /**
     * The number on a numbered step.
     *
     * Deliberately not copper: accent is focus, never a state and never chrome,
     * and a step number is chrome.
     */
    val stepNumber = mono.copy(
        fontSize = 13.sp,
        fontWeight = FontWeight.Bold,
        color = ShepPalette.overlay1,
    )

    /** One line saying what a whole change is: review's "written by ... - N files". */
    val summary = mono.copy(fontSize = 13.sp, color = ShepPalette.subtext0)

    // ── Doing things ────────────────────────────────────────────────────────

    /** A tappable word that does something: "send to…", "split", "ship ⑂". */
    val action = mono.copy(fontSize = 13.sp, fontWeight = FontWeight.SemiBold)

    /** The same, when it is the secondary option: "cancel", "remove", "keep". */
    val actionQuiet = mono.copy(fontSize = 13.sp, color = ShepPalette.overlay0)

    /** A header's one affordance: "+ new", "+ add", "voice". */
    val actionStrong = mono.copy(
        fontSize = 14.sp,
        fontWeight = FontWeight.SemiBold,
        color = ShepPalette.accent,
    )

    /** Content of a Material [androidx.compose.material3.Button]. */
    val button = mono.copy(fontSize = 14.sp, fontWeight = FontWeight.SemiBold)

    /** A key on the terminal key bar. */
    val key = mono.copy(fontSize = 13.sp, fontWeight = FontWeight.Bold)

    /** Legacy navigation styles retained for non-shell surfaces. */
    val navGlyph = mono.copy(fontSize = 18.sp)
    val navLabel = mono.copy(fontSize = 11.sp)

    // ── Input ───────────────────────────────────────────────────────────────

    /** What you type into a text field. */
    val field = mono.copy(fontSize = 14.sp, color = ShepPalette.text)

    /** A field's label and its placeholder. */
    val fieldLabel = mono.copy(fontSize = 12.sp, color = ShepPalette.overlay1)

    /** Terminal input and one-line hints beside it. */
    val hint = mono.copy(fontSize = 12.sp)

    // ── Prose — the only sans on these screens ──────────────────────────────

    /**
     * What an agent said, what a task asks for, what a memory entry records.
     *
     * Line height is generous because this is the only text on the phone anyone
     * reads a paragraph of; mono at the same measure would be a wall.
     */
    val body = sans.copy(
        fontSize = 14.sp,
        lineHeight = 20.sp,
        color = ShepPalette.text,
    )

    /** Prose that is explaining rather than reporting: dialog bodies, captions. */
    val bodySmall = sans.copy(
        fontSize = 13.sp,
        lineHeight = 18.sp,
        color = ShepPalette.overlay1,
    )

    // ── Verbatim output ─────────────────────────────────────────────────────

    /** A diff, a `git stat`, a tool's raw stdout. Tight, because it is dense. */
    val code = mono.copy(fontSize = 12.sp, lineHeight = 17.sp)

    /** The same where a lot of it has to fit: a full-screen diff. */
    val codeSmall = mono.copy(fontSize = 11.sp, lineHeight = 15.sp)

    /**
     * The terminal's own cell, before the user pinches.
     *
     * A bare `Float` rather than a `TextStyle` because `TerminalGrid` measures
     * one cell and multiplies, so it needs the number, and because pinch-zoom
     * scales it continuously — this is the base of a range, not a step on the
     * scale above.
     */
    const val TERMINAL_BASE_SP = 13f
}
