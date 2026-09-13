package dev.shep.companion.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.shep.companion.AgentRow
import dev.shep.companion.BoardDocket
import dev.shep.companion.BridgeClient
import dev.shep.companion.ChatRole
import dev.shep.companion.ChatTurn
import dev.shep.companion.Docket
import dev.shep.companion.DocketItem
import dev.shep.companion.DocketStatus
import dev.shep.companion.HealthFinding
import dev.shep.companion.HealthLevel
import dev.shep.companion.NeedsYouRow
import dev.shep.companion.OverseerSample
import dev.shep.companion.SessionHost
import dev.shep.companion.SessionTotals
import dev.shep.companion.Tab
import dev.shep.companion.ageCarriedForward
import dev.shep.companion.boardDocket
import dev.shep.companion.docketDueLabel
import dev.shep.companion.formatAge
import dev.shep.companion.looksUnsupported
import dev.shep.companion.needsYou
import dev.shep.companion.parseChatTurn
import dev.shep.companion.parseDocket
import dev.shep.companion.parseDocketItem
import dev.shep.companion.parseOverseerSample
import dev.shep.companion.parseOverview
import dev.shep.companion.proposals
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.ChromeRow
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.ExplainLine
import dev.shep.companion.ui.components.ExplainRow
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.PillHalf
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.StateGlyph
import dev.shep.companion.ui.components.rememberSecondsTicker
import dev.shep.companion.ui.components.rememberSpinnerTick
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import dev.shep.companion.ui.theme.spinnerFrame
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel as CoChannel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/** How often the board re-reads, when no event has told it to. */
private const val BOARD_POLL_MS = 5000L

/** How many chat turns a sample carries. The server keeps 200; this is the tail worth scrolling. */
private const val CHAT_TURNS = 40

/** Skip the opening tick when the situation is younger than this; the desktop's `STALE_SITUATION_SECS`. */
private const val STALE_SITUATION_SECONDS = 60

/**
 * What an `overseer.*` event said, parsed off one `events.subscribe` line.
 *
 * The line is an `EventEnvelope` (src/api/schema/events.rs:355) — `event` names
 * the kind, `data` carries it. Only the chat turn is worth reading in detail:
 * an answer that took forty seconds should appear the moment it lands, not on
 * the next five-second poll. `Updated` is just "go and look again".
 */
sealed interface OverseerEvent {
    data class Chat(val turn: ChatTurn, val pending: Boolean) : OverseerEvent
    data object Updated : OverseerEvent
}

/** One `events.subscribe` line, when it is one of the overseer's two kinds. */
fun parseOverseerEvent(line: JSONObject): OverseerEvent? {
    val data = line.optJSONObject("data") ?: return null
    return when (data.optString("type")) {
        "overseer_chat_turn" -> data.optJSONObject("turn")?.let {
            OverseerEvent.Chat(parseChatTurn(it), data.optBoolean("pending", false))
        }
        "overseer_updated" -> OverseerEvent.Updated
        else -> null
    }
}

/**
 * The chat, after turns arrived from somewhere.
 *
 * Every turn reaches this screen at least twice — once as an
 * `overseer.chat_turn` event, once in the tail of the next `overseer.sample` —
 * and the `you` turn a third time, as the answer to `overseer.chat` itself.
 * `(at, role)` is the identity of a line in `chat.jsonl`, so it is what tells
 * three copies of one turn from three real ones. Order is by time, and ties
 * keep the order they arrived in, because a question and its answer can land
 * inside the same second and the question still came first.
 */
fun mergeChat(existing: List<ChatTurn>, incoming: List<ChatTurn>): List<ChatTurn> {
    val seen = existing.map { it.at to it.role }.toMutableSet()
    val merged = existing.toMutableList()
    incoming.forEach { turn ->
        if (seen.add(turn.at to turn.role)) merged.add(turn)
    }
    return merged.sortedBy { it.at }
}

