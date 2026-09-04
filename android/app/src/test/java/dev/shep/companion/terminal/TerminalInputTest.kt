package dev.shep.companion.terminal

import android.view.KeyEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * What the soft keyboard's calls become on the wire.
 *
 * The live terminal has no text box: the IME talks to an `InputConnection`
 * and every call has to turn into a key the server understands, at once. A
 * wrong translation is silent — the pty just does not get the keystroke — so
 * each path the keyboard can take is pinned here.
 */
class TerminalInputTest {

    @Test
    fun `committed text is sent as text`() {
        assertEquals(TerminalKey.Text("ls -la"), ImeKeys.commit("ls -la"))
        assertNull(ImeKeys.commit(""))
        assertNull(ImeKeys.commit(null))
    }

    /** Gesture typing composes; the terminal must see it as it arrives, not on commit. */
    @Test
    fun `composition is sent unbuffered`() {
        assertEquals(TerminalKey.Text("hel"), ImeKeys.compose("hel"))
        assertEquals(ImeKeys.commit("x"), ImeKeys.compose("x"))
    }

    @Test
    fun `one backspace is one named key`() {
        assertEquals(TerminalKey.Named("backspace"), ImeKeys.deleteSurrounding(1, 0))
        assertEquals(TerminalKey.Named("delete"), ImeKeys.deleteSurrounding(0, 1))
    }

    /** A "delete the word" burst is one write carrying N backspaces, in order. */
    @Test
    fun `a burst of deletes is one key with n names`() {
        assertEquals(
            TerminalKey.Keys(listOf("backspace", "backspace", "backspace", "delete")),
            ImeKeys.deleteSurrounding(3, 1),
        )
        assertNull(ImeKeys.deleteSurrounding(0, 0))
        assertNull(ImeKeys.deleteSurrounding(-2, 0))
    }

    @Test
    fun `the IME is never shown a buffer`() {
        assertEquals("", ImeKeys.NO_BUFFER)
    }

    @Test
    fun `the editor action is enter`() {
        assertEquals(TerminalKey.Named("enter"), ImeKeys.editorAction())
    }

    /** Keys with no unicode char map to the names `parse_api_key` accepts; anything else is null. */
    @Test
    fun `keycodes map to server key names`() {
        val table = mapOf(
            KeyEvent.KEYCODE_DEL to "backspace",
            KeyEvent.KEYCODE_FORWARD_DEL to "delete",
            KeyEvent.KEYCODE_ENTER to "enter",
            KeyEvent.KEYCODE_NUMPAD_ENTER to "enter",
            KeyEvent.KEYCODE_TAB to "tab",
            KeyEvent.KEYCODE_ESCAPE to "esc",
            KeyEvent.KEYCODE_DPAD_UP to "up",
            KeyEvent.KEYCODE_DPAD_DOWN to "down",
            KeyEvent.KEYCODE_DPAD_LEFT to "left",
            KeyEvent.KEYCODE_DPAD_RIGHT to "right",
            KeyEvent.KEYCODE_MOVE_HOME to "home",
            KeyEvent.KEYCODE_MOVE_END to "end",
            KeyEvent.KEYCODE_PAGE_UP to "pageup",
            KeyEvent.KEYCODE_PAGE_DOWN to "pagedown",
            KeyEvent.KEYCODE_INSERT to "insert",
        )
        for ((code, name) in table) assertEquals(name, keyName(code))
        assertNull(keyName(KeyEvent.KEYCODE_A))
        assertNull(keyName(KeyEvent.KEYCODE_SPACE))
    }
}
