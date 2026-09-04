package dev.shep.companion.screens

import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
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
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.TaskRow
import dev.shep.companion.parseOverview
import dev.shep.companion.parseTasks
import dev.shep.companion.repoName
import dev.shep.companion.taskIsOpen
import dev.shep.companion.statusPriority
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.components.TaskGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Color for a task lifecycle state, reusing the shep attention vocabulary. */
fun taskStateColor(state: String): Color = ShepSemantic.taskColor(state)

/**
 * Tasks tab (A4): the queue with states, an add-task sheet (repo/runtime/
 * worktree), cancel, and dispatch-now. Polls `task.list` so a dispatched task
 * visibly flips todo → running → done — the A4 gate. `showAdd` is hoisted so
 * the A6 `shep://tasks/new` deep-link can pre-open the sheet from NavShell.
 */
@Composable
fun TasksScreen(
    client: BridgeClient,
    showAdd: Boolean = false,
    onShowAddChange: (Boolean) -> Unit = {},
) {
    var tasks by remember { mutableStateOf<List<TaskRow>>(emptyList()) }
    var sessions by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var sessionsLoaded by remember { mutableStateOf(false) }
    var status by remember { mutableStateOf("loading") }
    var notice by remember { mutableStateOf<String?>(null) }
    var assigning by remember { mutableStateOf<TaskRow?>(null) }
    val scope = rememberCoroutineScope()

    suspend fun refresh() {
        withContext(Dispatchers.IO) { runCatching { client.call("task.list") } }
            .onSuccess { tasks = parseTasks(it); status = "" }
            .onFailure { status = "reconnect: ${it.message}" }
        // The board is what makes a task assignable, so it is polled alongside
        // the queue rather than fetched only when the picker opens.
        withContext(Dispatchers.IO) { runCatching { client.call("session.overview") } }
            .onSuccess { result ->
                parseOverview(result)?.let {
                    sessions = it.agents
                    sessionsLoaded = true
                }
            }
    }

    // Poll so state transitions (the gate) show without a manual refresh.
    LaunchedEffect(client) {
        refresh()
        while (isActive) { delay(2500); refresh() }
    }

    fun act(label: String, method: String, params: JSONObject) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { notice = label; refresh() }
                .onFailure { notice = it.message }
        }
    }

    // Distinct repos already in the queue prefill the add sheet's repo picker.
    val knownRepos = tasks.map { it.repo }.filter { it.isNotEmpty() }.distinct()

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("tasks") {
            if (status.isNotEmpty()) {
                Text(status, style = ShepType.meta)
                Spacer(Modifier.width(ShepSpace.small))
            }
            if (tasks.any { !taskIsOpen(it.state) && it.state != "running" }) {
                ActionText("clear done") { act("cleared finished tasks", "task.clear", JSONObject()) }
            }
            ActionText("+ new", style = ShepType.actionStrong) { onShowAddChange(true) }
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        if (tasks.isEmpty()) {
            EmptyState("no tasks — queue one with + new")
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.listGutter),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(tasks, key = { it.id }) { task ->
                    TaskCard(
                        task = task,
                        assignedAgent = if (sessionsLoaded) {
                            sessions.firstOrNull { it.paneId == task.assignedPaneId }
                        } else {
                            null
                        },
                        onDispatch = {
                            act("dispatching #${task.id}", "task.dispatch", JSONObject().put("task_id", task.id))
                        },
                        onCancel = {
                            act("cancelled #${task.id}", "task.cancel", JSONObject().put("id", task.id))
                        },
                        onAssign = { assigning = task },
                        onRemove = {
                            act("removed #${task.id}", "task.remove", JSONObject().put("id", task.id))
                        },
                    )
                }
            }
        }
    }

    assigning?.let { task ->
        AssignTaskSheet(
            task = task,
            sessions = sessions,
            names = sessions.associate { it.paneId to (it.displayName ?: it.agent) },
            onDismiss = { assigning = null },
            onAssign = { row ->
                assigning = null
                // Send first, record second: the prompt landing in the pane is
                // the real effect, and `task.assign` only claims what already
                // happened. Queued delivery means a busy agent picks it up when
                // it next goes idle instead of being interrupted.
                scope.launch {
                    withContext(Dispatchers.IO) {
                        runCatching {
                            client.call(
                                "agent.send",
                                JSONObject()
                                    .put("target", row.paneId)
                                    .put("text", task.prompt)
                                    .put("queue", true),
                            )
                            client.call(
                                "task.assign",
                                JSONObject()
                                    .put("id", task.id)
                                    .put("workspace_id", row.workspaceId)
                                    .put("pane_id", row.paneId),
                            )
                        }
                    }
                        .onSuccess { notice = "sent #${task.id} to ${row.agent}"; refresh() }
                        .onFailure { notice = "assign failed: ${it.message}" }
                }
            },
        )
    }

    if (showAdd) {
        AddTaskSheet(
            knownRepos = knownRepos,
            agents = sessions,
            onDismiss = { onShowAddChange(false) },
            onSubmit = { prompt, repo, runtime, agent, worktree ->
                onShowAddChange(false)
                scope.launch {
                    withContext(Dispatchers.IO) {
                        runCatching {
                            val created = client.call(
                                "task.add",
                                JSONObject()
                                    .put("prompt", prompt)
                                    .put("repo", repo)
                                    .put("runtime", runtime)
                                    .put("worktree", worktree),
                            )
                            val taskId = created.getLong("id")
                            client.call(
                                "agent.send",
                                JSONObject()
                                    .put("target", agent.paneId)
                                    .put("text", prompt)
                                    .put("queue", true),
                            )
                            client.call(
                                "task.assign",
                                JSONObject()
                                    .put("id", taskId)
                                    .put("workspace_id", agent.workspaceId)
                                    .put("pane_id", agent.paneId),
                            )
                            agent
                        }
                    }.onSuccess { target ->
                        notice = "assigned to ${target.displayName ?: target.agent}"
                        refresh()
                    }.onFailure { notice = "could not assign task: ${it.message}" }
                }
            },
        )
    }
}

