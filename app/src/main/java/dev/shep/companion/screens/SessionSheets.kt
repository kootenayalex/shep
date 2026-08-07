package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.shep.companion.AgentRow
import dev.shep.companion.repoName
import dev.shep.companion.ui.theme.ShepPalette

/**
 * What a new session can be started as.
 *
 * `argv` is what shep actually executes, so "terminal" is simply the absence of
 * an agent — a login shell, which is a first-class thing to want from the phone
 * and not a degraded agent session.
 */
enum class SessionRuntime(val label: String, val argv: List<String>, val agentName: String) {
    Claude("claude", listOf("claude"), "claude"),
    Opencode("opencode", listOf("opencode"), "opencode"),
    Grok("grok", listOf("grok"), "grok"),
    Terminal("terminal", emptyList(), "shell");
}

/** A pill that reads as selected or not, the app's one chip shape. */
@Composable
private fun Chip(text: String, selected: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .clip(RoundedCornerShape(14.dp))
            .background(if (selected) ShepPalette.accent else ShepPalette.surface0)
            .clickable { onClick() }
            .padding(horizontal = 12.dp, vertical = 6.dp),
    ) {
        Text(
            text,
            color = if (selected) ShepPalette.panelBg else ShepPalette.subtext0,
            fontSize = 12.sp,
            fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Normal,
        )
    }
}

/**
 * Start a session without queueing a task first.
 *
 * A task is a thing you want done; a session is a place to work. Requiring the
 * first to get the second is the wrong shape — this sheet asks only where, what
 * to run, and (optionally) what to call it, which is the whole of what shep
 * needs to open a workspace.
 *
 * `recentRepos` are paths already visible on the board, so the common case is
 * two taps and no typing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionSheet(
    recentRepos: List<String>,
    onDismiss: () -> Unit,
    onStart: (cwd: String, name: String, runtime: SessionRuntime) -> Unit,
) {
    val sheetState = rememberModalBottomSheetState()
    var cwd by remember { mutableStateOf(recentRepos.firstOrNull() ?: "") }
    var name by remember { mutableStateOf("") }
    var runtime by remember { mutableStateOf(SessionRuntime.Claude) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = ShepPalette.surfaceDim,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Text(
                "new session",
                color = ShepPalette.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(14.dp))
            OutlinedTextField(
                value = cwd,
                onValueChange = { cwd = it },
                label = { Text("directory", color = ShepPalette.overlay1) },
                placeholder = { Text("/Users/alex/vault/dev/…", color = ShepPalette.overlay0) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            if (recentRepos.isNotEmpty()) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .horizontalScroll(rememberScrollState())
                        .padding(top = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    recentRepos.forEach { path ->
                        Chip(repoName(path), path == cwd) { cwd = path }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            Text("run", color = ShepPalette.overlay1, fontSize = 13.sp)
            Spacer(Modifier.height(6.dp))
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SessionRuntime.entries.forEach { option ->
                    Chip(option.label, option == runtime) { runtime = option }
                }
            }
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("name (optional)", color = ShepPalette.overlay1) },
                placeholder = { Text("billing fix", color = ShepPalette.overlay0) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            Spacer(Modifier.height(6.dp))
            Text(
                "naming it now is what makes it findable later — the board can " +
                    "only tell sessions apart by what you give it.",
                color = ShepPalette.overlay0,
                fontSize = 11.sp,
            )
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = { onStart(cwd.trim(), name.trim(), runtime) },
                enabled = cwd.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("start ${runtime.label}") }
            Spacer(Modifier.height(8.dp))
        }
    }
}

/**
 * Rename one session. Clearing the field restores shep's own label, which is
 * why the empty string is submitted rather than blocked.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RenameSessionSheet(row: AgentRow, onDismiss: () -> Unit, onRename: (String) -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    var name by remember { mutableStateOf(row.agent) }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = ShepPalette.surfaceDim,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Text(
                "name this session",
                color = ShepPalette.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                listOfNotNull(
                    row.workspaceLabel.takeIf { it.isNotBlank() },
                    row.branch,
                    row.cwd,
                ).joinToString(" · "),
                color = ShepPalette.overlay0,
                fontSize = 12.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(14.dp))
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("name", color = ShepPalette.overlay1) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            Spacer(Modifier.height(16.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Button(
                    onClick = { onRename(name.trim()) },
                    modifier = Modifier.weight(1f),
                ) { Text("save") }
                Spacer(Modifier.width(12.dp))
                TextButton(onClick = { onRename("") }) {
                    Text("reset", color = ShepPalette.overlay1)
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

/**
 * Pick which running session gets a queued task.
 *
 * This is the answer to "the queue is not useful": a task stops being an
 * orphan prompt waiting for a pane to be spawned for it, and becomes work you
 * hand to a named agent that is already in the right repo. Sessions in the
 * task's own repo are offered first because that is nearly always the intent.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AssignTaskSheet(
    task: dev.shep.companion.TaskRow,
    sessions: List<AgentRow>,
    names: Map<String, String>,
    onDismiss: () -> Unit,
    onAssign: (AgentRow) -> Unit,
) {
    val sheetState = rememberModalBottomSheetState()
    val wanted = repoName(task.repo)
    val ordered = sessions.sortedByDescending { row ->
        row.cwd?.let { repoName(it) == wanted } == true
    }

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheetState,
        containerColor = ShepPalette.surfaceDim,
    ) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(
                "send #${task.id} to…",
                color = ShepPalette.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(4.dp))
            Text(
                task.prompt,
                color = ShepPalette.overlay0,
                fontSize = 12.sp,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(14.dp))
            if (ordered.isEmpty()) {
                Text(
                    "no sessions running — start one from the board first",
                    color = ShepPalette.overlay1,
                    fontSize = 13.sp,
                )
            }
            ordered.forEach { row ->
                val sameRepo = row.cwd?.let { repoName(it) == wanted } == true
                Column(
                    Modifier
                        .fillMaxWidth()
                        .padding(vertical = 4.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .background(ShepPalette.surface0)
                        .clickable { onAssign(row) }
                        .padding(horizontal = 12.dp, vertical = 10.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            names[row.paneId] ?: row.agent,
                            color = ShepPalette.text,
                            fontSize = 14.sp,
                            fontWeight = FontWeight.SemiBold,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        if (sameRepo) {
                            Spacer(Modifier.width(8.dp))
                            Text("same repo", color = ShepPalette.teal, fontSize = 11.sp)
                        }
                        Spacer(Modifier.weight(1f))
                        Text(row.status, color = ShepPalette.overlay1, fontSize = 12.sp)
                    }
                    Text(
                        listOfNotNull(row.cwd, row.branch).joinToString(" · "),
                        color = ShepPalette.overlay0,
                        fontSize = 11.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}
