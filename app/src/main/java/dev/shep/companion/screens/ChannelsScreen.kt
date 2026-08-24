package dev.shep.companion.screens

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextOverflow
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.EXPECTED_PROTOCOL
import dev.shep.companion.PaneNode
import dev.shep.companion.SessionHost
import dev.shep.companion.SessionTotals
import dev.shep.companion.SpaceNode
import dev.shep.companion.asAgentRow
import dev.shep.companion.formatAge
import dev.shep.companion.parseOverview
import dev.shep.companion.parseSnapshot
import dev.shep.companion.parseTree
import dev.shep.companion.repoName
import dev.shep.companion.statusColor
import dev.shep.companion.totalsFromRows
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.Meter
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.NoticeTone
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.components.StateGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.Channel as CoChannel
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

/**
 * Everything that changes what this list says.
 *
 * The board watched agent state and the spaces tree watched structure, and each
 * re-read on its own half. One list needs both sets, plus a per-pane status
 * subscription for the agents currently in it.
 */
private val CHANNEL_SUBSCRIPTIONS = listOf(
    "workspace.created", "workspace.closed", "workspace.renamed",
    "workspace.moved", "workspace.focused", "workspace.updated",
    "tab.created", "tab.closed", "tab.renamed", "tab.moved", "tab.focused",
    "pane.created", "pane.closed", "pane.focused", "pane.moved", "pane.exited",
    "pane.agent_detected", "layout.updated",
)

/** Blocked, or finished and not yet looked at: the agents waiting on a human. */
fun needsAttention(status: String): Boolean = status == "blocked" || status == "done"

/**
 * One row: a pane, with whatever is known about it.
 *
 * [row] is the board's fact sheet and is absent for a plain shell, because
 * `session.overview` answers with agents. [node] is the tree's, and is absent
 * only when the tree call itself failed. Between them every open pane has a
 * name and a state, which is what a channel row needs.
 */
data class Channel(
    val paneId: String,
    val name: String,
    val status: String,
    val row: AgentRow?,
    val node: PaneNode?,
    val tabId: String?,
)

/** One space, and the panes inside it. */
data class ChannelSection(
    val workspaceId: String,
    val label: String,
    val space: SpaceNode?,
    val channels: List<Channel>,
)

/**
 * The list, from the two things the server will tell you about a session.
 *
 * The tree decides what exists and in what order — including shells, which the
 * overview does not carry — and the overview decides what each agent *is*. A
 * server (or a moment) that answers only one of the two still produces a list:
 * without the tree this degrades to the old board grouped by space, and without
 * the overview to bare names and states.
 */
fun buildChannels(spaces: List<SpaceNode>, rows: List<AgentRow>): List<ChannelSection> {
    val byPane = rows.associateBy { it.paneId }
    if (spaces.isNotEmpty()) {
        return spaces.map { space ->
            ChannelSection(
                workspaceId = space.workspaceId,
                label = space.label.ifBlank { space.workspaceId },
                space = space,
                channels = space.tabs.flatMap { tab ->
                    tab.panes.map { pane ->
                        val row = byPane[pane.paneId]
                        Channel(
                            paneId = pane.paneId,
                            name = row?.displayName ?: row?.agent ?: pane.agent ?: "shell",
                            status = row?.status ?: pane.status,
                            row = row,
                            node = pane,
                            tabId = tab.tabId,
                        )
                    }
                },
            )
        }
    }
    return rows
        .groupBy { it.workspaceId }
        .map { (id, group) ->
            ChannelSection(
                workspaceId = id,
                label = group.first().workspaceLabel.ifBlank { id },
                space = null,
                channels = group.map { row ->
                    Channel(
                        paneId = row.paneId,
                        name = row.displayName ?: row.agent,
                        status = row.status,
                        row = row,
                        node = null,
                        tabId = null,
                    )
                },
            )
        }
}

/**
 * Whether a failed call means "this server does not have that method".
 *
 * Worth being strict about: latching the fallback on *any* error means one
 * dropped packet permanently downgrades the connection to the thinner
 * `session.snapshot`, and nothing short of a restart puts it back. shep rejects
 * an unknown method while deserialising the request, so the message says so.
 */
