package dev.shep.companion.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import androidx.compose.ui.text.style.TextOverflow
import dev.shep.companion.BridgeClient
import dev.shep.companion.PaneNode
import dev.shep.companion.SpaceNode
import dev.shep.companion.TabNode
import dev.shep.companion.parseTree
import dev.shep.companion.repoName
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.components.StateGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback

/**
 * The events that change the session's shape.
 *
 * Agent-status events are deliberately absent: this screen is about structure,
 * and a pane changing state redraws the board, not this.
 */
private val TREE_SUBSCRIPTIONS = listOf(
    "workspace.created", "workspace.closed", "workspace.renamed",
    "workspace.moved", "workspace.focused",
    "tab.created", "tab.closed", "tab.renamed", "tab.moved", "tab.focused",
    "pane.created", "pane.closed", "pane.focused", "pane.moved", "pane.exited",
)

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
 * The session's shape, and the controls for changing it.
 *
 * The board is the home screen and answers "who needs me". This is the other
 * half — what is open and where — so a space can be opened or closed, a tab
 * added or renamed, a pane split or closed, without walking back to the desk.
 *
 * Every action here is an existing synchronous API method; nothing about the
 * session lives only on the phone.
 */
@Composable
fun SpacesScreen(
    client: BridgeClient,
    onOpenPane: (PaneNode, String) -> Unit,
) {
    var spaces by remember { mutableStateOf<List<SpaceNode>>(emptyList()) }
    var status by remember { mutableStateOf("connecting") }
    var notice by remember { mutableStateOf<String?>(null) }
    var collapsed by remember { mutableStateOf<Set<String>>(emptySet()) }
    var showNewSpace by remember { mutableStateOf(false) }
    var confirming by remember { mutableStateOf<Confirm?>(null) }
    var renaming by remember { mutableStateOf<Renaming?>(null) }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current

    suspend fun refresh() {
        withContext(Dispatchers.IO) { runCatching { client.call("session.snapshot") } }
            .onSuccess { spaces = parseTree(it); status = "" }
            .onFailure { status = "reconnect: ${it.message}" }
    }

    // Structural changes are exactly what this screen shows, so it re-reads on
    // any of them rather than polling on a timer. Same mechanism as the board's
    // subscription; only the event set differs.
    LaunchedEffect(client) {
        val signal = kotlinx.coroutines.channels.Channel<Unit>(
            kotlinx.coroutines.channels.Channel.CONFLATED,
        )
        val subs = org.json.JSONArray()
        TREE_SUBSCRIPTIONS.forEach { subs.put(JSONObject().put("type", it)) }
        val channel = runCatching {
            client.openChannel(
                "events.subscribe",
                JSONObject().put("subscriptions", subs),
                object : BridgeClient.ChannelListener {
                    override fun onLine(line: JSONObject) {
                        signal.trySend(Unit)
                    }

                    override fun onClosed(error: String?) {} // reconnect handled upstream
                },
            )
        }.getOrNull()

        refresh()
        // A slow tick as well as the events: it costs almost nothing, and it is
        // what keeps the screen honest if the subscription never opened.
        val keepalive = launch {
            while (isActive) {
                delay(if (channel == null) 4000 else 15000)
                signal.trySend(Unit)
            }
        }
        try {
            for (unused in signal) {
                delay(100) // coalesce event bursts
                refresh()
            }
        } finally {
            keepalive.cancel()
            channel?.let { client.closeChannel(it) }
        }
    }

    /** Run one mutating call, then re-read. Failures are shown, never swallowed. */
    fun act(describe: String, method: String, params: JSONObject) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { notice = describe; refresh() }
                .onFailure { notice = "$method failed: ${it.message}" }
        }
    }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("spaces") {
            Text(status, style = ShepType.meta)
            Spacer(Modifier.width(ShepSpace.small))
            ActionText("+ new", style = ShepType.actionStrong) { showNewSpace = true }
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        if (spaces.isEmpty()) {
            EmptyState("no spaces — start one with + new")
            return@Column
        }
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(ShepSpace.medium),
            verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            items(spaces.size, key = { spaces[it].workspaceId }) { index ->
                val space = spaces[index]
                SpaceCard(
                    space = space,
                    collapsed = space.workspaceId in collapsed,
                    onToggle = {
                        collapsed = if (space.workspaceId in collapsed) {
                            collapsed - space.workspaceId
                        } else {
                            collapsed + space.workspaceId
                        }
                    },
                    onFocusSpace = {
                        act(
                            "focused ${space.label}",
                            "workspace.focus",
                            JSONObject().put("workspace_id", space.workspaceId),
                        )
                    },
                    onRenameSpace = {
                        renaming = Renaming("rename space", space.label) { name ->
                            act(
                                "renamed to $name",
                                "workspace.rename",
                                JSONObject()
                                    .put("workspace_id", space.workspaceId)
                                    .put("label", name),
                            )
                        }
                    },
                    onCloseSpace = {
                        confirming = Confirm(
                            title = "Close space?",
                            body = "${space.label} — ${space.tabs.size} tab(s), " +
                                "${space.tabs.sumOf { it.panes.size }} pane(s). " +
                                "Anything running in them stops.",
                            action = "close space",
                        ) {
                            act(
                                "closed ${space.label}",
                                "workspace.close",
                                JSONObject().put("workspace_id", space.workspaceId),
                            )
                        }
                    },
                    onNewTab = {
                        act(
                            "new tab in ${space.label}",
                            "tab.create",
                            JSONObject()
                                .put("workspace_id", space.workspaceId)
                                .put("focus", false),
                        )
                    },
                    onFocusTab = { tab ->
                        act("focused ${tab.label}", "tab.focus", JSONObject().put("tab_id", tab.tabId))
                    },
                    onRenameTab = { tab ->
                        renaming = Renaming("rename tab", tab.label) { name ->
                            act(
                                "renamed to $name",
                                "tab.rename",
                                JSONObject().put("tab_id", tab.tabId).put("label", name),
                            )
                        }
                    },
                    onCloseTab = { tab ->
                        confirming = Confirm(
                            title = "Close tab?",
                            body = "${tab.label} — ${tab.panes.size} pane(s). " +
                                "Anything running in them stops.",
                            action = "close tab",
                        ) {
                            act("closed ${tab.label}", "tab.close", JSONObject().put("tab_id", tab.tabId))
                        }
                    },
                    onSplitPane = { pane ->
                        act(
                            "split ${pane.paneId}",
                            "pane.split",
                            JSONObject()
                                .put("target_pane_id", pane.paneId)
                                .put("direction", "vertical")
                                .put("focus", false),
                        )
                    },
                    onClosePane = { pane ->
                        confirming = Confirm(
                            title = "Close pane?",
                            body = "${pane.agent ?: pane.paneId} stops.",
                            action = "close pane",
                        ) {
                            act(
                                "closed ${pane.paneId}",
                                "pane.close",
                                JSONObject().put("pane_id", pane.paneId),
                            )
                        }
                    },
                    onOpenPane = onOpenPane,
                )
            }
        }
    }

    if (showNewSpace) {
        // The directories already open are the ones you are most likely to want
        // another session in, so they seed the picker.
        val recentRepos = spaces
            .flatMap { it.tabs }
            .flatMap { it.panes }
            .mapNotNull { it.cwd }
            .distinct()
        NewSessionSheet(
            recentRepos = recentRepos,
            onDismiss = { showNewSpace = false },
            onStart = { cwd, name, runtime ->
                showNewSpace = false
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
                        }
                    }
                        .onSuccess { notice = "started ${runtime.label}"; refresh() }
                        .onFailure { notice = "could not start: ${it.message}" }
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
            dismissButton = {
                ActionText("keep") { confirming = null }
            },
        )
    }
}