@Composable
fun TaskCard(
    task: TaskRow,
    assignedAgent: AgentRow? = null,
    onDispatch: () -> Unit,
    onCancel: () -> Unit,
    onAssign: () -> Unit = {},
    onRemove: () -> Unit = {},
) {
    ShepCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            TaskGlyph(task.state, style = ShepType.stateGlyphSmall)
            Spacer(Modifier.width(ShepSpace.small))
            Text("#${task.id}", style = ShepType.meta)
            Spacer(Modifier.width(ShepSpace.small))
            Text(task.state, style = ShepType.meta.copy(color = taskStateColor(task.state)))
            Spacer(Modifier.weight(1f))
            if (task.useWorktree) Text("⑂", style = ShepType.badge.copy(color = ShepPalette.accent))
        }
        Spacer(Modifier.height(ShepSpace.snug))
        // The task in the words someone typed, so sans.
        Text(task.prompt, style = ShepType.body, maxLines = 3, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(ShepSpace.snug))
        Text(
            "${repoName(task.repo)} · ${task.runtime}" + (task.workspaceId?.let { " · $it" } ?: ""),
            style = ShepType.meta,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        task.assignedPaneId?.takeIf { task.state == "running" || taskIsOpen(task.state) }?.let {
            if (assignedAgent == null) {
                Text("agent unavailable · target disappeared", style = ShepType.meta.copy(color = ShepPalette.peach))
            } else {
                Text(
                    "assigned to ${assignedAgent.displayName ?: assignedAgent.agent}",
                    style = ShepType.meta.copy(color = ShepPalette.teal),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Spacer(Modifier.height(ShepSpace.small))
        Row(
            Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (taskIsOpen(task.state)) {
                // "send to" leads: handing work to an agent already sitting in
                // the right repo is the cheaper move, and dispatch — which
                // spawns a whole new pane — is the fallback, not the default.
                ShepButton("send to…", onClick = onAssign)
                ShepButton("new pane", tone = ButtonTone.Quiet, onClick = onDispatch)
                ShepButton("cancel", tone = ButtonTone.Quiet, onClick = onCancel)
            }
            Spacer(Modifier.weight(1f))
            // Always removable. A queue you cannot empty stops being a queue.
            ActionText("remove", onClick = onRemove)
        }
    }
}

@Composable
fun AddTaskSheet(
    knownRepos: List<String>,
    agents: List<AgentRow>,
    onDismiss: () -> Unit,
    onSubmit: (prompt: String, repo: String, runtime: String, agent: AgentRow, worktree: Boolean) -> Unit,
) {
    var prompt by remember { mutableStateOf("") }
    var repo by remember { mutableStateOf(knownRepos.firstOrNull() ?: "") }
    var runtime by remember { mutableStateOf("claude") }
    var selectedAgent by remember(agents) { mutableStateOf(agents.firstOrNull()) }
    var worktree by remember { mutableStateOf(false) }
    var voiceError by remember { mutableStateOf<String?>(null) }

    // A6 voice add-task: the system recognizer app does the recording, so we
    // need no RECORD_AUDIO permission; absent recognizer just reports inline.
    val voiceLauncher = rememberLauncherForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val said = result.data
            ?.getStringArrayListExtra(android.speech.RecognizerIntent.EXTRA_RESULTS)
            ?.firstOrNull()
        if (!said.isNullOrBlank()) {
            prompt = if (prompt.isBlank()) said else "${prompt.trimEnd()} $said"
            voiceError = null
        }
    }

    ShepSheet(
        title = "new task",
        onDismiss = onDismiss,
        titleAction = {
            ActionText("voice", style = ShepType.actionStrong) {
                voiceError = null
                runCatching {
                    voiceLauncher.launch(
                        Intent(android.speech.RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                            .putExtra(
                                android.speech.RecognizerIntent.EXTRA_LANGUAGE_MODEL,
                                android.speech.RecognizerIntent.LANGUAGE_MODEL_FREE_FORM,
                            )
                            .putExtra(android.speech.RecognizerIntent.EXTRA_PROMPT, "describe the task")
                    )
                }.onFailure { voiceError = "no speech recognizer on this device" }
            }
        },
    ) {
        voiceError?.let {
            Text(it, style = ShepType.meta.copy(color = ShepPalette.peach))
            Spacer(Modifier.height(ShepSpace.tight))
        }
        OutlinedTextField(
            value = prompt,
            onValueChange = { prompt = it },
            placeholder = { Text("prompt for the agent…", style = ShepType.fieldLabel) },
            textStyle = ShepType.body,
            modifier = Modifier.fillMaxWidth(),
            maxLines = 4,
        )
        Spacer(Modifier.height(ShepSpace.small))
        OutlinedTextField(
            value = repo,
            onValueChange = { repo = it },
            label = { Text("repo path", style = ShepType.fieldLabel) },
            placeholder = { Text("/Users/alex/vault/dev/…", style = ShepType.fieldLabel) },
            textStyle = ShepType.field,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
        )
        if (knownRepos.isNotEmpty()) {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                knownRepos.forEach { r -> ShepChip(repoName(r), r == repo) { repo = r } }
            }
        }
        Spacer(Modifier.height(ShepSpace.small))
        Text("assign to agent", style = ShepType.sectionLabel)
        if (agents.isEmpty()) {
            Text("no live agents available", style = ShepType.meta.copy(color = ShepPalette.peach))
        } else {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                agents.sortedWith(compareBy({ statusPriority(it.status) }, { it.workspaceLabel }))
                    .forEach { agent ->
                        ShepChip(
                            (agent.displayName ?: agent.agent) + " · " + agent.workspaceLabel,
                            selectedAgent?.paneId == agent.paneId,
                        ) { selectedAgent = agent }
                    }
            }
        }
        Spacer(Modifier.height(ShepSpace.small))
        Row(
            Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text("--worktree", style = ShepType.sectionLabel)
            Switch(
                checked = worktree,
                onCheckedChange = { worktree = it },
                colors = SwitchDefaults.colors(
                    checkedThumbColor = ShepPalette.panelBg,
                    checkedTrackColor = ShepPalette.accent,
                    uncheckedThumbColor = ShepPalette.overlay0,
                    uncheckedTrackColor = ShepPalette.surface0,
                ),
            )
        }
        Text("branch task/<id>", style = ShepType.metaSmall)
        Spacer(Modifier.height(ShepSpace.small))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("runtime", style = ShepType.sectionLabel)
            Spacer(Modifier.width(ShepSpace.medium))
            Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.small)) {
                listOf("claude", "opencode").forEach { rt ->
                    ShepChip(rt, rt == runtime) { runtime = rt }
                }
            }
        }
        Spacer(Modifier.height(ShepSpace.medium))
        ShepButton(
            "queue task",
            onClick = {
                selectedAgent?.let {
                    onSubmit(prompt.trim(), repo.trim(), runtime, it, worktree)
                }
            },
            enabled = prompt.isNotBlank() && repo.isNotBlank() && selectedAgent != null,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}
