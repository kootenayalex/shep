package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.EXPECTED_PROTOCOL
import dev.shep.companion.SessionHost
import dev.shep.companion.SessionTotals
import dev.shep.companion.parseOverview
import dev.shep.companion.parseSnapshot
import dev.shep.companion.statusColor
import dev.shep.companion.totalsFromRows
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.NoticeTone
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

// Structural subscriptions (no pane arg) that signal the agent list changed
// shape; a re-snapshot on any of these keeps the board in sync without
// per-pane subs.
private val STRUCTURAL_SUBSCRIPTIONS = listOf(
    "workspace.updated", "workspace.created", "workspace.closed", "workspace.renamed",
    "pane.created", "pane.closed", "pane.exited", "pane.agent_detected", "layout.updated",
)

/** The board's filter chips. "attention" (blocked + done-unseen) is the default. */
enum class HomeFilter(val label: String) {
    Attention("attention"),
    All("all"),
    Blocked("blocked"),
    Working("working"),
    Idle("idle");

    fun accepts(status: String): Boolean = when (this) {
        All -> true
        Attention -> status == "blocked" || status == "done"
        Blocked -> status == "blocked"
        Working -> status == "working"
        Idle -> status == "idle"
    }
}

@Composable
fun BoardScreen(client: BridgeClient, onOpenPane: (AgentRow) -> Unit, onUnpair: () -> Unit) {
    var rows by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var totals by remember { mutableStateOf(SessionTotals()) }
    var host by remember { mutableStateOf(SessionHost()) }
    var status by remember { mutableStateOf("connecting") }
    var filter by remember { mutableStateOf(HomeFilter.Attention) }
    var showNew by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf<AgentRow?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    // Event-driven refresh: subscribe to structural events + a per-pane
    // agent_status_changed sub for each live pane, and re-snapshot on any event
    // (bridge relays each as a channel line). Keepalive poll is only a backstop.
    LaunchedEffect(client) {
        val refreshSignal = Channel<Unit>(Channel.CONFLATED)
        var subscribedPanes: Set<String> = emptySet()
        var subChannel = -1L

        // `session.overview` answers the whole board in one call. A server
        // older than that method errors, and we fall back to the snapshot —
        // fewer facts per card, but every agent still shows up. Latch the
        // fallback so one probe per connection is enough.
        var overviewSupported = true

        suspend fun refresh() {
            if (overviewSupported) {
                val result = withContext(Dispatchers.IO) {
                    runCatching { client.call("session.overview") }
                }
                val overview = result.getOrNull()?.let { parseOverview(it) }
                if (overview != null) {
                    rows = overview.agents
                    totals = overview.totals
                    host = overview.host
                    status = "live · shep ${overview.host.version ?: ""}".trim()
                    return
                }
                overviewSupported = false
            }
            val result = withContext(Dispatchers.IO) {
                runCatching { client.call("session.snapshot") }
            }
            result.onSuccess {
                rows = parseSnapshot(it)
                totals = totalsFromRows(rows)
                host = SessionHost(version = client.serverVersion)
                status = "live · shep ${client.serverVersion ?: ""}".trim()
            }.onFailure { status = "reconnect: ${it.message}" }
        }

        fun subscribe(paneIds: Set<String>) {
            if (subChannel >= 0) client.closeChannel(subChannel)
            val subs = JSONArray()
            STRUCTURAL_SUBSCRIPTIONS.forEach { subs.put(JSONObject().put("type", it)) }
            paneIds.forEach {
                subs.put(JSONObject().put("type", "pane.agent_status_changed").put("pane_id", it))
            }
            subChannel = client.openChannel(
                "events.subscribe",
                JSONObject().put("subscriptions", subs),
                object : BridgeClient.ChannelListener {
                    override fun onLine(line: JSONObject) { refreshSignal.trySend(Unit) }
                    override fun onClosed(error: String?) {} // socket reconnect handled upstream
                },
            )
            subscribedPanes = paneIds
        }

        refresh()
        subscribe(rows.map { it.paneId }.toSet())
        val keepalive = launch {
            while (isActive) { delay(15000); refreshSignal.trySend(Unit) }
        }
        try {
            for (signal in refreshSignal) {
                delay(100) // coalesce event bursts
                refresh()
                val current = rows.map { it.paneId }.toSet()
                if (current != subscribedPanes) subscribe(current)
            }
        } finally {
            keepalive.cancel()
            if (subChannel >= 0) client.closeChannel(subChannel)
        }
    }

    val visibleRows = rows.filter { filter.accepts(it.status) }
    // Directories already in play seed the new-session picker, so starting a
    // second session where you are working is two taps and no typing.
    val recentRepos = rows.mapNotNull { it.cwd }.distinct()

    /**
     * Open a session the way a person does at the desktop: make a workspace
     * rooted at `cwd`, then run the runtime in the shell it already gave you.
     *
     * `agent.start` is deliberately not used here. With a `workspace_id` it
     * *splits* into the workspace, which would leave the fresh root shell
     * sitting next to the agent; with `new_workspace` it cannot carry a label.
     * Typing the command into the root pane costs one call, leaves exactly one
     * pane, and is the same path shep's own detection is built to watch — so a
     * "terminal" is simply this flow with nothing typed.
     */
    fun startSession(cwd: String, name: String, runtime: SessionRuntime) {
        scope.launch {
            notice = "starting ${runtime.label}…"
            withContext(Dispatchers.IO) {
                runCatching {
                    val created = client.call(
                        "workspace.create",
                        JSONObject()
                            .put("cwd", cwd)
                            .put("focus", true)
                            .apply { if (name.isNotBlank()) put("label", name) },
                    )
                    val paneId = created.optJSONObject("root_pane")
                        ?.optString("pane_id")
                        ?.takeIf { it.isNotEmpty() }
                        ?: throw IllegalStateException("workspace.create returned no root pane")
                    if (runtime.argv.isNotEmpty()) {
                        client.call(
                            "pane.send_text",
                            JSONObject()
                                .put("pane_id", paneId)
                                .put("text", runtime.argv.joinToString(" ") + "\n"),
                        )
                    }
                    // Name the agent too, not just the workspace: the board's
                    // first line reads the agent name, and that is the line
                    // that has to stop saying "claude" five times over. Best
                    // effort — the runtime may not be detected yet, and a
                    // session that started without its name is still started.
                    if (name.isNotBlank()) {
                        runCatching {
                            client.call(
                                "agent.rename",
                                JSONObject().put("target", paneId).put("name", name),
                            )
                        }
                    }
                }
            }
                .onSuccess { notice = "started ${runtime.label}" }
                .onFailure { notice = "could not start: ${it.message}" }
        }
    }

    fun renameSession(row: AgentRow, name: String) {
        scope.launch {
            withContext(Dispatchers.IO) {
                runCatching {
                    client.call(
                        "agent.rename",
                        JSONObject().put("target", row.paneId)
                            .apply { if (name.isNotBlank()) put("name", name) },
                    )
                }
            }
                .onSuccess { notice = if (name.isBlank()) "name reset" else "renamed to $name" }
                .onFailure { notice = "rename failed: ${it.message}" }
        }
    }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("board") {
            Text(status, style = ShepType.meta)
            Spacer(Modifier.width(ShepSpace.small))
            ActionText("+ new", style = ShepType.actionStrong) { showNew = true }
            ActionText("unpair", style = ShepType.meta, onClick = onUnpair)
        }
        val serverProtocol = client.serverProtocol
        if (serverProtocol != null && serverProtocol != EXPECTED_PROTOCOL) {
            Notice(
                "⚠ server protocol $serverProtocol · app built for $EXPECTED_PROTOCOL — " +
                    if (serverProtocol > EXPECTED_PROTOCOL) "update the app" else "update the server",
                tone = NoticeTone.Alert,
            )
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        DashboardStrip(totals, host) { statusColor(it) }
        FilterChips(filter, rows) { filter = it }
        if (visibleRows.isEmpty()) {
            EmptyState(
                if (rows.isEmpty()) "no sessions — start one with + new"
                else "nothing ${filter.label}"
            )
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.listGutter),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(visibleRows, key = { it.paneId }) { row ->
                    BoardCard(
                        row = row,
                        statusColor = { statusColor(it) },
                        displayName = row.displayName ?: row.agent,
                        onLongClick = { renaming = row },
                        onClick = { onOpenPane(row) },
                    )
                }
            }
        }
    }

    if (showNew) {
        NewSessionSheet(
            recentRepos = recentRepos,
            onDismiss = { showNew = false },
            onStart = { cwd, name, runtime ->
                showNew = false
                startSession(cwd, name, runtime)
            },
        )
    }
    renaming?.let { row ->
        RenameSessionSheet(
            row = row,
            onDismiss = { renaming = null },
            onRename = { name ->
                renaming = null
                renameSession(row, name)
            },
        )
    }
}

@Composable
fun FilterChips(selected: HomeFilter, rows: List<AgentRow>, onSelect: (HomeFilter) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        HomeFilter.entries.forEach { entry ->
            val count = rows.count { entry.accepts(it.status) }
            ShepChip("${entry.label} $count", entry == selected) { onSelect(entry) }
        }
    }
}