/** One space, its tabs, and their panes. */
@Composable
private fun SpaceCard(
    space: SpaceNode,
    collapsed: Boolean,
    onToggle: () -> Unit,
    onFocusSpace: () -> Unit,
    onRenameSpace: () -> Unit,
    onCloseSpace: () -> Unit,
    onNewTab: () -> Unit,
    onFocusTab: (TabNode) -> Unit,
    onRenameTab: (TabNode) -> Unit,
    onCloseTab: (TabNode) -> Unit,
    onSplitPane: (PaneNode) -> Unit,
    onClosePane: (PaneNode) -> Unit,
    onOpenPane: (PaneNode, String) -> Unit,
) {
    ShepCard(padding = ShepSpace.none) {
        Row(
            Modifier
                .fillMaxWidth()
                .minimumInteractiveComponentSize()
                .clickable { onToggle() }
                .padding(horizontal = ShepSpace.card, vertical = ShepSpace.snug),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // The identity group is the one weighted child, so the chevron
            // lands flush right instead of halfway across: a weighted label
            // beside a weighted spacer splits the row evenly however short the
            // label is, which parked ▾ in the middle of every space card.
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                StatusDot(space.status)
                Spacer(Modifier.width(ShepSpace.small))
                Text(
                    space.label,
                    style = ShepType.agentName,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
                // The desktop's own review badges, so the two read the same —
                // from the shared table, not a local copy of it.
                ShepSemantic.reviewBadge(space.reviewState)?.let { (glyph, ink) ->
                    Spacer(Modifier.width(ShepSpace.snug))
                    Text(glyph, style = ShepType.badge.copy(color = ink))
                }
                if (space.isWorktree) {
                    Spacer(Modifier.width(ShepSpace.snug))
                    Text("worktree", style = ShepType.badge.copy(color = ShepPalette.overlay0))
                }
            }
            if (space.focused) {
                Text("here", style = ShepType.badge.copy(color = ShepPalette.teal))
                Spacer(Modifier.width(ShepSpace.small))
            }
            Text(if (collapsed) "▸" else "▾", style = ShepType.state.copy(color = ShepPalette.overlay1))
        }
        Row(
            Modifier.fillMaxWidth().padding(horizontal = ShepSpace.snug),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.tight),
        ) {
            SmallAction("go to", ShepPalette.accent, onFocusSpace)
            SmallAction("+ tab", ShepPalette.subtext0, onNewTab)
            SmallAction("rename", ShepPalette.subtext0, onRenameSpace)
            Spacer(Modifier.weight(1f))
            SmallAction("close", ShepPalette.red, onCloseSpace)
        }

        if (!collapsed) space.tabs.forEach { tab ->
            Column(Modifier.fillMaxWidth().padding(start = ShepSpace.medium, end = ShepSpace.snug)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusDot(tab.status, nested = true)
                    Spacer(Modifier.width(ShepSpace.small))
                    // Weighted and filling, with no spacer after it, so the
                    // actions sit flush right on every row rather than
                    // wherever this tab's name happens to end.
                    Text(
                        tab.label.ifEmpty { "tab ${tab.number}" },
                        style = ShepType.itemLabel,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f),
                    )
                    Spacer(Modifier.width(ShepSpace.small))
                    SmallAction("go to", ShepPalette.overlay1) { onFocusTab(tab) }
                    SmallAction("rename", ShepPalette.overlay1) { onRenameTab(tab) }
                    // The server refuses to close a space's last tab, so offer
                    // the thing that does work instead of a button that errors.
                    if (!space.hasOnlyOneTab) {
                        SmallAction("close", ShepPalette.red) { onCloseTab(tab) }
                    }
                }
                tab.panes.forEach { pane ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .minimumInteractiveComponentSize()
                            .clickable { onOpenPane(pane, space.label) }
                            .padding(start = ShepSpace.indent, top = ShepSpace.tight, bottom = ShepSpace.tight),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        StatusDot(pane.status, nested = true)
                        Spacer(Modifier.width(ShepSpace.small))
                        Column(Modifier.weight(1f)) {
                            Text(
                                pane.agent ?: "shell",
                                style = ShepType.itemLabel.copy(color = ShepPalette.text),
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            pane.cwd?.let {
                                Text(
                                    repoName(it),
                                    style = ShepType.metaSmall,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                        }
                        SmallAction("split", ShepPalette.overlay1) { onSplitPane(pane) }
                        SmallAction("close", ShepPalette.red) { onClosePane(pane) }
                    }
                }
            }
        }
    }
}

/**
 * A space's state glyph, or — one level in — a tab's or a pane's.
 *
 * Two sizes carry three levels: the space heads its card, and everything
 * nested under it is quieter by the same step. The tree used to ask for four
 * different dot diameters to say the same thing.
 */
@Composable
private fun StatusDot(status: String, nested: Boolean = false) {
    StateGlyph(status, style = if (nested) ShepType.stateGlyphSmall else ShepType.stateGlyph)
}

/**
 * One word in a row of them: "go to", "rename", "close".
 *
 * Up to five share a row here, so they use the shared [ActionText] for its
 * 48dp minimum height rather than being 16dp of bare text — which is what
 * every one of them was.
 */
@Composable
private fun SmallAction(text: String, color: Color, onClick: () -> Unit) {
    ActionText(text, style = ShepType.chip.copy(color = color), onClick = onClick)
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