fun looksUnsupported(message: String?): Boolean {
    val text = message?.lowercase() ?: return false
    return "unknown variant" in text || "unknown method" in text || "unsupported" in text
}

/** What a confirm dialog is about to do, and the words for it. */
private data class Confirm(
    val title: String,
    val body: String,
    val action: String,
    val run: () -> Unit,
)

/** Which thing is being renamed. Spaces, tabs and panes each have their own method. */
private data class Renaming(
    val title: String,
    val current: String,
    val run: (String) -> Unit,
)

/**
 * The session as one list: spaces as headings, the panes inside them as rows.
 *
 * This replaced two screens — a board that answered "who needs me" and a tree
 * that answered "what is open" — which between them listed every agent twice
 * and made you carry the mapping between the two in your head. One list answers
 * both: the order is the session's own shape, and every pane is on it.
 *
 * There is deliberately no filter. Filtering a list this size hides agents to
 * save scrolling you were not doing, and a default of "attention" meant the
 * answer to "what is running" was a screen that said "nothing attention".
 * Triage is the heading's count and the state mark on each row.
 *
 * Tapping a row still opens the live terminal. Nothing about how you drive an
 * agent changed here; only how you find it.
 */
@Composable
fun ChannelsScreen(
    client: BridgeClient,
    onOpenPane: (AgentRow) -> Unit,
    onUnpair: () -> Unit,
    collapsed: Set<String> = emptySet(),
    onCollapsedChange: (Set<String>) -> Unit = {},
) {
    var spaces by remember { mutableStateOf<List<SpaceNode>>(emptyList()) }
    var rows by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var totals by remember { mutableStateOf(SessionTotals()) }
    var host by remember { mutableStateOf(SessionHost()) }
    var status by remember { mutableStateOf("connecting") }
    var showNew by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    var channelActions by remember { mutableStateOf<Channel?>(null) }
    var spaceActions by remember { mutableStateOf<ChannelSection?>(null) }
    var renaming by remember { mutableStateOf<Renaming?>(null) }
    var confirming by remember { mutableStateOf<Confirm?>(null) }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current

    val refreshSignal = remember { CoChannel<Unit>(CoChannel.CONFLATED) }

    // Event-driven refresh: subscribe once to the structural events plus a
    // per-pane status subscription for each live pane, and re-read on any of
    // them. The timer is only a backstop for a subscription that never opened.
    LaunchedEffect(client) {
        var subscribedPanes: Set<String> = emptySet()
        var subChannel = -1L
        // Latched off only by a server that genuinely lacks the method, so a
        // blip cannot permanently cost the list its richer facts.
        var overviewSupported = true

        suspend fun refresh() {
            val snapshot = withContext(Dispatchers.IO) {
                runCatching { client.call("session.snapshot") }
            }
            snapshot
                .onSuccess { spaces = parseTree(it) }
                .onFailure { status = "reconnect: ${it.message}" }

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
                if (!looksUnsupported(result.exceptionOrNull()?.message)) {
                    status = "reconnect: ${result.exceptionOrNull()?.message}"
                    return
                }
                overviewSupported = false
            }
            // No overview on this server: the snapshot we already fetched
            // carries the agents too, just with fewer facts each.
            snapshot.onSuccess {
                rows = parseSnapshot(it)
                totals = totalsFromRows(rows)
                host = SessionHost(version = client.serverVersion)
                status = "live · shep ${client.serverVersion ?: ""}".trim()
            }
        }

        fun subscribe(paneIds: Set<String>) {
            if (subChannel >= 0) client.closeChannel(subChannel)
            val subs = JSONArray()
            CHANNEL_SUBSCRIPTIONS.forEach { subs.put(JSONObject().put("type", it)) }
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

    /** Run one mutating call, then re-read. Failures are shown, never swallowed. */
    fun act(describe: String, method: String, params: JSONObject) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { notice = describe; refreshSignal.trySend(Unit) }
                .onFailure { notice = "$method failed: ${it.message}" }
        }
    }

    /**
     * Open a session the way a person does at the desktop: make a workspace
     * rooted at `cwd`, then run the runtime in the shell it already gave you.
     *
     * `agent.start` is deliberately not used here. With a `workspace_id` it
     * *splits* into the workspace, which would leave the fresh root shell
     * sitting next to the agent; with `new_workspace` it cannot carry a label.
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
                    // Name the agent too, not just the workspace: the row's
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
                .onSuccess { notice = "started ${runtime.label}"; refreshSignal.trySend(Unit) }
                .onFailure { notice = "could not start: ${it.message}" }
        }
    }

    val sections = buildChannels(spaces, rows)
    // Directories already in play seed the new-session picker, so starting a
    // second session where you are working is two taps and no typing.
    val recentRepos = (rows.mapNotNull { it.cwd } + spaces.flatMap { it.tabs }
        .flatMap { it.panes }.mapNotNull { it.cwd }).distinct()

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("agents") {
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
        if (sections.isEmpty()) {
            EmptyState("no sessions — start one with + new")
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(bottom = ShepSpace.section),
            ) {
                sections.forEach { section ->
                    val channels = section.channels
                    item(key = "space:${section.workspaceId}") {
                        SectionHeader(
                            section = section,
                            shown = channels.size,
                            attention = channels.count { needsAttention(it.status) },
                            collapsed = section.workspaceId in collapsed,
                            onToggle = {
                                onCollapsedChange(
                                    if (section.workspaceId in collapsed) {
                                        collapsed - section.workspaceId
                                    } else {
                                        collapsed + section.workspaceId
                                    }
                                )
                            },
                            onActions = { spaceActions = section },
                        )
                    }
                    if (section.workspaceId !in collapsed) {
                        items(channels, key = { "pane:${it.paneId}" }) { channel ->
                            ChannelRow(
                                modifier = Modifier.animateItem(),
                                channel = channel,
                                onClick = {
                                    onOpenPane(
                                        channel.row
                                            ?: channel.node?.asAgentRow(section.label)
                                            ?: return@ChannelRow,
                                    )
                                },
                                onLongClick = {
                                    // The app's one hidden gesture. Without a
                                    // tick you cannot tell a long-press that
                                    // worked from one that was half a second
                                    // short.
                                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                    channelActions = channel
                                },
                            )
                        }
                    }
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

    channelActions?.let { channel ->
        ChannelActionsSheet(
            channel = channel,
            onDismiss = { channelActions = null },
            onRename = {
                channelActions = null
                renaming = Renaming("rename", channel.name) { name ->
                    act(
                        if (name.isBlank()) "name reset" else "renamed to $name",
                        "agent.rename",
                        JSONObject().put("target", channel.paneId)
                            .apply { if (name.isNotBlank()) put("name", name) },
                    )
                }
            },
            onFocus = {
                channelActions = null
                channel.tabId?.let {
                    act("desktop is on ${channel.name}", "tab.focus", JSONObject().put("tab_id", it))
                }
            },
            onSplit = {
                channelActions = null
                act(
                    "split ${channel.name}",
                    "pane.split",
                    JSONObject()
                        .put("target_pane_id", channel.paneId)
                        .put("direction", "vertical")
                        .put("focus", false),
                )
            },
            onClose = {
                channelActions = null
                confirming = Confirm(
                    title = "Close pane?",
                    body = "${channel.name} stops.",
                    action = "close pane",
                ) {
                    act(
                        "closed ${channel.name}",
                        "pane.close",
                        JSONObject().put("pane_id", channel.paneId),
                    )
                }
            },
        )
    }

    spaceActions?.let { section ->
        SpaceActionsSheet(
            section = section,
            onDismiss = { spaceActions = null },
            onFocus = {
                spaceActions = null
                act(
                    "desktop is on ${section.label}",
                    "workspace.focus",
                    JSONObject().put("workspace_id", section.workspaceId),
                )
            },
            onNewTab = {
                spaceActions = null
                act(
                    "new tab in ${section.label}",
                    "tab.create",
                    JSONObject().put("workspace_id", section.workspaceId).put("focus", false),
                )
            },
            onRename = {
                spaceActions = null
                renaming = Renaming("rename space", section.label) { name ->
                    act(
                        "renamed to $name",
                        "workspace.rename",
                        JSONObject().put("workspace_id", section.workspaceId).put("label", name),
                    )
                }
            },
            onRenameTab = { tabId, label ->
                spaceActions = null
                renaming = Renaming("rename tab", label) { name ->
                    act(
                        "renamed to $name",
                        "tab.rename",
                        JSONObject().put("tab_id", tabId).put("label", name),
                    )
                }
            },
            onCloseTab = { tabId, label, panes ->
                spaceActions = null
                confirming = Confirm(
                    title = "Close tab?",
                    body = "$label — $panes pane(s). Anything running in them stops.",
                    action = "close tab",
                ) {
                    act("closed $label", "tab.close", JSONObject().put("tab_id", tabId))
                }
            },
            onClose = {
                spaceActions = null
                confirming = Confirm(
                    title = "Close space?",
                    body = "${section.label} — ${section.channels.size} pane(s). " +
                        "Anything running in them stops.",
                    action = "close space",
                ) {
                    act(
                        "closed ${section.label}",
                        "workspace.close",
                        JSONObject().put("workspace_id", section.workspaceId),
                    )
                }
            },
        )
    }

    renaming?.let { target ->
        RenameSheet(target.title, target.current, onDismiss = { renaming = null }) { name ->
            renaming = null
            target.run(name)
        }
    }

    confirming?.let { target ->
        AlertDialog(
            onDismissRequest = { confirming = null },
            containerColor = ShepPalette.surfaceDim,
            title = { Text(target.title, style = ShepType.sheetTitle) },
            text = { Text(target.body, style = ShepType.bodySmall) },
            confirmButton = {
                ActionText(
                    target.action,
                    style = ShepType.action.copy(color = ShepPalette.red),
                ) {
                    // Closing a space stops everything running in it. A tick
                    // is the difference between "I pressed it" and "it went".
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    confirming = null
                    target.run()
                }
            },
            dismissButton = { ActionText("keep") { confirming = null } },
        )
    }
}

/**
 * A space, as a heading over its panes.
 *
 * Sticky-feeling rather than a card: the sections are one list, not a stack of
 * boxes, which is the whole difference between reading this and reading the
 * tree it replaced. The attention count is on the heading so a collapsed space
 * can still tell you something is waiting inside it.
 */
