package dev.shep.companion.screens.pane

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.Transcript
import dev.shep.companion.net.StreamEvent
import dev.shep.companion.net.paneStream
import dev.shep.companion.screens.ReviewScreen
import dev.shep.companion.statusColor
import dev.shep.companion.terminal.GridState
import dev.shep.companion.terminal.KeyBar
import dev.shep.companion.terminal.ShepInputView
import dev.shep.companion.terminal.TerminalGrid
import dev.shep.companion.terminal.TerminalKey
import dev.shep.companion.terminal.stepFontSize
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.components.StateGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import androidx.compose.material3.minimumInteractiveComponentSize

/**
 * How often collected scroll is sent.
 *
 * Three frames. Short enough that the pane keeps up with a thumb, long enough
 * that a fling is a handful of requests rather than one per pointer event.
 */
private const val SCROLL_FLUSH_MS = 50L

/** Where the chosen terminal text size and any unsent drafts are kept. */
private const val PREFS = "shep"
private const val PREF_TERMINAL_SP = "terminal_sp"

private fun draftKey(paneId: String) = "draft:$paneId"

/** Live keystrokes, or compose-and-queue for when the agent is busy. */
enum class InputMode { Stream, Queue }

/**
 * The pane's screen as it is right now, or the conversation that produced it.
 *
 * Live mirrors the pty — right for driving an agent. Recorded reads the agent's
 * own session log and lays it out as a chat — right for catching up on what it
 * did while you were away.
 */
enum class OutputMode { Live, Recorded }

/**
 * A live pane: the terminal as it looks on the desktop, and a way to type into
 * it.
 *
 * Output is a streamed cell grid rather than polled text, and input goes
 * straight to the pty as it is typed. The stream is strictly an *observer* —
 * it never resizes the real pane (see `pane.stream` in the shep repo).
 */
