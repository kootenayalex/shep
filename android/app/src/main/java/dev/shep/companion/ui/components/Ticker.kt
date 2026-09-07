package dev.shep.companion.ui.components

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import kotlinx.coroutines.delay

/**
 * A monotonic clock that advances once a second while it is on screen.
 *
 * An age the server stated once is only true at the moment it was stated. The
 * board holds rows that nothing updates for minutes at a time — an idle agent
 * produces no events — so without a clock of its own the counter under a status
 * sits at whatever it said when the row arrived, and only appears to move when
 * leaving the screen and coming back rebuilds it.
 *
 * A second is the right period because the shortest thing an age says is
 * seconds; the coroutine stops with the composable, so a backgrounded screen
 * costs nothing.
 */
@Composable
fun rememberSecondsTicker(): State<Long> {
    val now = remember { mutableLongStateOf(android.os.SystemClock.elapsedRealtime()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000L)
            now.longValue = android.os.SystemClock.elapsedRealtime()
        }
    }
    return now
}
