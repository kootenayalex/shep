package dev.shep.companion.screens

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import dev.shep.companion.AgentRow
import dev.shep.companion.repoName
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

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
    var cwd by remember { mutableStateOf(recentRepos.firstOrNull() ?: "") }
    var name by remember { mutableStateOf("") }
    var runtime by remember { mutableStateOf(SessionRuntime.Claude) }

    ShepSheet(title = "new session", onDismiss = onDismiss) {
            OutlinedTextField(
                value = cwd,
                onValueChange = { cwd = it },
                label = { Text("directory", style = ShepType.fieldLabel) },
                placeholder = { Text("/Users/alex/vault/dev/…", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            if (recentRepos.isNotEmpty()) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .horizontalScroll(rememberScrollState())
                        .padding(top = ShepSpace.snug),
                    horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
                ) {
                    recentRepos.forEach { path ->
                        ShepChip(repoName(path), path == cwd) { cwd = path }
                    }
                }
            }
            Spacer(Modifier.height(ShepSpace.medium))
            Text("run", style = ShepType.sectionLabel)
            Spacer(Modifier.height(ShepSpace.snug))
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                SessionRuntime.entries.forEach { option ->
                    ShepChip(option.label, option == runtime) { runtime = option }
                }
            }
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("name (optional)", style = ShepType.fieldLabel) },
                placeholder = { Text("billing fix", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            Spacer(Modifier.height(ShepSpace.snug))
            Text(
                "naming it now is what makes it findable later — the board can " +
                    "only tell sessions apart by what you give it.",
                style = ShepType.metaSmall,
            )
            Spacer(Modifier.height(ShepSpace.screen))
            ShepButton(
                "start ${runtime.label}",
                enabled = cwd.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { onStart(cwd.trim(), name.trim(), runtime) }
    }
}

/**
 * Rename one session. Clearing the field restores shep's own label, which is
 * why the empty string is submitted rather than blocked.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RenameSessionSheet(row: AgentRow, onDismiss: () -> Unit, onRename: (String) -> Unit) {
    var name by remember { mutableStateOf(row.agent) }

    ShepSheet(title = "name this session", onDismiss = onDismiss) {
            Text(
                listOfNotNull(
                    row.workspaceLabel.takeIf { it.isNotBlank() },
                    row.branch,
                    row.cwd,
                ).joinToString(" · "),
                style = ShepType.meta,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = name,
                onValueChange = { name = it },
                label = { Text("name", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            Spacer(Modifier.height(ShepSpace.screen))
            Row(
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(ShepSpace.medium),
            ) {
                ShepButton("save", modifier = Modifier.weight(1f)) { onRename(name.trim()) }
                // Clearing the field restores shep's own label, which is why
                // the empty string is submitted rather than blocked.
                ShepButton("reset", tone = ButtonTone.Quiet) { onRename("") }
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
    val wanted = repoName(task.repo)
    val ordered = sessions.sortedByDescending { row ->
        row.cwd?.let { repoName(it) == wanted } == true
    }

    ShepSheet(title = "send #${task.id} to…", onDismiss = onDismiss) {
            // The task's own words, so sans: this is the one line in the
            // sheet that a person wrote rather than shep generated.
            Text(
                task.prompt,
                style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            Spacer(Modifier.height(ShepSpace.medium))
            if (ordered.isEmpty()) {
                Text(
                    "no sessions running — start one from the board first",
                    style = ShepType.emptyState,
                )
            }
            ordered.forEach { row ->
                val sameRepo = row.cwd?.let { repoName(it) == wanted } == true
                Spacer(Modifier.height(ShepSpace.small))
                ShepCard(shape = ShepShape.field, onClick = { onAssign(row) }) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            names[row.paneId] ?: row.agent,
                            style = ShepType.itemName,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        if (sameRepo) {
                            Spacer(Modifier.width(ShepSpace.small))
                            Text("same repo", style = ShepType.badge.copy(color = ShepPalette.teal))
                        }
                        Spacer(Modifier.weight(1f))
                        Text(row.status, style = ShepType.meta.copy(color = ShepPalette.overlay1))
                    }
                    Text(
                        listOfNotNull(row.cwd, row.branch).joinToString(" · "),
                        style = ShepType.metaSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
            }
        }
    }
}