/**
 * Whether the board should keep asking for `overseer.sample`.
 *
 * Only the server saying it has never heard of the method turns this off; a
 * timeout or a dropped packet leaves it exactly as it was, and a later success
 * turns it back on. Latching on any error would mean one bad moment costs the
 * board its whole right-hand side until the app is restarted.
 */
fun overseerSupported(previous: Boolean, error: String?): Boolean = when {
    error == null -> true
    looksUnsupported(error) -> false
    else -> previous
}

/**
 * The board: the phone's copy of `src/ui/overseer.rs`, which is what the
 * desktop opens on.
 *
 * The regions are stacked in the desktop's narrow order
 * (`overseer_layout`, src/ui/overseer.rs:932-938) — what needs you, then what
 * is running, then what is owed, then whether the machine is well, and then
 * the three regions that are the overseer's own voice: its read of the room,
 * what it proposes, and the chat. One column, because a phone is always the
 * narrow case.
 *
 * Everything here is read. The only two things this screen writes are a
 * proposal's fate and a question to the overseer — and the overseer never
 * types into a pane, so nothing on this board can act on an agent by accident.
 */
@Composable
fun BoardScreen(
    client: BridgeClient,
    onOpenPane: (AgentRow) -> Unit,
    onSelectTab: (Tab) -> Unit,
) {
    var sample by remember { mutableStateOf<OverseerSample?>(null) }
    var docket by remember { mutableStateOf<Docket?>(null) }
    var rows by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var totals by remember { mutableStateOf(SessionTotals()) }
    var host by remember { mutableStateOf(SessionHost()) }
    var chat by remember { mutableStateOf<List<ChatTurn>>(emptyList()) }
    var chatPending by remember { mutableStateOf(false) }
    var composer by remember { mutableStateOf("") }
    var notice by remember { mutableStateOf<String?>(null) }
    var supported by remember { mutableStateOf(true) }
    // The same word the agents header carries, read off the same call. The
    // board is where the phone lands, so this is the first — and on a quiet
    // session the only — place a dropped bridge can show itself.
    var status by remember { mutableStateOf("connecting") }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val actions = remember(client, scope) { DocketActions(client, scope) }
    val refreshSignal = remember { CoChannel<Unit>(CoChannel.CONFLATED) }
    val nowElapsedMs by rememberSecondsTicker()
    var statedAtMs by remember { mutableStateOf(0L) }

    // Poll only while somebody is looking. The tab is disposed when it is not
    // current, and this covers the app going to the background with it open.
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

    suspend fun refresh() {
        if (supported) {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    client.call("overseer.sample", JSONObject().put("chat_turns", CHAT_TURNS))
                }
            }
            result.onSuccess {
                val read = parseOverseerSample(it)
                sample = read
                chat = mergeChat(chat, read.chat)
                chatPending = read.chatPending
            }
            supported = overseerSupported(supported, result.exceptionOrNull()?.message)
        }
        withContext(Dispatchers.IO) { runCatching { client.call("docket.list") } }
            .onSuccess { docket = parseDocket(it) }
        withContext(Dispatchers.IO) { runCatching { client.call("session.overview") } }
            .onSuccess { result ->
                val overview = parseOverview(result)
                if (overview == null) {
                    status = RECONNECTING
                } else {
                    rows = overview.agents
                    totals = overview.totals
                    host = overview.host
                    statedAtMs = android.os.SystemClock.elapsedRealtime()
                    status = "live · shep ${overview.host.version ?: ""}".trim()
                }
            }
            .onFailure {
                // A server too old to answer `session.overview` is still a
                // server that is answering; only a transport failure is a
                // dropped connection. `ChannelsScreen` draws the same line.
                status = if (looksUnsupported(it.message)) {
                    "live · shep ${client.serverVersion ?: ""}".trim()
                } else {
                    RECONNECTING
                }
            }
    }

    // One channel for both overseer events. A chat turn is applied straight
    // away — an answer that took forty seconds should not then wait five more
    // for a poll — and everything else is a nudge to re-read.
    LaunchedEffect(client) {
        val subs = JSONArray()
            .put(JSONObject().put("type", "overseer.chat_turn"))
            .put(JSONObject().put("type", "overseer.updated"))
        val channel = client.openChannel(
            "events.subscribe",
            JSONObject().put("subscriptions", subs),
            object : BridgeClient.ChannelListener {
                override fun onLine(line: JSONObject) {
                    when (val event = parseOverseerEvent(line)) {
                        is OverseerEvent.Chat -> {
                            chat = mergeChat(chat, listOf(event.turn))
                            chatPending = event.pending
                        }
                        OverseerEvent.Updated -> refreshSignal.trySend(Unit)
                        null -> {}
                    }
                }
                override fun onClosed(error: String?) {} // socket reconnect handled upstream
            },
        )
        try {
            for (signal in refreshSignal) {
                delay(100) // coalesce event bursts
                refresh()
            }
        } finally {
            client.closeChannel(channel)
        }
    }

    LaunchedEffect(client, active) {
        if (!active) return@LaunchedEffect
        // Ask for a fresh situation on arriving, and let it age out on its own
        // after that: the tick is the expensive half of the overseer, and the
        // desktop's own board guards it the same way.
        withContext(Dispatchers.IO) {
            runCatching {
                client.call(
                    "overseer.tick",
                    JSONObject().put("max_age_seconds", STALE_SITUATION_SECONDS),
                )
            }
        }
        while (isActive) {
            refresh()
            delay(BOARD_POLL_MS)
        }
    }

    fun send() {
        val text = composer.trim()
        if (text.isEmpty()) return
        composer = ""
        scope.launch {
            withContext(Dispatchers.IO) {
                runCatching { client.call("overseer.chat", JSONObject().put("text", text)) }
            }
                .onSuccess { result ->
                    result.optJSONObject("turn")?.let {
                        chat = mergeChat(chat, listOf(parseChatTurn(it)))
                    }
                    chatPending = true
                }
                .onFailure {
                    // The refused question goes back in the field rather than
                    // into the void: it is still the thing you wanted to ask.
                    composer = text
                    notice = it.message ?: "the overseer could not be asked"
                }
        }
    }

    val read = sample
    val currentDocket = docket
    val proposalRows = currentDocket?.let { proposals(it) }.orEmpty()
    val boardRows = currentDocket?.let { boardDocket(it, proposalRows) }
    val waiting = needsYou(rows)
    val sinceMs = nowElapsedMs - statedAtMs
    // Re-read off the wall clock once a second, driven by the same ticker the
    // ages are: a chat turn's age is unix seconds against now.
    val nowSeconds = remember(nowElapsedMs) { System.currentTimeMillis() / 1000L }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("board") { ConnectionLine(status) }
        ChromeRow(
            totals = totals,
            proposals = proposalRows.size,
            current = PillHalf.Board,
            onSelect = { if (it == PillHalf.Desktop) onSelectTab(Tab.Agents) },
        )
        if (!supported) {
            Notice("overseer needs a newer shep on the computer")
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        HeaderFacts(read, host)

        LazyColumn(
            Modifier.fillMaxWidth().weight(1f),
            contentPadding = PaddingValues(bottom = ShepSpace.small),
        ) {
            if (waiting.isNotEmpty()) {
                item(key = "needs-you") {
                    RegionHeading("needs you", waiting.size.toString())
                }
                items(waiting, key = { "needs:" + it.row.paneId }) { row ->
                    NeedsYouCard(row, sinceMs) { onOpenPane(row.row) }
                }
            }

            item(key = "agents") { RegionHeading("agents", rows.size.toString()) }
            if (rows.isEmpty()) {
                item(key = "agents-empty") { EmptyState("no agents running") }
            } else {
                items(rows, key = { "agent:" + it.paneId }) { row ->
                    AgentTableRow(row, sinceMs) { onOpenPane(row) }
                }
            }

            item(key = "docket") { DocketHeading(boardRows) }
            if (boardRows == null || boardRows.rows.isEmpty()) {
                item(key = "docket-empty") { DimLine("nothing due, inbox empty") }
            } else {
                items(boardRows.rows, key = { "docket:" + it.id }) { item ->
                    DocketBoardRow(item, currentDocket?.today ?: "") { onSelectTab(Tab.Docket) }
                }
            }

            if (read != null && read.health.isNotEmpty()) {
                item(key = "health") { RegionHeading("health", "") }
                item(key = "health-row") {
                    HealthRow(read.health) { notice = it }
                }
            }

            if (read != null && !read.pluginLinked) {
                item(key = "no-plugin") {
                    EmptyState(
                        "link the overseer plugin",
                        body = "shep plugin link plugins/overseer",
                    )
                }
            } else {
                item(key = "room") {
                    RegionHeading(
                        "${ShepSemantic.overseer.glyph} read of the room",
                        "",
                        hint = read?.tickAt,
                        color = ShepSemantic.overseer.color,
                    )
                }
                item(key = "room-body") { ReadOfTheRoom(read) }

                if (proposalRows.isNotEmpty()) {
                    item(key = "proposals") {
                        RegionHeading(
                            "proposals",
                            proposalRows.size.toString(),
                            hint = "· inbox only, you dispose",
                            color = ShepPalette.teal,
                        )
                    }
                    items(proposalRows, key = { "proposal:" + it.id }) { item ->
                        ProposalRow(
                            item = item,
                            onKeep = {
                                actions.keep(
                                    item.id,
                                    onItem = { returned -> swapDocket(currentDocket, returned) { docket = it } },
                                    onSuccess = { notice = it; scope.launch { refresh() } },
                                    onFailure = { notice = it },
                                )
                            },
                            onDrop = {
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                actions.drop(
                                    item.id,
                                    onItem = { returned -> swapDocket(currentDocket, returned) { docket = it } },
                                    onSuccess = { notice = it; scope.launch { refresh() } },
                                    onFailure = { notice = it },
                                )
                            },
                        )
                    }
                }

                item(key = "chat") {
                    RegionHeading(
                        "chat",
                        "",
                        hint = listOfNotNull(
                            read?.runtime?.let { "· $it headless" },
                            "· never types into a pane",
                        ).joinToString(" "),
                    )
                }
                items(chat, key = { "chat:${it.at}:${it.role.wire}" }) { turn ->
                    ChatTurnRow(turn, nowSeconds)
                }
                if (chatPending) {
                    item(key = "chat-pending") { ChatPending() }
                }
                item(key = "chat-explain") {
                    ExplainRow("is this the same conversation as the desk?") {
                        ExplainLine(
                            "one thread",
                            "the board's chat and the overseer's own session pane resume the " +
                                "same claude conversation, so a question asked here is a " +
                                "question it remembers there.",
                        )
                    }
                }
            }
        }

        if (read == null || read.pluginLinked) {
            BoardComposer(
                value = composer,
                onValue = { composer = it },
                runtime = read?.runtime,
                onSend = { send() },
            )
        }
    }
}