@Composable
fun PaneScreen(
    client: BridgeClient,
    row: AgentRow,
    onBack: () -> Unit,
) {
    val grid = remember(row.paneId) { GridState() }
    val context = LocalContext.current
    val prefs = remember(context) {
        context.getSharedPreferences(PREFS, android.content.Context.MODE_PRIVATE)
    }
    var status by remember { mutableStateOf(row.status) }
    var notice by remember { mutableStateOf<String?>(null) }
    var ended by remember { mutableStateOf<String?>(null) }
    var mode by remember { mutableStateOf(InputMode.Stream) }
    var output by remember { mutableStateOf(OutputMode.Live) }
    // The terminal's text size is a setting, not a session: picking a readable
    // size once should not have to be redone on every pane, or after a restart.
    var fontSizeSp by remember {
        mutableFloatStateOf(prefs.getFloat(PREF_TERMINAL_SP, ShepType.TERMINAL_BASE_SP))
    }
    LaunchedEffect(fontSizeSp) { prefs.edit().putFloat(PREF_TERMINAL_SP, fontSizeSp).apply() }
    // A half-written prompt is worth more than the app's process. Kept across a
    // rotation by `rememberSaveable`, and across being swiped away or killed in
    // the background by the draft below — which is how it used to vanish.
    var composer by rememberSaveable(row.paneId) {
        mutableStateOf(prefs.getString(draftKey(row.paneId), "").orEmpty())
    }
    // Rows the drag gesture has asked for and the flush below has not sent yet.
    // A fling is sixty pointer events a second; one request each would put the
    // scroll behind the finger by the time it stopped moving.
    var pendingScroll by remember(row.paneId) { mutableIntStateOf(0) }
    var showReview by remember { mutableStateOf(false) }
    var streaming by remember { mutableStateOf(false) }
    var transcript by remember(row.paneId) { mutableStateOf<Transcript?>(null) }
    var transcriptError by remember(row.paneId) { mutableStateOf<String?>(null) }
    var transcriptLoading by remember(row.paneId) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    // Held so the key bar and the IME can both reach the open channel.
    var send by remember { mutableStateOf<((TerminalKey) -> Unit)>({}) }

    // Every key, from the bar or from the soft keyboard, goes through here.
    val press: (TerminalKey) -> Unit = { key -> send(key) }

    // Collected scroll goes out on a beat. `pane.scroll` routes it the way the
    // desktop routes a wheel over the same pane: a shell moves its own
    // viewport, an agent on the alternate screen — which keeps no terminal
    // scrollback at all — is sent the scroll and moves its own view.
    LaunchedEffect(row.paneId, client) {
        while (true) {
            kotlinx.coroutines.delay(SCROLL_FLUSH_MS)
            val rows = pendingScroll
            if (rows == 0) continue
            pendingScroll = 0
            withContext(Dispatchers.IO) {
                runCatching {
                    client.call(
                        "pane.scroll",
                        JSONObject().put("pane_id", row.paneId).put("rows", rows),
                    )
                }
            }
        }
    }

    // Pause while backgrounded: a live terminal is not worth the battery when
    // nobody is looking, and reconnecting repaints from a full frame anyway.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var active by remember { mutableStateOf(true) }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> active = true
                Lifecycle.Event.ON_STOP -> active = false
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }

    // Park the draft whenever the screen stops being watched. `ON_STOP` is the
    // last moment guaranteed to run before the process can be killed, so this
    // is the difference between minimising the app and losing what you typed.
    val draft by rememberUpdatedState(composer)
    DisposableEffect(lifecycle, row.paneId) {
        val save = {
            prefs.edit().apply {
                if (draft.isBlank()) remove(draftKey(row.paneId)) else putString(draftKey(row.paneId), draft)
            }.apply()
        }
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) save()
        }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            save()
        }
    }

    // Review is a full-screen push over the pane, so it belongs here rather
    // than threaded up through the nav shell. The screen and its
    // `workspace.diff` / `workspace.ship` calls were finished months ago; the
    // title bar's "review" was visible, tappable and rippling, and did nothing,
    // because no caller ever passed the callback.
    if (showReview) {
        BackHandler { showReview = false }
        ReviewScreen(client, row, onBack = { showReview = false })
        return
    }

    // Only polled while the recorded view is on screen: it is a whole-file read
    // on the other end, and the live stream is the expensive thing to keep warm.
    LaunchedEffect(row.paneId, client, output, active) {
        if (output != OutputMode.Recorded || !active) return@LaunchedEffect
        transcriptLoading = transcript == null
        while (true) {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    client.call(
                        "pane.transcript",
                        JSONObject().put("target", row.paneId).put("limit", 200),
                    )
                }
            }
            transcriptLoading = false
            result
                .onSuccess {
                    transcript = dev.shep.companion.parseTranscript(it)
                    transcriptError = null
                }
                .onFailure {
                    // Keep whatever we already showed; a poll failing is not a
                    // reason to blank the conversation.
                    if (transcript == null) transcriptError = it.message ?: "no transcript"
                }
            kotlinx.coroutines.delay(5000)
        }
    }

    LaunchedEffect(row.paneId, client, active) {
        if (!active) return@LaunchedEffect
        ended = null
        client.paneStream(row.paneId).collect { (channel, event) ->
            send = { key ->
                when (key) {
                    is TerminalKey.Text -> channel.sendText(key.text)
                    is TerminalKey.Named -> channel.sendKeys(key.name)
                }
            }
            when (event) {
                is StreamEvent.Size -> streaming = true
                is StreamEvent.Frame -> {
                    streaming = true
                    // A frame is proof the stream works, so any complaint left
                    // over from the drop that preceded it is no longer true.
                    notice = null
                    grid.apply(event.json)
                }
                is StreamEvent.Ping -> {}
                is StreamEvent.Ended -> ended = event.reason ?: "pane ended"
                is StreamEvent.Failed -> {
                    streaming = false
                    notice = event.message
                }
            }
        }
    }

    // Status still comes from the snapshot: the frame carries pixels, not state.
    LaunchedEffect(row.paneId) {
        while (true) {
            withContext(Dispatchers.IO) {
                runCatching { client.call("session.snapshot") }.getOrNull()
            }?.let { snap ->
                dev.shep.companion.parseSnapshot(snap)
                    .find { it.paneId == row.paneId }
                    ?.let { status = it.status }
            }
            kotlinx.coroutines.delay(3000)
        }
    }

    Column(Modifier.fillMaxSize().background(ShepPalette.panelBg).imePadding()) {
        PaneTitleBar(
            row,
            status,
            onBack = onBack,
            fontSizeSp = fontSizeSp,
            onFontSizeSp = { fontSizeSp = it },
        )

        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = ShepSpace.small, vertical = ShepSpace.medium),
        ) {
            if (output == OutputMode.Recorded) {
                PaneFrame(agent = row.agent, state = status, blocked = status == "blocked") {
                    TranscriptView(
                        transcript = transcript,
                        error = transcriptError,
                        loading = transcriptLoading,
                        onLiveTerminal = { output = OutputMode.Live },
                        modifier = Modifier.fillMaxSize(),
                    )
                }
                return@Box
            }
            PaneFrame(agent = row.agent, state = status, blocked = status == "blocked") {
                Box(Modifier.fillMaxSize()) {
                    val context = androidx.compose.ui.platform.LocalContext.current
                    val inputView = remember(context) {
                        ShepInputView(context).apply { onKey = press }
                    }
                    // Keep the sink pointed at the current channel.
                    inputView.onKey = press

                    TerminalGrid(
                        grid = grid,
                        modifier = Modifier.fillMaxSize().testTag("terminal-grid"),
                        fontSizeSp = fontSizeSp,
                        onFontSizeSp = { fontSizeSp = it },
                        onTap = { if (mode == InputMode.Stream) inputView.showKeyboard() },
                        onScrollRows = { pendingScroll += it },
                    )
                    // Zero-size: it exists only to own the IME connection, so
                    // this dimension is a hack and not a design token.
                    AndroidView(factory = { inputView }, modifier = Modifier.size(1.dp)) // not-a-token

                    if (!streaming && grid.isEmpty) {
                        Text(
                            "connecting to pane…",
                            style = ShepType.hint.copy(color = ShepPalette.overlay0),
                            modifier = Modifier.align(Alignment.Center),
                        )
                    }
                    ended?.let {
                        Text(
                            it,
                            style = ShepType.hint.copy(color = ShepPalette.red),
                            modifier = Modifier
                                .align(Alignment.TopCenter)
                                .background(ShepPalette.redDim, ShepShape.key)
                                .padding(horizontal = ShepSpace.small, vertical = ShepSpace.tight),
                        )
                    }
                }
            }
        }

        notice?.let {
            Text(
                it,
                style = ShepType.hint.copy(color = ShepPalette.peach),
                modifier = Modifier.padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
            )
        }

        InputBar(
            mode = mode,
            onMode = { mode = it },
            output = output,
            onOutput = {
                output = it
                // The two halves pair by default: a chat wants a composer, a
                // live terminal wants keystrokes. The input chips are right
                // there, so this is a default rather than a decision.
                mode = if (it == OutputMode.Recorded) InputMode.Queue else InputMode.Stream
            },
            onReview = { showReview = true },
        )

        when (mode) {
            InputMode.Stream -> KeyBar(onKey = press)
            InputMode.Queue -> QueueComposer(
                value = composer,
                onValue = { composer = it },
                onSend = { queue ->
                    val text = composer.trim()
                    if (text.isEmpty()) return@QueueComposer
                    scope.launch(Dispatchers.IO) {
                        runCatching {
                            client.call(
                                "agent.send",
                                JSONObject()
                                    .put("target", row.paneId)
                                    .put("text", text)
                                    .put("queue", queue),
                            )
                        }.onSuccess {
                            composer = ""
                            notice = if (queue) "queued — fires on idle" else "sent"
                        }.onFailure { notice = it.message }
                    }
                },
            )
        }
    }
}