@Composable
private fun SectionHeader(
    section: ChannelSection,
    shown: Int,
    attention: Int,
    collapsed: Boolean,
    onToggle: () -> Unit,
    onActions: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.panelBg)
            .minimumInteractiveComponentSize()
            .clickable(onClick = onToggle)
            .padding(start = ShepSpace.medium, end = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (collapsed) "▸" else "▾",
            style = ShepType.state.copy(color = ShepPalette.overlay1),
        )
        Spacer(Modifier.width(ShepSpace.snug))
        Text(
            section.label,
            style = ShepType.sectionLabel,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        section.space?.let { space ->
            ShepSemantic.reviewBadge(space.reviewState)?.let { (glyph, ink) ->
                Spacer(Modifier.width(ShepSpace.snug))
                Text(glyph, style = ShepType.badge.copy(color = ink))
            }
            if (space.isWorktree) {
                Spacer(Modifier.width(ShepSpace.snug))
                Text("worktree", style = ShepType.badge.copy(color = ShepPalette.overlay0))
            }
            if (space.focused) {
                Spacer(Modifier.width(ShepSpace.snug))
                Text("here", style = ShepType.badge.copy(color = ShepPalette.teal))
            }
        }
        Spacer(Modifier.weight(1f))
        if (attention > 0) {
            Text("$attention", style = ShepType.badge.copy(color = ShepPalette.peach))
            Spacer(Modifier.width(ShepSpace.snug))
        } else if (collapsed) {
            Text("$shown", style = ShepType.badge.copy(color = ShepPalette.overlay0))
            Spacer(Modifier.width(ShepSpace.snug))
        }
        ActionText("⋯", style = ShepType.state.copy(color = ShepPalette.overlay1), onClick = onActions)
    }
}

