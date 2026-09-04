package dev.shep.companion.terminal

import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/**
 * What a keypress turns into on the wire.
 *
 * Names must match `parse_api_key`/`normalize_api_key_alias` in the shep repo
 * (src/app/api_helpers.rs) — an unrecognized name is rejected server-side with
 * no visible effect on the phone.
 */
sealed interface TerminalKey {
    data class Text(val text: String) : TerminalKey
    data class Named(val name: String) : TerminalKey
    /** Several named keys in one write, e.g. a burst of backspaces. */
    data class Keys(val names: List<String>) : TerminalKey
}

/**
 * An invisible key sink for the live terminal.
 *
 * There is no text box: keystrokes go straight to the pty and the *terminal's*
 * own cursor is the caret the user sees. That rules out a normal text field,
 * which would hold a local buffer the remote shell knows nothing about.
 *
 * Two input paths have to be handled, because neither alone is sufficient:
 *  - `sendKeyEvent`, which is what physical keyboards and most soft-keyboard
 *    keys produce under [InputType.TYPE_NULL]
 *  - `commitText`, which Gboard still uses for gesture typing, emoji, and some
 *    locales regardless of the input type
 */
class ShepInputView(context: Context) : View(context) {

    var onKey: (TerminalKey) -> Unit = {}

    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    override fun onCheckIsTextEditor(): Boolean = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // TYPE_NULL asks the IME to behave as a dumb key sink: no composition,
        // no autocorrect, no suggestion strip.
        outAttrs.inputType = InputType.TYPE_NULL
        outAttrs.imeOptions = EditorInfo.IME_ACTION_NONE or
            EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_FLAG_NO_PERSONALIZED_LEARNING
        return TerminalInputConnection(this)
    }

    fun showKeyboard() {
        requestFocus()
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.showSoftInput(this, InputMethodManager.SHOW_IMPLICIT)
    }

    fun hideKeyboard() {
        val imm = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        imm.hideSoftInputFromWindow(windowToken, 0)
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean {
        if (handleKeyEvent(event)) return true
        return super.onKeyDown(keyCode, event)
    }

    internal fun handleKeyEvent(event: KeyEvent): Boolean {
        if (event.action != KeyEvent.ACTION_DOWN) return false
        keyName(event.keyCode)?.let { onKey(TerminalKey.Named(it)); return true }
        val unicode = event.unicodeChar
        if (unicode != 0) {
            onKey(TerminalKey.Text(unicode.toChar().toString()))
            return true
        }
        return false
    }

    private class TerminalInputConnection(private val view: ShepInputView) :
        BaseInputConnection(view, false) {

        // The translation itself is [ImeKeys], which has no Android in it and
        // is pinned by TerminalInputTest; this class only owns the connection.
        override fun commitText(text: CharSequence?, newCursorPosition: Int): Boolean {
            ImeKeys.commit(text)?.let(view.onKey)
            return true
        }

        override fun setComposingText(text: CharSequence?, newCursorPosition: Int): Boolean {
            ImeKeys.compose(text)?.let(view.onKey)
            return true
        }

        override fun finishComposingText(): Boolean = true

        /** How Gboard's backspace arrives under TYPE_NULL. */
        override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
            ImeKeys.deleteSurrounding(beforeLength, afterLength)?.let(view.onKey)
            return true
        }

        override fun sendKeyEvent(event: KeyEvent?): Boolean {
            if (event != null && view.handleKeyEvent(event)) return true
            return super.sendKeyEvent(event)
        }

        override fun performEditorAction(actionCode: Int): Boolean {
            view.onKey(ImeKeys.editorAction())
            return true
        }

        // Never expose a buffer to the IME — there isn't one.
        override fun getTextBeforeCursor(length: Int, flags: Int): CharSequence = ImeKeys.NO_BUFFER
        override fun getTextAfterCursor(length: Int, flags: Int): CharSequence = ImeKeys.NO_BUFFER
        override fun getSelectedText(flags: Int): CharSequence? = null
    }
}

/** Android keycode → shep API key name, for keys that carry no unicode char. */
internal fun keyName(keyCode: Int): String? = when (keyCode) {
    KeyEvent.KEYCODE_DEL -> "backspace"
    KeyEvent.KEYCODE_FORWARD_DEL -> "delete"
    KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> "enter"
    KeyEvent.KEYCODE_TAB -> "tab"
    KeyEvent.KEYCODE_ESCAPE -> "esc"
    KeyEvent.KEYCODE_DPAD_UP -> "up"
    KeyEvent.KEYCODE_DPAD_DOWN -> "down"
    KeyEvent.KEYCODE_DPAD_LEFT -> "left"
    KeyEvent.KEYCODE_DPAD_RIGHT -> "right"
    KeyEvent.KEYCODE_MOVE_HOME -> "home"
    KeyEvent.KEYCODE_MOVE_END -> "end"
    KeyEvent.KEYCODE_PAGE_UP -> "pageup"
    KeyEvent.KEYCODE_PAGE_DOWN -> "pagedown"
    KeyEvent.KEYCODE_INSERT -> "insert"
    else -> null
}