@Composable
private fun PaneTitleBar(
    row: AgentRow,
    status: String,
    onBack: () -> Unit,
    fontSizeSp: Float,
    onFontSizeSp: (Float) -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.medium),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        ActionText(
            "‹",
            style = ShepType.wordmark.copy(color = ShepPalette.accent),
            description = "back to the board",
            onClick = onBack,
        )
        Column(Modifier.weight(1f)) {
            Text(row.paneId, style = ShepType.agentName.copy(color = ShepPalette.accent))
            Text(
                listOfNotNull(row.workspaceLabel, row.branch).joinToString(" · "),
                style = ShepType.paneId,
                maxLines = 1,
            )
        }
        TextSizeControl(fontSizeSp, onFontSizeSp)
        StateGlyph(status, style = ShepType.stateGlyphSmall)
        Text(status, style = ShepType.metaSmall.copy(color = statusColor(status)))
    }
}

/**
 * How big the terminal's text is.
 *
 * The most consequential control on the screen, because the size is also the
 * wrap: bigger text means fewer of the pane's columns per line and more lines
 * to scroll, smaller means more of the agent's own layout survives intact.
 * Living in the title bar is the point — it is adjusted while reading, not
 * found in a settings screen.
 */
@Composable
private fun TextSizeControl(sp: Float, onSp: (Float) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        ActionText(
            "−",
            style = ShepType.key.copy(color = ShepPalette.accent),
            description = "smaller terminal text",
            onClick = { onSp(stepFontSize(sp, -1)) },
        )
        Text(
            sp.toInt().toString(),
            style = ShepType.paneId,
            modifier = Modifier.testTag("terminal-size"),
        )
        ActionText(
            "+",
            style = ShepType.key.copy(color = ShepPalette.accent),
            description = "bigger terminal text",
            onClick = { onSp(stepFontSize(sp, 1)) },
        )
    }
}