/**
 * One pane in the list.
 *
 * A heading line — the state mark, what it is called, what state it is in —
 * over up to three lines of what the pane is actually saying. The facts that
 * dropped off the board card, age and context, ride the last of those, so a
 * screenful of these still answers the same questions without looking like a
 * screenful of boxes.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ChannelRow(
    channel: Channel,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val row = channel.row
    Row(
        modifier
            .fillMaxWidth()
            .combinedClickable(onLongClick = onLongClick, onClick = onClick)
            .padding(
                start = ShepSpace.screen,
                end = ShepSpace.medium,
                top = ShepSpace.small,
                bottom = ShepSpace.small,
            ),
        verticalAlignment = Alignment.Top,
    ) {
        StateGlyph(channel.status, style = ShepType.stateGlyphSmall)
        Spacer(Modifier.width(ShepSpace.small))
        Column(Modifier.weight(1f)) {
            // The identity group is the one weighted child and the state word
            // is not, so the name gets every pixel the state does not need. A
            // weighted name beside a weighted spacer splits the row fifty-fifty
            // however short the name is, which is what ellipsised "claude ·
            // workmayt" at "workm…" with half a row of empty space beside it.
            Row(verticalAlignment = Alignment.CenterVertically) {
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        channel.name,
                        style = ShepType.agentName,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    row?.displayAgent?.let {
                        Spacer(Modifier.width(ShepSpace.snug))
                        Text(
                            it,
                            style = ShepType.metaSmall.copy(color = ShepPalette.teal),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                    if ((row?.queuedInput ?: 0) > 0) {
                        Spacer(Modifier.width(ShepSpace.snug))
                        Text(
                            "⇥${row?.queuedInput}",
                            style = ShepType.badge.copy(color = ShepPalette.teal),
                        )
                    }
                    row?.reviewState?.let { state ->
                        ShepSemantic.reviewBadge(state)?.let { (glyph, ink) ->
                            Spacer(Modifier.width(ShepSpace.snug))
                            Text(glyph, style = ShepType.badge.copy(color = ink))
                        }
                    }
                }
                Spacer(Modifier.width(ShepSpace.small))
                Text(
                    row?.customStatus ?: channel.status,
                    style = ShepType.metaSmall.copy(color = statusColor(channel.status)),
                    maxLines = 1,
                )
            }
            // What the pane is actually saying. Three lines, because one line
            // of an agent's screen is usually its spinner and tells you the
            // agent is alive but not what it is doing. For a shell — which
            // says nothing shep can read — it is where the shell is instead.
            val transcript = row?.activityLines?.takeIf { it.isNotEmpty() }
                ?: listOfNotNull(row?.activityLine)
            val preview = transcript.ifEmpty {
                listOfNotNull(row?.branch, channel.node?.cwd?.let { repoName(it) }).take(1)
            }
            // One blank line rather than none when there is nothing to quote:
            // age and context ride the last line, and a row with neither line
            // nor age reads as an agent nothing is known about.
            val lines = preview.ifEmpty { listOf("") }
            lines.forEachIndexed { index, line ->
                val isLast = index == lines.lastIndex
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        line,
                        // The newest line is the live one and reads at full
                        // strength; the ones above it are the context that got
                        // it there, so they recede rather than compete.
                        style = ShepType.meta.copy(
                            color = if (isLast) ShepPalette.overlay1 else ShepPalette.overlay0,
                            fontStyle =
                                if (transcript.isNotEmpty()) FontStyle.Italic else FontStyle.Normal,
                        ),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    // Age and context sit on the last line so they stay pinned
                    // to the bottom-right of the row whatever its height.
                    if (isLast) {
                        row?.stateAgeSeconds?.let {
                            Spacer(Modifier.width(ShepSpace.small))
                            Text(formatAge(it), style = ShepType.metaSmall)
                        }
                        row?.contextPercent?.let {
                            Spacer(Modifier.width(ShepSpace.small))
                            ContextGauge(it)
                        }
                    }
                }
            }
        }
    }
}

/** The actions a pane has: the tree's, minus the walk to a second screen. */
@Composable
private fun ChannelActionsSheet(
    channel: Channel,
    onDismiss: () -> Unit,
    onRename: () -> Unit,
    onFocus: () -> Unit,
    onSplit: () -> Unit,
    onClose: () -> Unit,
) {
    ShepSheet(title = channel.name, onDismiss = onDismiss) {
        SheetRow("rename", hint = "what this pane is called", onClick = onRename)
        if (channel.tabId != null) {
            SheetRow("go to", hint = "put the desktop on it", onClick = onFocus)
        }
        SheetRow("split", hint = "a second pane beside it", onClick = onSplit)
        SheetRow("close pane", tone = ShepPalette.red, onClick = onClose)
    }
}

