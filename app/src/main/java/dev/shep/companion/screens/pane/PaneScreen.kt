package dev.shep.companion.screens.pane

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalDensity
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
import dev.shep.companion.terminal.GridState
import dev.shep.companion.terminal.KeyBar
import dev.shep.companion.terminal.ShepInputView
import dev.shep.companion.terminal.TerminalGrid
import dev.shep.companion.terminal.TerminalKey
import dev.shep.companion.terminal.fittedGridHeight
import dev.shep.companion.terminal.rememberTerminalCell
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
    // Measured here as well as inside the grid, because the frame around it is
    // sized to what the grid will draw.
    val cellSize = rememberTerminalCell()
    var status by remember { mutableStateOf(row.status) }
    var notice by remember { mutableStateOf<String?>(null) }
    var ended by remember { mutableStateOf<String?>(null) }
    var mode by remember { mutableStateOf(InputMode.Stream) }
    var output by remember { mutableStateOf(OutputMode.Live) }
    var composer by remember { mutableStateOf("") }
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
        PaneTitleBar(row, status, onBack = onBack, onReview = { showReview = true })

        OutputToggle(output) {
            output = it
            // The two halves pair by default: a chat wants a composer, a live
            // terminal wants keystrokes. The input toggle stays right there, so
            // this is a default rather than a decision.
            mode = if (it == OutputMode.Recorded) InputMode.Queue else InputMode.Stream
        }

        BoxWithConstraints(
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
                return@BoxWithConstraints
            }
            // The frame is only as tall as the terminal actually draws, and it
            // sits against the keys. Fitted to the width — which is what
            // mirroring the desktop means — a 167x54 pane fills a little over
            // half the height a phone offers, and a border drawn around the
            // empty rest reads as a window that failed rather than as a pane
            // whose shape is simply not the phone's. With the keyboard up the
            // terminal is the taller of the two and this changes nothing.
            val inset = ShepSpace.small * 2
            val fitted = with(LocalDensity.current) {
                fittedGridHeight(
                    grid.cols, grid.rows, cellSize.width, cellSize.height,
                    (maxWidth - inset).toPx(),
                ).toDp()
            }
            val frameHeight =
                if (fitted.value <= 0f) maxHeight else (fitted + inset).coerceAtMost(maxHeight)
            PaneFrame(
                agent = row.agent,
                state = status,
                blocked = status == "blocked",
                modifier = Modifier
                    .fillMaxWidth()
                    .height(frameHeight)
                    .align(Alignment.BottomCenter),
            ) {
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

        ModeToggle(mode) { mode = it }

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
    onReview: () -> Unit,
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
        Text(row.paneId, style = ShepType.agentName.copy(color = ShepPalette.accent))
        Text(row.workspaceLabel, style = ShepType.paneId, modifier = Modifier.weight(1f))
        ActionText("review", style = ShepType.hint.copy(color = ShepPalette.accent), onClick = onReview)
        StateGlyph(status, style = ShepType.stateGlyphSmall)
    }
}

/** Bordered viewport with the desktop's `agent · state` label on the border. */
@Composable
private fun PaneFrame(
    agent: String,
    state: String,
    blocked: Boolean,
    modifier: Modifier = Modifier.fillMaxSize(),
    content: @Composable () -> Unit,
) {
    val ring = if (blocked) ShepPalette.red else ShepPalette.accent
    Box(modifier) {
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
 * Two toggles, one shape.
 *
 * Output and input each have a live mode and a considered one, and they read as
 * a pair; the `out`/`in` labels are what stop two chips called "live" from
 * being ambiguous.
 */
@Composable
private fun ToggleRow(label: String, content: @Composable () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = ShepSpace.small, vertical = ShepSpace.tight),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = ShepType.viewTitle)
        content()
    }
}

@Composable
private fun OutputToggle(mode: OutputMode, onMode: (OutputMode) -> Unit) {
    ToggleRow("out") {
        ModeChip("live", mode == OutputMode.Live, "out") { onMode(OutputMode.Live) }
        ModeChip("recorded", mode == OutputMode.Recorded, "out") { onMode(OutputMode.Recorded) }
    }
}

@Composable
private fun ModeToggle(mode: InputMode, onMode: (InputMode) -> Unit) {
    ToggleRow("in") {
        ModeChip("live", mode == InputMode.Stream, "in") { onMode(InputMode.Stream) }
        ModeChip("⇥ queue", mode == InputMode.Queue, "in") { onMode(InputMode.Queue) }
    }
}

@Composable
private fun ModeChip(label: String, on: Boolean, side: String = "in", onClick: () -> Unit) {
    // Both rows have a chip called "live"; the side keeps the tags apart.
    ShepChip(label, on, modifier = Modifier.testTag("mode-$side-$label"), onClick = onClick)
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