/** Swap one mutated docket row into the list so it moves lane before the next poll. */
private fun swapDocket(current: Docket?, returned: JSONObject, onDocket: (Docket) -> Unit) {
    current ?: return
    val item = parseDocketItem(returned, current.today)
    onDocket(current.copy(items = listOf(item) + current.items.filterNot { it.id == item.id }))
}

/**
 * The header's facts: `tick hh:mm · brain 4m ago · claude headless · shep
 * 0.7.3 · load 26 % of 12 · mem 17 %`.
 *
 * The desktop's `header_strip` (src/ui/overseer.rs:795) drops whole facts as
 * the terminal narrows; a phone scrolls instead, which keeps every fact
 * reachable without a ladder of rungs to maintain.
 *
 * The vitals are overlay0 at every value, with no colour ladder — deliberately.
 * `DashboardStrip` used to warm them yellow and red, which said a busy laptop
 * was more urgent than a blocked agent.
 */
@Composable
private fun HeaderFacts(sample: OverseerSample?, host: SessionHost) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val facts = buildList {
            sample?.tickAt?.let { add(Triple("tick ", it, false)) }
            sample?.brainAgeSeconds?.let { add(Triple("brain ", "${formatAge(it)} ago", false)) }
            sample?.runtime?.let { add(Triple(null, "$it headless", true)) }
            host.version?.let { add(Triple("shep ", it, true)) }
            host.loadPercent?.let { load ->
                add(Triple(null, "load $load %" + (host.cores?.let { " of $it" } ?: ""), true))
            }
            host.memoryPercent?.let { add(Triple(null, "mem $it %", true)) }
        }
        facts.forEachIndexed { index, (label, value, dim) ->
            if (index > 0) {
                Text(" · ", style = ShepType.metaSmall.copy(color = ShepPalette.surface1))
            }
            label?.let { Text(it, style = ShepType.metaSmall.copy(color = ShepPalette.subtext0)) }
            Text(
                value,
                style = ShepType.metaSmall.copy(
                    color = if (dim) ShepPalette.overlay0 else ShepPalette.text,
                ),
            )
        }
    }
}