/** The actions a space has, plus its tabs' — the only place tabs still appear. */
@Composable
private fun SpaceActionsSheet(
    section: ChannelSection,
    onDismiss: () -> Unit,
    onFocus: () -> Unit,
    onNewTab: () -> Unit,
    onRename: () -> Unit,
    onRenameTab: (String, String) -> Unit,
    onCloseTab: (String, String, Int) -> Unit,
    onClose: () -> Unit,
) {
    ShepSheet(title = section.label, onDismiss = onDismiss) {
        SheetRow("go to", hint = "put the desktop on it", onClick = onFocus)
        SheetRow("new tab", onClick = onNewTab)
        SheetRow("rename space", onClick = onRename)
        val tabs = section.space?.tabs.orEmpty()
        // Tabs stopped being a level of the list — they are placement, not
        // identity, and nesting three deep is what made the tree hard to read.
        // They keep their actions here so nothing became unreachable.
        if (tabs.size > 1) {
            Spacer(Modifier.height(ShepSpace.small))
            Text("tabs", style = ShepType.sectionLabel)
            tabs.forEach { tab ->
                val label = tab.label.ifEmpty { "tab ${tab.number}" }
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        label,
                        style = ShepType.itemLabel,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    ActionText("rename", style = ShepType.chip.copy(color = ShepPalette.overlay1)) {
                        onRenameTab(tab.tabId, label)
                    }
                    ActionText("close", style = ShepType.chip.copy(color = ShepPalette.red)) {
                        onCloseTab(tab.tabId, label, tab.panes.size)
                    }
                }
            }
        }
        Spacer(Modifier.height(ShepSpace.small))
        SheetRow("close space", tone = ShepPalette.red, onClick = onClose)
    }
}