/** Bordered viewport with the desktop's `agent · state` label on the border. */
@Composable
private fun PaneFrame(
    agent: String,
    state: String,
    blocked: Boolean,
    content: @Composable () -> Unit,
) {
    val ring = if (blocked) ShepPalette.red else ShepPalette.accent
    Box(Modifier.fillMaxSize()) {
        Box(
            Modifier
                .fillMaxSize()
                .border(ShepSize.focusRing, ring, ShepShape.sheet)
                .padding(ShepSpace.small),
        ) { content() }
        Text(
            "$agent · $state",
            style = ShepType.badge.copy(color = ring),
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(start = ShepSpace.medium)
                .background(ShepPalette.panelBg)
                .padding(horizontal = ShepSpace.snug),
        )
    }
}

/**
 * One row for everything that is not the terminal itself.
 *
 * There used to be two: an `out` row choosing live terminal or recorded chat,
 * and an `in` row choosing keystrokes or a composer. On a phone that is two
 * bands of chrome between the pane and the keyboard, for one decision each. The
 * output modes are two states, so they are one word naming where you are *not*
 * — and switching output still carries the input with it, which is why the two
 * ever read as a pair.
 */
@Composable
private fun InputBar(
    mode: InputMode,
    onMode: (InputMode) -> Unit,
    output: OutputMode,
    onOutput: (OutputMode) -> Unit,
    onReview: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.small, vertical = ShepSpace.tight),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("out", style = ShepType.viewTitle)
        ActionText(
            if (output == OutputMode.Recorded) "recorded" else "live",
            style = ShepType.hint.copy(color = ShepPalette.accent),
            description = if (output == OutputMode.Recorded) {
                "back to the live terminal"
            } else {
                "read the recorded conversation"
            },
            onClick = { onOutput(if (output == OutputMode.Recorded) OutputMode.Live else OutputMode.Recorded) },
        )
        Spacer(Modifier.width(ShepSpace.small))
        Text("in", style = ShepType.viewTitle)
        ModeChip("live", mode == InputMode.Stream) { onMode(InputMode.Stream) }
        ModeChip("⇥ queue", mode == InputMode.Queue) { onMode(InputMode.Queue) }
        Spacer(Modifier.weight(1f))
        ActionText(
            "review",
            style = ShepType.hint.copy(color = ShepPalette.accent),
            onClick = onReview,
        )
    }
}

@Composable
private fun ModeChip(label: String, on: Boolean, onClick: () -> Unit) {
    ShepChip(label, on, modifier = Modifier.testTag("mode-in-$label"), onClick = onClick)
}

@Composable
private fun QueueComposer(
    value: String,
    onValue: (String) -> Unit,
    onSend: (queue: Boolean) -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        Box(
            Modifier
                .weight(1f)
                .clip(ShepShape.field)
                .background(ShepPalette.surface0)
                .border(ShepSize.border, ShepPalette.surface1, ShepShape.field)
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        ) {
            if (value.isEmpty()) {
                Text("prompt claude…", style = ShepType.hint.copy(color = ShepPalette.overlay0))
            }
            BasicTextField(
                value = value,
                onValueChange = onValue,
                textStyle = ShepType.hint.copy(color = ShepPalette.text),
                cursorBrush = androidx.compose.ui.graphics.SolidColor(ShepPalette.accent),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Box(
            Modifier
                .minimumInteractiveComponentSize()
                .clip(ShepShape.field)
                .background(ShepPalette.tealDim)
                .border(ShepSize.border, ShepPalette.teal, ShepShape.field)
                .clickable { onSend(true) }
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
            contentAlignment = Alignment.Center,
        ) { Text("queue", style = ShepType.key.copy(color = ShepPalette.teal)) }
        Box(
            Modifier
                .minimumInteractiveComponentSize()
                .clip(ShepShape.field)
                .background(ShepPalette.accent)
                .clickable { onSend(false) }
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
            contentAlignment = Alignment.Center,
        ) { Text("send", style = ShepType.key.copy(color = ShepPalette.panelBg)) }
    }
}