/**
 * A region's heading: the title bold in its own ink, the count beside it, a
 * hint after that.
 *
 * `docs/DESIGN-LANGUAGE.md:222` — headings in bold text with a count in
 * overlay0. The two regions that are the overseer's own voice carry its
 * colours: mauve for the read of the room, teal for proposals (the queued
 * tier, because a proposal is waiting on you and nothing has happened yet).
 */
@Composable
private fun RegionHeading(
    title: String,
    count: String,
    hint: String? = null,
    color: Color = ShepPalette.text,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.snug),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        Text(
            title,
            style = ShepType.sectionLabel.copy(color = color, fontWeight = FontWeight.Bold),
        )
        if (count.isNotEmpty()) Text(count, style = ShepType.meta.copy(color = ShepPalette.text))
        hint?.takeIf { it.isNotEmpty() }?.let {
            Text(
                it,
                style = ShepType.metaSmall.copy(color = ShepPalette.overlay1),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

/** The docket heading's three counts: `docket  due 2 !1 · inbox 5` (src/ui/overseer.rs:1166-1182). */
@Composable
private fun DocketHeading(board: BoardDocket?) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.snug),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text("docket", style = ShepType.sectionLabel.copy(fontWeight = FontWeight.Bold))
        Text("  due ", style = ShepType.metaSmall.copy(color = ShepPalette.overlay1))
        Text("${board?.due ?: 0}", style = ShepType.meta.copy(color = ShepPalette.text))
        if ((board?.overdue ?: 0) > 0) {
            Text(" !${board?.overdue}", style = ShepType.badge.copy(color = ShepPalette.peach))
        }
        Text("  ·  inbox ", style = ShepType.metaSmall.copy(color = ShepPalette.overlay1))
        Text("${board?.inbox ?: 0}", style = ShepType.meta.copy(color = ShepPalette.text))
    }
}