/** One line in a sheet: what it does, and — when it is not obvious — what that means. */
@Composable
private fun SheetRow(
    label: String,
    hint: String? = null,
    tone: Color = ShepPalette.text,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clickable(onClick = onClick)
            .padding(vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = ShepType.action.copy(color = tone), modifier = Modifier.weight(1f))
        hint?.let { Text(it, style = ShepType.metaSmall, maxLines = 1) }
    }
}

/** One text field and a save button — renaming a space, a tab or a pane. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RenameSheet(
    title: String,
    current: String,
    onDismiss: () -> Unit,
    onSave: (String) -> Unit,
) {
    var name by remember { mutableStateOf(current) }
    ShepSheet(title = title, onDismiss = onDismiss) {
        OutlinedTextField(
            value = name,
            onValueChange = { name = it },
            label = { Text("name", style = ShepType.fieldLabel) },
            textStyle = ShepType.field,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
        )
        Spacer(Modifier.height(ShepSpace.medium))
        ShepButton(
            "save",
            enabled = name.isNotBlank(),
            modifier = Modifier.fillMaxWidth(),
        ) { onSave(name.trim()) }
    }
}

/**
 * The context window as a bar plus its number.
 *
 * A bar because the thing worth seeing down a list is *which agent is nearly
 * full*, and a column of bare percentages does not show that. Warms through
 * yellow to red as it fills, matching the desktop.
 */
@Composable
fun ContextGauge(percent: Int) {
    val clamped = percent.coerceIn(0, 100)
    val color = when {
        clamped >= 85 -> ShepPalette.red
        clamped >= 60 -> ShepPalette.yellow
        else -> ShepPalette.overlay0
    }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Meter(
            fraction = clamped / 100f,
            color = color,
            height = ShepSize.gaugeHeight,
            modifier = Modifier.width(ShepSize.gaugeWidth),
        )
        Spacer(Modifier.width(ShepSpace.snug))
        Text("$clamped%", style = ShepType.badge.copy(color = color))
    }
}
