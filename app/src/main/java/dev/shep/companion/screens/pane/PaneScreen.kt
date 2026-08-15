package dev.shep.companion.screens.pane

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
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
import dev.shep.companion.statusColorFor
import dev.shep.companion.terminal.GridState
import dev.shep.companion.terminal.KeyBar
import dev.shep.companion.terminal.ShepInputView
import dev.shep.companion.terminal.TerminalGrid
import dev.shep.companion.terminal.TerminalKey
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import dev.shep.companion.ui.components.StateGlyph
import androidx.compose.ui.unit.sp

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
    onReview: () -> Unit = {},
) {
    val grid = remember(row.paneId) { GridState() }
    var status by remember { mutableStateOf(row.status) }
    var notice by remember { mutableStateOf<String?>(null) }
    var ended by remember { mutableStateOf<String?>(null) }
    var mode by remember { mutableStateOf(InputMode.Stream) }
    var output by remember { mutableStateOf(OutputMode.Live) }
    var composer by remember { mutableStateOf("") }
    var showHistory by remember { mutableStateOf(false) }
    var streaming by remember { mutableStateOf(false) }
    var transcript by remember(row.paneId) { mutableStateOf<Transcript?>(null) }
    var transcriptError by remember(row.paneId) { mutableStateOf<String?>(null) }
    var transcriptLoading by remember(row.paneId) { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    // Held so the key bar and the IME can both reach the open channel.
    var send by remember { mutableStateOf<((TerminalKey) -> Unit)>({}) }

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

    if (showHistory) {
        BackHandler { showHistory = false }
        HistoryView(client, row, onBack = { showHistory = false })
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
        PaneTitleBar(row, status, onBack = onBack, onReview = onReview, onHistory = { showHistory = true })

        OutputToggle(output) {
            output = it
            // The two halves pair by default: a chat wants a composer, a live
            // terminal wants keystrokes. The input toggle stays right there, so
            // this is a default rather than a decision.
            mode = if (it == OutputMode.Recorded) InputMode.Queue else InputMode.Stream
        }

        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .padding(horizontal = 10.dp, vertical = 12.dp),
        ) {
            if (output == OutputMode.Recorded) {
                PaneFrame(agent = row.agent, state = status, blocked = status == "blocked") {
                    TranscriptView(
                        transcript = transcript,
                        error = transcriptError,
                        loading = transcriptLoading,
                        onRawScrollback = { showHistory = true },
                        modifier = Modifier.fillMaxSize(),
                    )
                }
                return@Box
            }
            PaneFrame(agent = row.agent, state = status, blocked = status == "blocked") {
                Box(Modifier.fillMaxSize()) {
                    val context = androidx.compose.ui.platform.LocalContext.current
                    val inputView = remember(context) {
                        ShepInputView(context).apply { onKey = { send(it) } }
                    }
                    // Keep the sink pointed at the current channel.
                    inputView.onKey = { send(it) }

                    TerminalGrid(
                        grid = grid,
                        modifier = Modifier.fillMaxSize().testTag("terminal-grid"),
                        onTap = { if (mode == InputMode.Stream) inputView.showKeyboard() },
                    )
                    // Zero-size: it exists only to own the IME connection.
                    AndroidView(factory = { inputView }, modifier = Modifier.size(1.dp))

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
                                .background(ShepPalette.redDim, RoundedCornerShape(6.dp))
                                .padding(horizontal = 10.dp, vertical = 4.dp),
                        )
                    }
                }
            }
        }

        notice?.let {
            Text(
                it,
                style = ShepType.hint.copy(color = ShepPalette.peach),
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }

        ModeToggle(mode) { mode = it }

        when (mode) {
            InputMode.Stream -> KeyBar(onKey = { send(it) })
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
    onHistory: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Text(
            "‹",
            style = ShepType.wordmark.copy(color = ShepPalette.accent),
            modifier = Modifier.clickable { onBack() }.padding(end = 4.dp),
        )
        Text(row.paneId, style = ShepType.agentName.copy(color = ShepPalette.accent))
        Text(row.workspaceLabel, style = ShepType.paneId, modifier = Modifier.weight(1f))
        Text("history", style = ShepType.hint.copy(color = ShepPalette.subtext0),
            modifier = Modifier.clickable { onHistory() }.padding(4.dp))
        Text("review", style = ShepType.hint.copy(color = ShepPalette.accent),
            modifier = Modifier.clickable { onReview() }.padding(4.dp))
        StateGlyph(status, fontSize = 12.sp)
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
                .border(1.5.dp, ring, RoundedCornerShape(16.dp))
                .padding(10.dp),
        ) { content() }
        Text(
            "$agent · $state",
            style = ShepType.badge.copy(color = ring),
            modifier = Modifier
                .align(Alignment.TopStart)
                .padding(start = 14.dp)
                .background(ShepPalette.panelBg)
                .padding(horizontal = 6.dp),
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
            .padding(horizontal = 10.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
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
    Text(
        label,
        style = ShepType.chip.copy(
            color = if (on) ShepPalette.panelBg else ShepPalette.subtext0,
        ),
        modifier = Modifier
            // Both rows have a chip called "live"; the side keeps the tags apart.
            .testTag("mode-$side-$label")
            .background(
                if (on) ShepPalette.accent else ShepPalette.surface0,
                RoundedCornerShape(999.dp),
            )
            .clickable { onClick() }
            .padding(horizontal = 12.dp, vertical = 5.dp),
    )
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
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Box(
            Modifier
                .weight(1f)
                .background(ShepPalette.surface0, RoundedCornerShape(10.dp))
                .border(1.dp, ShepPalette.surface1, RoundedCornerShape(10.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
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
        Text(
            "queue",
            style = ShepType.key.copy(color = ShepPalette.teal),
            modifier = Modifier
                .background(ShepPalette.tealDim, RoundedCornerShape(10.dp))
                .border(1.dp, ShepPalette.teal, RoundedCornerShape(10.dp))
                .clickable { onSend(true) }
                .padding(horizontal = 12.dp, vertical = 9.dp),
        )
        Text(
            "send",
            style = ShepType.key.copy(color = ShepPalette.panelBg),
            modifier = Modifier
                .background(ShepPalette.accent, RoundedCornerShape(10.dp))
                .clickable { onSend(false) }
                .padding(horizontal = 14.dp, vertical = 10.dp),
        )
    }
}