/** A region's "there is nothing here" line, in the desktop's own words. */
@Composable
private fun DimLine(text: String) {
    Text(
        text,
        style = ShepType.emptyState,
        modifier = Modifier.padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
    )
}

/**
 * One row of `needs you`: who, how long, and what to do about it.
 *
 * Two lines, as the desktop draws it (`render_needs_you`,
 * src/ui/overseer.rs:1042): name and group with the state and its age pinned
 * right, then what the agent said with the one thing left to do about it.
 * `↑N not pushed` is green because commits waiting to go out are settled work,
 * not a warning.
 */
@Composable
private fun NeedsYouCard(entry: NeedsYouRow, sinceMs: Long, onClick: () -> Unit) {
    val row = entry.row
    Column(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StateGlyph(
                row.status,
                style = ShepType.stateGlyphSmall,
                manualTier = row.manualState?.tier,
                manualLabel = row.manualState?.label,
            )
            Spacer(Modifier.width(ShepSpace.small))
            Text(
                "${row.displayName ?: row.agent} · ${row.workspaceLabel}",
                style = ShepType.itemName,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                listOfNotNull(
                    row.manualState?.label ?: row.status,
                    ageCarriedForward(row.stateAgeSeconds, sinceMs)?.let { formatAge(it) },
                ).joinToString(" "),
                style = ShepType.state.copy(color = ShepSemantic.agentColor(row.status)),
            )
            row.location?.let {
                Spacer(Modifier.width(ShepSpace.small))
                Text(it, style = ShepType.metaSmall)
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                entry.detail,
                style = ShepType.metaSmall.copy(color = ShepPalette.subtext0),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(ShepSpace.small))
            when {
                entry.blocked -> Text(
                    "→ answer it",
                    style = ShepType.metaSmall.copy(color = ShepPalette.overlay1),
                )
                (entry.ahead ?: 0) > 0 -> Text(
                    "↑${entry.ahead} not pushed",
                    style = ShepType.badge.copy(color = ShepPalette.green),
                )
                else -> Text(
                    "→ look",
                    style = ShepType.metaSmall.copy(color = ShepPalette.overlay1),
                )
            }
        }
    }
}

