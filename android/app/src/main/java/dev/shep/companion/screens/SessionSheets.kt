package dev.shep.companion.screens

import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.minimumInteractiveComponentSize
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
enum class SessionRuntime(
    val label: String,
    val argv: List<String>,
    val agentName: String,
    /**
     * The runtime's own "stop asking me" flag, or `null` when it has none that
     * can be named with certainty.
     *
     * Only claude's is listed. The others have their own spellings and their
     * own blast radius, and guessing a flag that a runtime does not accept
     * either fails the launch or — worse — is swallowed and leaves a session
     * you believe is unsupervised behaving normally, or the reverse.
     */
    val bypassFlag: String? = null,
) {
    Claude("claude", listOf("claude"), "claude", "--dangerously-skip-permissions"),
    Opencode("opencode", listOf("opencode"), "opencode"),
    Grok("grok", listOf("grok"), "grok"),
    Terminal("terminal", emptyList(), "shell");
}

/**
 * Start a session.
 *
 * The sheet asks only where, what to run, and (optionally) what to call it,
 * which is the whole of what shep needs to open a workspace.
 *
 * `recentRepos` are paths already visible on the board, so the common case is
 * two taps and no typing.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NewSessionSheet(
    recentRepos: List<String>,
    onDismiss: () -> Unit,
    onStart: (cwd: String, name: String, runtime: SessionRuntime, bypass: Boolean) -> Unit,
) {
    var cwd by remember { mutableStateOf(recentRepos.firstOrNull() ?: "") }
    var name by remember { mutableStateOf("") }
    var runtime by remember { mutableStateOf(SessionRuntime.Claude) }
    // Never remembered across sheets and never the default: the whole point of
    // the flag is that it is a decision, and a decision made once should not
    // quietly apply to every session you open afterwards.
    var bypass by remember { mutableStateOf(false) }

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
            runtime.bypassFlag?.let { flag ->
                Spacer(Modifier.height(ShepSpace.medium))
                Row(
                    Modifier
                        .fillMaxWidth()
                        .minimumInteractiveComponentSize()
                        .clickable { bypass = !bypass },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            "skip permission prompts",
                            style = ShepType.itemName.copy(
                                color = if (bypass) ShepPalette.peach else ShepPalette.text,
                            ),
                        )
                        // What it costs, concretely, rather than "are you sure".
                        // This session will edit files and run commands without
                        // asking, including while nobody is looking at it.
                        Text(
                            if (bypass) {
                                "starts with $flag — it will edit and run " +
                                    "without asking you first"
                            } else {
                                "it will ask before editing files or running commands"
                            },
                            style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
                        )
                    }
                    Switch(
                        checked = bypass,
                        onCheckedChange = { bypass = it },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = ShepPalette.panelBg,
                            checkedTrackColor = ShepPalette.peach,
                            uncheckedThumbColor = ShepPalette.overlay0,
                            uncheckedTrackColor = ShepPalette.surface0,
                        ),
                    )
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
                if (bypass) "start ${runtime.label} unsupervised" else "start ${runtime.label}",
                enabled = cwd.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { onStart(cwd.trim(), name.trim(), runtime, bypass && runtime.bypassFlag != null) }
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

