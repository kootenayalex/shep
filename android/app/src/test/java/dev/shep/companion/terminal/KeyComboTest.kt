package dev.shep.companion.terminal

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The combo strings the key bar puts on the wire.
 *
 * Every name has to survive `parse_key_combo` (shep `src/config/keybinds.rs`)
 * on the other end. A combo the server does not recognise comes back as
 * `invalid_key` and the key silently does nothing, which on a phone reads as a
 * dead button rather than as an error — so the spellings are pinned here.
 */
class KeyComboTest {

    @Test
    fun `a bare key carries no modifiers`() {
        assertEquals("tab", keyCombo("tab"))
        assertEquals("enter", keyCombo("enter"))
    }

    /**
     * The key this whole bar exists for: claude cycles its permission mode on
     * shift+tab, and a phone keyboard cannot produce it at all.
     */
    @Test
    fun `shift plus tab is spelled the way the server parses it`() {
        assertEquals("shift+tab", keyCombo("tab", shift = true))
    }

    @Test
    fun `modifiers stack in a fixed order`() {
        assertEquals(
            "ctrl+alt+shift+tab",
            keyCombo("tab", ModifierState.Armed, ModifierState.Locked, shift = true),
        )
        assertEquals("ctrl+shift+tab", keyCombo("tab", ctrl = ModifierState.Armed, shift = true))
        assertEquals("alt+shift+tab", keyCombo("tab", alt = ModifierState.Armed, shift = true))
    }

    /** Armed and locked differ in how long they last, not in what they send. */
    @Test
    fun `a locked modifier sends the same combo as an armed one`() {
        assertEquals(
            keyCombo("up", ctrl = ModifierState.Armed),
            keyCombo("up", ctrl = ModifierState.Locked),
        )
    }

    @Test
    fun `tapping a modifier off leaves the key alone`() {
        assertEquals("up", keyCombo("up", ModifierState.Off, ModifierState.Off, shift = false))
    }

    /**
     * Tap arms, a second tap on an armed modifier locks it, and a tap on a
     * locked one clears — so a one-off chord costs one tap and a run of them
     * does not need the modifier held down.
     */
    @Test
    fun `sticky modifiers advance the way terminal apps have trained people`() {
        assertEquals(ModifierState.Armed, ModifierState.Off.advance(doubleTap = false))
        assertEquals(ModifierState.Off, ModifierState.Armed.advance(doubleTap = false))
        assertEquals(ModifierState.Locked, ModifierState.Off.advance(doubleTap = true))
        assertEquals(ModifierState.Off, ModifierState.Locked.advance(doubleTap = true))
    }
}