/**
 * One row of the agents table: glyph, name, group, state and age, then the
 * branch with the context gauge pinned right (`render_agents`,
 * src/ui/overseer.rs:1068).
 */
@Composable
private fun AgentTableRow(row: AgentRow, sinceMs: Long, onClick: () -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StateGlyph(
                row.status,
                style = ShepType.stateGlyphSmall,
                manualTier = row.manualState?.tier,
                manualLabel = row.manualState?.label,
            )
            Spacer(Modifier.width(ShepSpace.small))
            Text(
                row.displayName ?: row.agent,
                style = ShepType.itemName,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.width(ShepSpace.small))
            Text(
                row.workspaceLabel,
                style = ShepType.itemLabel,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(
                listOfNotNull(
                    row.manualState?.label ?: row.status,
                    ageCarriedForward(row.stateAgeSeconds, sinceMs)?.let { formatAge(it) },
                ).joinToString(" "),
                style = ShepType.state.copy(color = ShepSemantic.agentColor(row.status)),
            )
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                row.branch.orEmpty(),
                style = ShepType.metaSmall.copy(color = ShepPalette.mauve),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            row.contextPercent?.let {
                Spacer(Modifier.width(ShepSpace.small))
                ContextGauge(it)
            }
        }
    }
}

/** One docket row on the board: glyph, `#id`, title, and what it is due. */
@Composable
private fun DocketBoardRow(item: DocketItem, today: String, onClick: () -> Unit) {
    val look = ShepSemantic.docket(item.status.wire, item.overdue, item.dueToday)
    Row(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(look.glyph, style = ShepType.stateGlyphSmall.copy(color = look.color))
        Spacer(Modifier.width(ShepSpace.small))
        Text("#${item.id} ", style = ShepType.metaSmall)
        Text(
            item.title,
            style = ShepType.meta.copy(color = ShepPalette.text),
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Spacer(Modifier.width(ShepSpace.small))
        Text(
            if (item.status == DocketStatus.Inbox) {
                "inbox"
            } else {
                docketDueLabel(item.due, today) + (item.repeat?.let { " · $it" } ?: "")
            },
            style = ShepType.metaSmall.copy(
                color = if (item.status == DocketStatus.Inbox) ShepPalette.overlay0 else look.color,
            ),
        )
    }
}

/**
 * The health strip: one fact per check, wrapping rather than dropping.
 *
 * The desktop drops whole findings off the end when the terminal narrows
 * (`render_health`, src/ui/overseer.rs:1276) because a terminal row cannot
 * wrap; a phone can, and a warning that is simply not drawn is the one you
 * needed to see. A long press on a warning or a failure shows what would fix
 * it, when the check knows.
 */
@OptIn(ExperimentalLayoutApi::class, ExperimentalFoundationApi::class)
@Composable
private fun HealthRow(findings: List<HealthFinding>, onFix: (String) -> Unit) {
    FlowRow(
        Modifier.fillMaxWidth().padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.medium),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.tight),
    ) {
        findings.forEach { finding ->
            val look = ShepSemantic.health(finding.level.wire)
            val text = if (finding.level == HealthLevel.Ok) {
                "${look.glyph} ${finding.check}"
            } else {
                "${look.glyph} ${finding.check} ${finding.detail}".trimEnd()
            }
            Text(
                text,
                style = ShepType.metaSmall.copy(
                    color = if (finding.level == HealthLevel.Ok) {
                        ShepPalette.subtext0
                    } else {
                        look.color
                    },
                ),
                modifier = Modifier.combinedClickable(
                    onClick = {},
                    onLongClick = { finding.fix?.let { onFix("${finding.check}: $it") } },
                ),
            )
        }
    }
}

