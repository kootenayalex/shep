package dev.shep.companion.terminal

/**
 * What the soft keyboard's `InputConnection` calls turn into.
 *
 * Kept free of Android types so the translation can be pinned on the JVM;
 * [ShepInputView]'s connection delegates every call here.
 */
object ImeKeys {
    /** There is no buffer, so the IME is always told the text before the cursor is empty. */
    const val NO_BUFFER = ""

    fun commit(text: CharSequence?): TerminalKey? =
        text?.toString()?.takeIf { it.isNotEmpty() }?.let { TerminalKey.Text(it) }

    /**
     * Composition is deliberately not buffered.
     *
     * A terminal has no undo and agents react per keystroke, so holding text
     * back until the IME commits would mean the remote side sees nothing while
     * the user types. Each delta is sent as it arrives; the cost is that
     * gesture-typed words land as one chunk.
     */
    fun compose(text: CharSequence?): TerminalKey? = commit(text)

    /**
     * Gboard's backspace under `TYPE_NULL`, and its "delete the word" burst.
     *
     * One key carrying N names, not N keys: a burst of ten backspaces used to
     * be ten socket writes (or ten request round trips before the stream was
     * up), and the pty saw them trickle in.
     */
    fun deleteSurrounding(before: Int, after: Int): TerminalKey? {
        val names = List(before.coerceAtLeast(0)) { "backspace" } +
            List(after.coerceAtLeast(0)) { "delete" }
        return when (names.size) {
            0 -> null
            1 -> TerminalKey.Named(names[0])
            else -> TerminalKey.Keys(names)
        }
    }

    fun editorAction(): TerminalKey = TerminalKey.Named("enter")
}
