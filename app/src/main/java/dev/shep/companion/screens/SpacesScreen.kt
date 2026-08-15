package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shep.companion.BridgeClient
import dev.shep.companion.PaneNode
import dev.shep.companion.SpaceNode
import dev.shep.companion.TabNode
import dev.shep.companion.parseTree
import dev.shep.companion.repoName
import dev.shep.companion.statusColorFor
import dev.shep.companion.ui.theme.ShepPalette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import dev.shep.companion.ui.components.StateGlyph

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
    onOpenPane: (PaneNode) -> Unit,
) {
    var spaces by remember { mutableStateOf<List<SpaceNode>>(emptyList()) }
    var status by remember { mutableStateOf("connecting") }
    var notice by remember { mutableStateOf<String?>(null) }
    var collapsed by remember { mutableStateOf<Set<String>>(emptySet()) }
    var showNewSpace by remember { mutableStateOf(false) }
    var confirming by remember { mutableStateOf<Confirm?>(null) }
    var renaming by remember { mutableStateOf<Renaming?>(null) }
    val scope = rememberCoroutineScope()

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
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("spaces", color = ShepPalette.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            Text(status, color = ShepPalette.overlay1, fontSize = 12.sp)
            Spacer(Modifier.width(12.dp))
            Text(
                "+ new",
                color = ShepPalette.accent,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clickable { showNewSpace = true },
            )
        }
        notice?.let {
            Text(
                it,
                color = ShepPalette.peach,
                fontSize = 12.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { notice = null }
                    .padding(horizontal = 16.dp, vertical = 4.dp),
            )
        }
        if (spaces.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no spaces — start one with + new", color = ShepPalette.overlay1)
            }
            return@Column
        }
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
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
            title = { Text(target.title, color = ShepPalette.text) },
            text = { Text(target.body, color = ShepPalette.overlay1, fontSize = 13.sp) },
            confirmButton = {
                TextButton(onClick = {
                    confirming = null
                    target.run()
                }) { Text(target.action, color = ShepPalette.red) }
            },
            dismissButton = {
                TextButton(onClick = { confirming = null }) {
                    Text("keep", color = ShepPalette.overlay1)
                }
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
    onOpenPane: (PaneNode) -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(ShepPalette.surface0)
            .padding(vertical = 10.dp),
    ) {
        Row(
            Modifier
                .fillMaxWidth()
                .clickable { onToggle() }
                .padding(horizontal = 14.dp, vertical = 2.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StatusDot(space.status)
            Spacer(Modifier.width(10.dp))
            Text(
                space.label,
                color = ShepPalette.text,
                fontSize = 15.sp,
                fontWeight = FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f, fill = false),
            )
            // The desktop's own review badges, so the two read the same.
            reviewBadge(space.reviewState)?.let {
                Spacer(Modifier.width(6.dp))
                Text(it, color = ShepPalette.peach, fontSize = 13.sp)
            }
            if (space.isWorktree) {
                Spacer(Modifier.width(6.dp))
                Text("worktree", color = ShepPalette.overlay0, fontSize = 11.sp)
            }
            Spacer(Modifier.weight(1f))
            if (space.focused) {
                Text("here", color = ShepPalette.teal, fontSize = 11.sp)
                Spacer(Modifier.width(8.dp))
            }
            Text(if (collapsed) "▸" else "▾", color = ShepPalette.overlay1, fontSize = 13.sp)
        }
        Row(
            Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            SmallAction("go to", ShepPalette.accent, onFocusSpace)
            SmallAction("+ tab", ShepPalette.subtext0, onNewTab)
            SmallAction("rename", ShepPalette.subtext0, onRenameSpace)
            Spacer(Modifier.weight(1f))
            SmallAction("close", ShepPalette.red, onCloseSpace)
        }

        if (collapsed) return@Column

        space.tabs.forEach { tab ->
            Column(Modifier.fillMaxWidth().padding(start = 22.dp, end = 14.dp, top = 6.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusDot(tab.status)
                    Spacer(Modifier.width(8.dp))
                    Text(
                        tab.label.ifEmpty { "tab ${tab.number}" },
                        color = ShepPalette.subtext0,
                        fontSize = 13.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    Spacer(Modifier.weight(1f))
                    SmallAction("go to", ShepPalette.overlay1) { onFocusTab(tab) }
                    Spacer(Modifier.width(12.dp))
                    SmallAction("rename", ShepPalette.overlay1) { onRenameTab(tab) }
                    Spacer(Modifier.width(12.dp))
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
                            .padding(start = 18.dp, top = 4.dp)
                            .clickable { onOpenPane(pane) },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        StatusDot(pane.status, size = 6)
                        Spacer(Modifier.width(8.dp))
                        Column(Modifier.weight(1f)) {
                            Text(
                                pane.agent ?: "shell",
                                color = ShepPalette.text,
                                fontSize = 13.sp,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            pane.cwd?.let {
                                Text(
                                    repoName(it),
                                    color = ShepPalette.overlay0,
                                    fontSize = 11.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                        }
                        SmallAction("split", ShepPalette.overlay1) { onSplitPane(pane) }
                        Spacer(Modifier.width(12.dp))
                        SmallAction("close", ShepPalette.red) { onClosePane(pane) }
                    }
                }
            }
        }
        Spacer(Modifier.height(4.dp))
    }
}

/** `◆ ↺ ✓` — the desktop sidebar's review badges, unchanged. */
private fun reviewBadge(state: String): String? = when (state) {
    "needs_review" -> "◆"
    "changes_requested" -> "↺"
    "approved" -> "✓"
    else -> null
}

@Composable
private fun StatusDot(status: String, size: Int = 8) {
    // `size` was a dot diameter and is now a type size; the two happen to read
    // at about the same weight, so the tree's rhythm is unchanged.
    StateGlyph(status, fontSize = (size + 4).sp)
}

@Composable
private fun SmallAction(text: String, color: Color, onClick: () -> Unit) {
    Text(
        text,
        color = color,
        fontSize = 12.sp,
        modifier = Modifier.clickable { onClick() },
    )
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
    val sheetState = rememberModalBottomSheetState()
    var name by remember { mutableStateOf(current) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = ShepPalette.surfaceDim,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Text(title, color = ShepPalette.text, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(14.dp))
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("name", color = ShepPalette.overlay1) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = { onSave(name.trim()) },
                enabled = name.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("save") }
            Spacer(Modifier.height(8.dp))
        }
    }
}