/**
 * The narrative, as paragraphs.
 *
 * The one place on these screens that is prose rather than shep talking about
 * itself, so it is the one place in sans. A deterministic board — the tick's
 * own template, written when no brain answered — says so, because "the
 * overseer thinks" and "a template filled itself in" are different claims.
 */
@Composable
private fun ReadOfTheRoom(sample: OverseerSample?) {
    val lines = sample?.narrative.orEmpty()
    if (lines.isEmpty()) {
        DimLine("the overseer has not spoken yet")
        return
    }
    Column(
        Modifier.padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.tight),
    ) {
        lines.forEach { Text(it, style = ShepType.body) }
        if (sample?.source == "deterministic") {
            Text("no brain answered — this is the tick's own summary", style = ShepType.metaSmall)
        }
    }
}

/**
 * One proposal: what the overseer wants written down, and the two words that
 * dispose of it (`render_proposals`, src/ui/overseer.rs:1374).
 *
 * `keep` promotes it onto the slated lane; `drop` discards it. Nothing here
 * happens on its own — the overseer proposes into the inbox and you dispose,
 * which is the whole reason this is a region and not a notification.
 */
@Composable
private fun ProposalRow(item: DocketItem, onKeep: () -> Unit, onDrop: () -> Unit) {
    val look = ShepSemantic.docket(DocketStatus.Inbox.wire)
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(look.glyph, style = ShepType.stateGlyphSmall.copy(color = look.color))
            Spacer(Modifier.width(ShepSpace.small))
            Text("#${item.id} ", style = ShepType.metaSmall)
            Text(
                item.title,
                style = ShepType.meta.copy(color = ShepPalette.text),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            ActionText(
                "keep",
                style = ShepType.action.copy(color = ShepPalette.green),
                onClick = onKeep,
            )
            ActionText(
                "drop",
                style = ShepType.action.copy(color = ShepPalette.red),
                onClick = onDrop,
            )
        }
        val second = item.sourceLabel?.let { "from  $it" }
            ?: item.notes?.lineSequence()?.firstOrNull()?.trim()?.takeIf { it.isNotEmpty() }
        second?.let {
            Text(it, style = ShepType.metaSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/**
 * One chat turn: who said it, how long ago, and what they said, with the text
 * hanging under itself rather than under the name.
 */
@Composable
private fun ChatTurnRow(turn: ChatTurn, nowSeconds: Long) {
    val you = turn.role == ChatRole.You
    val ageSeconds = (nowSeconds - turn.at).coerceAtLeast(0L)
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
    ) {
        Text(
            if (you) "you" else ShepSemantic.overseer.glyph,
            style = ShepType.metaSmall.copy(
                color = if (you) ShepPalette.accent else ShepSemantic.overseer.color,
                fontWeight = FontWeight.Bold,
            ),
        )
        Spacer(Modifier.width(ShepSpace.small))
        Text(formatAge(ageSeconds), style = ShepType.metaSmall)
        Spacer(Modifier.width(ShepSpace.small))
        Text(
            turn.text,
            style = ShepType.body.copy(
                color = if (you) ShepPalette.text else ShepPalette.subtext0,
            ),
            modifier = Modifier.weight(1f),
        )
    }
}

/** The question is out with the runtime; its answer is an event away. */
@Composable
private fun ChatPending() {
    val tick by rememberSpinnerTick()
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            "${spinnerFrame(tick)} thinking…",
            style = ShepType.metaSmall.copy(color = ShepPalette.yellow),
        )
    }
}

/**
 * The board's composer. `›` leads it because the question goes to the
 * overseer, not to an agent — there is no pane on the other end of this field.
 */
@Composable
private fun BoardComposer(
    value: String,
    onValue: (String) -> Unit,
    runtime: String?,
    onSend: () -> Unit,
) {
    Column(Modifier.imePadding()) {
        if (runtime == null) {
            Text(
                "no headless runtime: set [plugins.overseer] runtime",
                style = ShepType.metaSmall.copy(color = ShepPalette.peach),
                modifier = Modifier
                    .fillMaxWidth()
                    .background(ShepPalette.surfaceDim)
                    .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
            )
        }
        ComposerField(
            value = value,
            onValue = onValue,
            placeholder = "ask the overseer",
            enabled = runtime != null,
            lead = "›",
            testTag = "overseer-composer",
        ) {
            ComposerButton(
                label = "send",
                ink = ShepPalette.panelBg,
                background = ShepPalette.accent,
                enabled = runtime != null,
                onClick = onSend,
            )
        }
    }
}

/**
 * A one-line text field with its own buttons beside it.
 *
 * Lifted out of the pane's queue composer, which was the only field in the app
 * that got the details right — the surface0 well with a surface1 hairline, the
 * copper cursor, the placeholder that is a sibling rather than a decoration —
 * and is now one of two. [buttons] is whatever the caller sends the text with,
 * because that is the only part that differs: the pane has `queue` and `send`,
 * the board has `send` alone.
 */
@Composable
fun ComposerField(
    value: String,
    onValue: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    lead: String? = null,
    testTag: String = "composer",
    buttons: @Composable RowScope.() -> Unit,
) {
    Row(
        modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        Row(
            Modifier
                .weight(1f)
                .clip(ShepShape.field)
                .background(ShepPalette.surface0)
                .border(ShepSize.border, ShepPalette.surface1, ShepShape.field)
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            lead?.let {
                Text(it, style = ShepType.hint.copy(color = ShepPalette.accent))
                Spacer(Modifier.width(ShepSpace.snug))
            }
            Box(Modifier.weight(1f)) {
                if (value.isEmpty()) {
                    Text(placeholder, style = ShepType.hint.copy(color = ShepPalette.overlay0))
                }
                BasicTextField(
                    value = value,
                    onValueChange = onValue,
                    enabled = enabled,
                    textStyle = ShepType.hint.copy(color = ShepPalette.text),
                    cursorBrush = SolidColor(ShepPalette.accent),
                    modifier = Modifier.fillMaxWidth().testTag(testTag),
                )
            }
        }
        buttons()
    }
}

/** One of a [ComposerField]'s buttons. */
@Composable
fun ComposerButton(
    label: String,
    ink: Color,
    background: Color,
    border: Color? = null,
    enabled: Boolean = true,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .minimumInteractiveComponentSize()
            .clip(ShepShape.field)
            .background(if (enabled) background else ShepPalette.surface0)
            .then(
                if (border == null) {
                    Modifier
                } else {
                    Modifier.border(ShepSize.border, border, ShepShape.field)
                }
            )
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label,
            style = ShepType.key.copy(color = if (enabled) ink else ShepPalette.overlay0),
        )
    }
}
