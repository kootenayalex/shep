package dev.shep.companion

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import dev.shep.companion.screens.SessionRuntime
import dev.shep.companion.screens.pane.PaneScreen
import dev.shep.companion.ui.components.TaskGlyph
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepTheme
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

fun statusColor(status: String): Color = ShepSemantic.agentColor(status)

class MainActivity : ComponentActivity() {
    // Pane id from a `shep://pane?pane=…` notification tap; consumed by NavShell.
    private val deepLinkPane = mutableStateOf<String?>(null)
    // A6: `shep://tasks/new` (launcher shortcut / widget) → Tasks tab, add sheet.
    private val deepLinkNewTask = mutableStateOf(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        deepLinkPane.value = paneFromIntent(intent)
        deepLinkNewTask.value = newTaskFromIntent(intent)
        setContent {
            ShepTheme {
                ShepApp(
                    getSharedPreferences("shep", Context.MODE_PRIVATE),
                    deepLinkPane = deepLinkPane.value,
                    onDeepLinkConsumed = { deepLinkPane.value = null },
                    newTask = deepLinkNewTask.value,
                    onNewTaskConsumed = { deepLinkNewTask.value = false },
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        paneFromIntent(intent)?.let { deepLinkPane.value = it }
        if (newTaskFromIntent(intent)) deepLinkNewTask.value = true
    }

    private fun paneFromIntent(intent: Intent?): String? {
        val data = intent?.data ?: return null
        if (data.scheme != "shep" || data.host != "pane") return null
        return data.getQueryParameter("pane")?.takeIf { it.isNotBlank() }
    }

    private fun newTaskFromIntent(intent: Intent?): Boolean {
        val data = intent?.data ?: return false
        return data.scheme == "shep" && data.host == "tasks" && data.pathSegments.firstOrNull() == "new"
    }
}

/** Bottom-nav destinations. Glyphs mirror the TUI vocabulary (no icon dep). */
enum class Tab(val label: String, val glyph: String) {
    Agents("board", "◫"),
    Spaces("spaces", "❏"),
    Tasks("tasks", "☰"),
    Memory("memory", "✦"),
    Shep("shep", "⚙"),
}

@Composable
fun ShepApp(
    prefs: android.content.SharedPreferences,
    deepLinkPane: String? = null,
    onDeepLinkConsumed: () -> Unit = {},
    newTask: Boolean = false,
    onNewTaskConsumed: () -> Unit = {},
) {
    var client by remember { mutableStateOf<BridgeClient?>(null) }
    var paired by remember { mutableStateOf(false) }
    var connectError by remember { mutableStateOf<String?>(null) }
    // True while the first auto-connect with a saved pairing is in flight —
    // shows a connecting spinner instead of flashing the manual pairing form.
    var firstConnect by remember { mutableStateOf(prefs.getString("token", null) != null) }
    // Bumped by a dropped socket to kick the reconnect loop below.
    var reconnectSignal by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current

    // Ask for POST_NOTIFICATIONS (Android 13+) so A3 pages can show.
    val notifPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* granted or not — push still registers; the OS just suppresses posts */ }

    // Once paired, register for push and (13+) request the notif permission.
    // Runs whenever pairing flips true; both registrations are idempotent.
    LaunchedEffect(paired) {
        if (paired) {
            if (Build.VERSION.SDK_INT >= 33) {
                notifPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
            }
            // FCM is the transport that works when the phone is asleep. No
            // kinds are passed: an unqualified re-register on every app start
            // must not overwrite a choice made in settings.
            FcmManager.register(context)
            // UnifiedPush stays registered alongside until FCM is confirmed on
            // real hardware, so a failure of one is not a failure of both.
            withContext(Dispatchers.IO) { PushManager.register(context) }
        }
    }

    // Establish a fresh connection from the saved pairing. Used for the first
    // auto-connect and for every reconnect; wires onDisconnect so a dropped
    // tailnet socket self-heals instead of stranding the screen.
    suspend fun establish(): String? {
        val url = prefs.getString("url", null) ?: return "no saved pairing"
        val token = prefs.getString("token", null) ?: return "no saved pairing"
        val fresh = BridgeClient(url, token)
        val error = withContext(Dispatchers.IO) {
            runCatching { fresh.connect() }.getOrElse { it.message ?: "connection failed" }
        }
        if (error != null) return error
        fresh.onDisconnect = { reason ->
            connectError = reason ?: "disconnected"
            reconnectSignal += 1
        }
        client?.close()
        client = fresh
        return null
    }

    fun pairAndConnect(url: String, token: String, onDone: (String?) -> Unit) {
        scope.launch {
            prefs.edit().putString("url", url).putString("token", token).apply()
            val error = establish()
            if (error == null) {
                paired = true
                connectError = null
            }
            onDone(error)
        }
    }

    // Auto-connect on launch when a saved pairing exists.
    LaunchedEffect(Unit) {
        if (prefs.getString("token", null) != null) {
            val error = establish()
            if (error == null) paired = true else connectError = error
            firstConnect = false
        }
    }

    // Reconnect loop: on a drop, retry with exponential backoff until the
    // socket is back or the user unpairs.
    LaunchedEffect(reconnectSignal) {
        if (reconnectSignal == 0 || !paired) return@LaunchedEffect
        client = null
        var backoff = 1000L
        while (paired && isActive) {
            val error = establish()
            if (error == null) {
                connectError = null
                break
            }
            connectError = error
            delay(backoff)
            backoff = (backoff * 2).coerceAtMost(15000L)
        }
    }

    // Nav bar as well as status bar: API 35 draws edge-to-edge whether the app
    // asks or not, and without this the gesture pill sits on top of the key bar.
    Surface(
        Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding(),
        color = ShepPalette.panelBg,
    ) {
        if (!paired) {
            if (firstConnect && connectError == null) {
                ReconnectingScreen(null, label = "connecting…")
            } else {
                PairingScreen(
                    initialUrl = prefs.getString("url", "") ?: "",
                    initialToken = prefs.getString("token", "") ?: "",
                    lastError = connectError,
                    onConnect = { url, token, onDone -> pairAndConnect(url, token, onDone) },
                )
            }
        } else {
            val active = client
            if (active == null) {
                ReconnectingScreen(connectError)
            } else {
                NavShell(
                    client = active,
                    deepLinkPane = deepLinkPane,
                    onDeepLinkConsumed = onDeepLinkConsumed,
                    newTask = newTask,
                    onNewTaskConsumed = onNewTaskConsumed,
                    onUnpair = {
                        paired = false
                        active.close()
                        client = null
                    },
                )
            }
        }
    }
}

/** Shown while the reconnect loop re-establishes a dropped socket. */
@Composable
fun ReconnectingScreen(error: String?, label: String = "reconnecting…") {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            CircularProgressIndicator(color = ShepPalette.accent)
            Spacer(Modifier.height(ShepSpace.screen))
            Text(label, style = ShepType.emptyState)
            error?.let {
                Spacer(Modifier.height(ShepSpace.snug))
                Text(it, style = ShepType.meta.copy(color = ShepPalette.peach))
            }
        }
    }
}

/**
 * The paired experience: a bottom-nav Scaffold over the four destinations, with
 * the pane view pushed as a full-screen detail over the Agents tab on phones, or
 * docked side-by-side on iPad-class widths (A6 two-pane). A3 deep-links route
 * here by setting the tab + selecting a pane; the A6 `shep://tasks/new`
 * deep-link opens the Tasks tab with the add sheet already up.
 */
@Composable
fun NavShell(
    client: BridgeClient,
    onUnpair: () -> Unit,
    deepLinkPane: String? = null,
    onDeepLinkConsumed: () -> Unit = {},
    newTask: Boolean = false,
    onNewTaskConsumed: () -> Unit = {},
) {
    var tab by remember { mutableStateOf(Tab.Agents) }
    var paneDetail by remember { mutableStateOf<AgentRow?>(null) }
    // A pane id waiting to be resolved into the row the pane view needs.
    var openPaneById by remember { mutableStateOf<String?>(null) }
    // Hoisted from TasksScreen so the new-task deep-link can pre-open the sheet.
    var tasksShowAdd by remember { mutableStateOf(false) }

    // A notification tap (shep://pane?pane=…) resolves the pane id to its row via
    // a one-shot snapshot and pushes the pane detail; falls back to the Agents
    // tab if the pane is gone.
    LaunchedEffect(deepLinkPane) {
        val target = deepLinkPane ?: return@LaunchedEffect
        tab = Tab.Agents
        val row = withContext(Dispatchers.IO) {
            runCatching { parseSnapshot(client.call("session.snapshot")) }.getOrNull()
        }?.find { it.paneId == target }
        if (row != null) paneDetail = row
        onDeepLinkConsumed()
    }

    // Same resolution, from the spaces tree. It stays on the Spaces tab, so
    // closing the pane view returns you to where you were.
    LaunchedEffect(openPaneById) {
        val target = openPaneById ?: return@LaunchedEffect
        val row = withContext(Dispatchers.IO) {
            runCatching { parseSnapshot(client.call("session.snapshot")) }.getOrNull()
        }?.find { it.paneId == target }
        if (row != null) paneDetail = row
        openPaneById = null
    }

    // Launcher shortcut / widget (shep://tasks/new): Tasks tab, sheet open.
    LaunchedEffect(newTask) {
        if (newTask) {
            tab = Tab.Tasks
            tasksShowAdd = true
            onNewTaskConsumed()
        }
    }

    BoxWithConstraints(Modifier.fillMaxSize()) {
        // iPad-class width: list and detail stay visible side-by-side (spatial
        // memory — no full-screen push). Phone: the detail pushes over, as before.
        val wide = maxWidth >= ShepSize.twoPaneWidth
        val detail = paneDetail

        if (detail != null && !wide) {
            BackHandler { paneDetail = null }
            PaneScreen(client, detail, onBack = { paneDetail = null })
            return@BoxWithConstraints
        }

        Scaffold(
            containerColor = ShepPalette.panelBg,
            bottomBar = {
                NavigationBar(
                    containerColor = ShepPalette.surfaceDim,
                    modifier = Modifier.navigationBarsPadding(),
                ) {
                    Tab.entries.forEach { entry ->
                        NavigationBarItem(
                            selected = tab == entry,
                            onClick = { tab = entry },
                            icon = { Text(entry.glyph, style = ShepType.navGlyph) },
                            label = { Text(entry.label, style = ShepType.navLabel) },
                            colors = NavigationBarItemDefaults.colors(
                                selectedIconColor = ShepPalette.accent,
                                selectedTextColor = ShepPalette.accent,
                                unselectedIconColor = ShepPalette.overlay0,
                                unselectedTextColor = ShepPalette.overlay0,
                                indicatorColor = ShepPalette.surface0,
                            ),
                        )
                    }
                }
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                if (wide && tab == Tab.Agents) {
                    Row(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f)) {
                            HomeScreen(
                                client = client,
                                onOpenPane = { paneDetail = it },
                                onUnpair = onUnpair,
                            )
                        }
                        Box(
                            Modifier
                                .weight(1.3f)
                                .fillMaxHeight()
                                .background(ShepPalette.panelBg),
                        ) {
                            if (detail != null) {
                                PaneScreen(client, detail, onBack = { paneDetail = null })
                            } else {
                                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                                    Text("select an agent", style = ShepType.emptyState)
                                }
                            }
                        }
                    }
                } else {
                    when (tab) {
                        Tab.Agents -> HomeScreen(
                            client = client,
                            onOpenPane = { paneDetail = it },
                            onUnpair = onUnpair,
                        )
                        Tab.Spaces -> dev.shep.companion.screens.SpacesScreen(
                            client = client,
                            // The tree knows a pane id; the pane view wants the
                            // board's row for it, so resolve through the same
                            // path a notification tap uses rather than inventing
                            // a second, thinner pane view.
                            onOpenPane = { node -> openPaneById = node.paneId },
                        )
                        Tab.Tasks -> TasksScreen(
                            client = client,
                            showAdd = tasksShowAdd,
                            onShowAddChange = { tasksShowAdd = it },
                        )
                        Tab.Memory -> MemoryScreen(client)
                        Tab.Shep -> ShepScreen()
                    }
                }
            }
        }
    }
}

/**
 * Settings tab: what shep will notify about, and whether push works at all.
 *
 * The toggles change what the *server* sends, not just what this phone shows.
 * That way a muted kind costs no radio wake, and the choice survives a
 * reinstall. The test button exists because a broken push setup looks exactly
 * like a quiet one — there is no other way to tell them apart.
 */
@Composable
fun ShepScreen() {
    val context = androidx.compose.ui.platform.LocalContext.current
    val prefs = remember { context.getSharedPreferences("shep", Context.MODE_PRIVATE) }
    var status by remember { mutableStateOf(prefs.getString("push_status", "not registered") ?: "") }
    var token by remember { mutableStateOf(prefs.getString("fcm_token", null)) }
    var kinds by remember { mutableStateOf(FcmManager.selectedKinds(context)) }
    var testResult by remember { mutableStateOf<String?>(null) }
    var testing by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.screen),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("shep", style = ShepType.screenTitle)
        }
        Column(Modifier.fillMaxWidth().padding(ShepSpace.screen)) {
            Text("notify me about", style = ShepType.sectionLabel)
            Spacer(Modifier.height(ShepSpace.hair))
            Text(
                "shep stops sending what is off here, so it costs no battery.",
                style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
            )
            Spacer(Modifier.height(ShepSpace.small))
            NotifyKind.entries.forEach { kind ->
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable {
                            val next = if (kind in kinds) kinds - kind else kinds + kind
                            kinds = next
                            FcmManager.setKinds(context, next) { status = it }
                        }
                        .padding(vertical = ShepSpace.small),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(kind.label, style = ShepType.itemName)
                        // What the toggle actually costs you, in prose.
                        Text(kind.description, style = ShepType.bodySmall.copy(color = ShepPalette.overlay0))
                    }
                    Switch(
                        checked = kind in kinds,
                        onCheckedChange = { on ->
                            val next = if (on) kinds + kind else kinds - kind
                            kinds = next
                            FcmManager.setKinds(context, next) { status = it }
                        },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = ShepPalette.panelBg,
                            checkedTrackColor = ShepPalette.accent,
                            uncheckedThumbColor = ShepPalette.overlay0,
                            uncheckedTrackColor = ShepPalette.surface0,
                        ),
                    )
                }
            }

            Spacer(Modifier.height(ShepSpace.section))
            Text("delivery", style = ShepType.sectionLabel)
            Spacer(Modifier.height(ShepSpace.snug))
            Text(status, style = ShepType.state.copy(color = ShepPalette.overlay0))
            Text(
                if (token != null) "registered with FCM" else "no FCM token yet",
                style = ShepType.meta.copy(
                    color = if (token != null) ShepPalette.green else ShepPalette.peach,
                ),
            )
            Spacer(Modifier.height(ShepSpace.medium))
            Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.medium)) {
                Button(
                    onClick = {
                        testing = true
                        testResult = null
                        FcmManager.sendTest(context) {
                            testResult = it
                            testing = false
                        }
                    },
                    enabled = !testing,
                ) { Text(if (testing) "sending…" else "Send test notification", style = ShepType.button) }
                TextButton(onClick = {
                    FcmManager.register(context, kinds)
                    status = "registering…"
                    token = prefs.getString("fcm_token", null)
                }) { Text("Re-register", style = ShepType.button.copy(color = ShepPalette.overlay0)) }
            }
            testResult?.let {
                Spacer(Modifier.height(ShepSpace.small))
                Text(
                    it,
                    style = ShepType.meta.copy(
                        color = if (it.startsWith("sent to")) ShepPalette.green else ShepPalette.peach,
                    ),
                )
            }
            Spacer(Modifier.height(ShepSpace.section))
        }
    }
}

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
            .onSuccess { result -> parseOverview(result)?.let { sessions = it.agents } }
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
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.screen),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("tasks", style = ShepType.screenTitle)
            Spacer(Modifier.weight(1f))
            if (status.isNotEmpty()) {
                Text(status, style = ShepType.meta)
                Spacer(Modifier.width(ShepSpace.medium))
            }
            if (tasks.any { !taskIsOpen(it.state) && it.state != "running" }) {
                Text(
                    "clear done",
                    style = ShepType.actionQuiet,
                    modifier = Modifier.clickable {
                        act("cleared finished tasks", "task.clear", JSONObject())
                    },
                )
                Spacer(Modifier.width(ShepSpace.medium))
            }
            Text(
                "+ new",
                style = ShepType.actionStrong,
                modifier = Modifier.clickable { onShowAddChange(true) },
            )
        }
        notice?.let {
            Text(
                it,
                style = ShepType.meta.copy(color = ShepPalette.peach),
                modifier = Modifier.padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
            )
        }
        if (tasks.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no tasks — queue one with + new", style = ShepType.emptyState)
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.medium),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(tasks, key = { it.id }) { task ->
                    TaskCard(
                        task = task,
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
        dev.shep.companion.screens.AssignTaskSheet(
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
                                    .put("workspace_id", row.workspaceId),
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
            onDismiss = { onShowAddChange(false) },
            onSubmit = { prompt, repo, runtime, worktree ->
                onShowAddChange(false)
                act(
                    "queued task",
                    "task.add",
                    JSONObject()
                        .put("prompt", prompt)
                        .put("repo", repo)
                        .put("runtime", runtime)
                        .put("worktree", worktree),
                )
            },
        )
    }
}

@Composable
fun TaskCard(
    task: TaskRow,
    onDispatch: () -> Unit,
    onCancel: () -> Unit,
    onAssign: () -> Unit = {},
    onRemove: () -> Unit = {},
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(ShepShape.card)
            .background(ShepPalette.surface0)
            .padding(ShepSpace.card),
    ) {
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
        Spacer(Modifier.height(ShepSpace.small))
        Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.small)) {
            if (taskIsOpen(task.state)) {
                // "send to" leads: handing work to an agent already sitting in
                // the right repo is the cheaper move, and dispatch — which
                // spawns a whole new pane — is the fallback, not the default.
                Box(
                    Modifier
                        .clip(ShepShape.button)
                        .background(ShepPalette.accent)
                        .clickable { onAssign() }
                        .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                ) { Text("send to…", style = ShepType.action.copy(color = ShepPalette.panelBg)) }
                Box(
                    Modifier
                        .clip(ShepShape.button)
                        .background(ShepPalette.surface0)
                        .clickable { onDispatch() }
                        .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                ) { Text("new pane", style = ShepType.actionQuiet) }
                Box(
                    Modifier
                        .clip(ShepShape.button)
                        .background(ShepPalette.surface0)
                        .clickable { onCancel() }
                        .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                ) { Text("cancel", style = ShepType.actionQuiet) }
            }
            Spacer(Modifier.weight(1f))
            // Always removable. A queue you cannot empty stops being a queue.
            Text(
                "remove",
                style = ShepType.actionQuiet,
                modifier = Modifier
                    .clickable { onRemove() }
                    .padding(horizontal = ShepSpace.small, vertical = ShepSpace.snug),
            )
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddTaskSheet(
    knownRepos: List<String>,
    onDismiss: () -> Unit,
    onSubmit: (prompt: String, repo: String, runtime: String, worktree: Boolean) -> Unit,
) {
    val sheetState = rememberModalBottomSheetState()
    var prompt by remember { mutableStateOf("") }
    var repo by remember { mutableStateOf(knownRepos.firstOrNull() ?: "") }
    var runtime by remember { mutableStateOf("claude") }
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

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepPalette.surfaceDim) {
        Column(Modifier.fillMaxWidth().padding(ShepSpace.screen).imePadding()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("new task", style = ShepType.sheetTitle)
                Spacer(Modifier.weight(1f))
                Text(
                    "voice",
                    style = ShepType.actionStrong,
                    modifier = Modifier
                        .clip(ShepShape.pill)
                        .background(ShepPalette.surface0)
                        .clickable {
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
                        .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                )
            }
            voiceError?.let {
                Spacer(Modifier.height(ShepSpace.tight))
                Text(it, style = ShepType.meta.copy(color = ShepPalette.peach))
            }
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = prompt,
                onValueChange = { prompt = it },
                placeholder = { Text("prompt for the agent…", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 4,
            )
            Spacer(Modifier.height(ShepSpace.small))
            OutlinedTextField(
                value = repo,
                onValueChange = { repo = it },
                label = { Text("repo path", style = ShepType.fieldLabel) },
                placeholder = { Text("/Users/alex/vault/dev/…", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            if (knownRepos.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(top = ShepSpace.snug),
                    horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
                ) {
                    knownRepos.forEach { r ->
                        Box(
                            Modifier
                                .clip(ShepShape.pill)
                                .background(if (r == repo) ShepPalette.accent else ShepPalette.surface0)
                                .clickable { repo = r }
                                .padding(horizontal = ShepSpace.small, vertical = ShepSpace.snug),
                        ) {
                            Text(
                                repoName(r),
                                style = ShepType.chip.copy(
                                    color = if (r == repo) ShepPalette.panelBg else ShepPalette.subtext0,
                                ),
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.height(ShepSpace.medium))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("runtime", style = ShepType.sectionLabel)
                Spacer(Modifier.width(ShepSpace.medium))
                listOf("claude", "opencode").forEach { rt ->
                    Box(
                        Modifier
                            .clip(ShepShape.pill)
                            .background(if (rt == runtime) ShepPalette.accent else ShepPalette.surface0)
                            .clickable { runtime = rt }
                            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                    ) {
                        Text(
                            rt,
                            style = ShepType.chip.copy(
                                color = if (rt == runtime) ShepPalette.panelBg else ShepPalette.subtext0,
                            ),
                        )
                    }
                    Spacer(Modifier.width(ShepSpace.small))
                }
            }
            Spacer(Modifier.height(ShepSpace.small))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Switch(checked = worktree, onCheckedChange = { worktree = it })
                Spacer(Modifier.width(ShepSpace.small))
                Text("isolate in a worktree", style = ShepType.itemName)
            }
            Spacer(Modifier.height(ShepSpace.screen))
            Button(
                onClick = { onSubmit(prompt.trim(), repo.trim(), runtime, worktree) },
                enabled = prompt.isNotBlank() && repo.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("queue task", style = ShepType.button) }
            Spacer(Modifier.height(ShepSpace.small))
        }
    }
}

/**
 * Memory tab (A4): the USER profile's entries with a cap meter and add / edit /
 * remove. Backed by the bridge-local `memory.show/add/replace/remove`.
 */
@Composable
fun MemoryScreen(client: BridgeClient) {
    var view by remember { mutableStateOf<MemoryView?>(null) }
    var status by remember { mutableStateOf("loading") }
    var notice by remember { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf<String?>(null) } // existing entry text, or "" for a new entry
    val scope = rememberCoroutineScope()

    suspend fun refresh() {
        withContext(Dispatchers.IO) { runCatching { client.call("memory.show") } }
            .onSuccess { view = parseMemory(it); status = "" }
            .onFailure { status = "reconnect: ${it.message}" }
    }
    LaunchedEffect(client) { refresh() }

    fun mutate(method: String, params: JSONObject, label: String) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { view = parseMemory(it); notice = label; editing = null }
                .onFailure { notice = it.message }
        }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.screen),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("memory", style = ShepType.screenTitle)
            Spacer(Modifier.width(ShepSpace.small))
            Text("· user", style = ShepType.meta)
            Spacer(Modifier.weight(1f))
            Text(
                "+ add",
                style = ShepType.actionStrong,
                modifier = Modifier.clickable { editing = "" },
            )
        }
        val v = view
        if (v != null) {
            val overCap = v.percent >= 80
            Column(Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(horizontal = ShepSpace.screen, vertical = ShepSpace.small)) {
                // Two boxes rather than a LinearProgressIndicator: Material's
                // draws a "stop indicator" dot at the track end, so an empty
                // memory rendered a copper mark at 100% and read as full. This
                // is also the same bar the board card's context gauge draws,
                // which is the point — one meter, one look.
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(ShepSize.meterHeight)
                        .clip(ShepShape.bar)
                        .background(ShepPalette.surface0),
                ) {
                    Box(
                        Modifier
                            .fillMaxWidth((v.percent / 100f).coerceIn(0f, 1f))
                            .height(ShepSize.meterHeight)
                            .clip(ShepShape.bar)
                            .background(if (overCap) ShepPalette.peach else ShepPalette.accent),
                    )
                }
                Spacer(Modifier.height(ShepSpace.tight))
                Text(
                    "${v.used}/${v.cap} chars · ${v.percent}%" + if (overCap) " — consolidate soon" else "",
                    style = ShepType.meta.copy(
                        color = if (overCap) ShepPalette.peach else ShepPalette.overlay0,
                    ),
                )
            }
        }
        notice?.let {
            Text(
                it,
                style = ShepType.meta.copy(color = ShepPalette.peach),
                modifier = Modifier.padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
            )
        }
        if (v == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(if (status.isEmpty()) "loading…" else status, style = ShepType.emptyState)
            }
        } else if (v.entries.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no entries yet — add one with + add", style = ShepType.emptyState)
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.medium),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(v.entries, key = { it }) { entry ->
                    MemoryCard(
                        entry = entry,
                        onEdit = { editing = entry },
                        onRemove = { mutate("memory.remove", JSONObject().put("find", entry), "removed") },
                    )
                }
            }
        }
    }

    val target = editing
    if (target != null) {
        MemoryEditSheet(
            initial = target,
            onDismiss = { editing = null },
            onSave = { text ->
                if (target.isEmpty()) {
                    mutate("memory.add", JSONObject().put("text", text), "added")
                } else {
                    mutate("memory.replace", JSONObject().put("find", target).put("text", text), "updated")
                }
            },
        )
    }
}

@Composable
fun MemoryCard(entry: String, onEdit: () -> Unit, onRemove: () -> Unit) {
    Column(
        Modifier.fillMaxWidth().clip(ShepShape.card).background(ShepPalette.surface0).padding(ShepSpace.card),
    ) {
        // A memory entry is a sentence a person wrote. Sans.
        Text(entry, style = ShepType.body)
        Spacer(Modifier.height(ShepSpace.small))
        Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.screen)) {
            Text("edit", style = ShepType.action.copy(color = ShepPalette.accent), modifier = Modifier.clickable { onEdit() })
            Text("remove", style = ShepType.actionQuiet, modifier = Modifier.clickable { onRemove() })
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemoryEditSheet(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    var text by remember { mutableStateOf(initial) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepPalette.surfaceDim) {
        Column(Modifier.fillMaxWidth().padding(ShepSpace.screen).imePadding()) {
            Text(
                if (initial.isEmpty()) "add entry" else "edit entry",
                style = ShepType.sheetTitle,
            )
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                placeholder = { Text("one fact, present tense…", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 6,
            )
            Spacer(Modifier.height(ShepSpace.screen))
            Button(
                onClick = { onSave(text.trim()) },
                enabled = text.isNotBlank() && text.trim() != initial,
                modifier = Modifier.fillMaxWidth(),
            ) { Text(if (initial.isEmpty()) "add" else "save", style = ShepType.button) }
            Spacer(Modifier.height(ShepSpace.small))
        }
    }
}

/** Parse a `shep://pair?url=…&token=…` payload from a scanned QR. */
fun parsePairingUri(raw: String): Pair<String, String>? = runCatching {
    val uri = android.net.Uri.parse(raw.trim())
    if (uri.scheme == "shep" && uri.host == "pair") {
        val u = uri.getQueryParameter("url")
        val t = uri.getQueryParameter("token")
        if (!u.isNullOrBlank() && !t.isNullOrBlank()) return@runCatching u to t
    }
    null
}.getOrNull()

@Composable
fun PairingScreen(
    initialUrl: String,
    initialToken: String,
    lastError: String?,
    onConnect: (String, String, (String?) -> Unit) -> Unit,
) {
    var url by remember { mutableStateOf(initialUrl.ifEmpty { "ws://100.64.0.0:7431/" }) }
    var token by remember { mutableStateOf(initialToken) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf(lastError) }

    val scanLauncher = rememberLauncherForActivityResult(
        com.journeyapps.barcodescanner.ScanContract()
    ) { result ->
        val contents = result.contents ?: return@rememberLauncherForActivityResult
        val parsed = parsePairingUri(contents)
        if (parsed == null) {
            error = "unrecognized QR — expected `shep://pair`"
        } else {
            url = parsed.first
            token = parsed.second
            busy = true
            error = null
            onConnect(parsed.first.trim(), parsed.second.filterNot { it.isWhitespace() }) { failure ->
                busy = false
                error = failure
            }
        }
    }

    Column(
        Modifier.fillMaxSize().padding(ShepSpace.section).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("shep", style = ShepType.hero)
        Text(
            "the cockpit in your pocket",
            style = ShepType.meta,
            modifier = Modifier.padding(bottom = ShepSpace.section),
        )
        Button(
            onClick = {
                error = null
                scanLauncher.launch(
                    com.journeyapps.barcodescanner.ScanOptions().apply {
                        setDesiredBarcodeFormats(com.journeyapps.barcodescanner.ScanOptions.QR_CODE)
                        setPrompt("Scan the QR from `shep bridge pair`")
                        setBeepEnabled(false)
                        setOrientationLocked(false)
                    }
                )
            },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
        ) {
            Text("Scan QR to pair", style = ShepType.button)
        }
        Text(
            "— or enter manually —",
            style = ShepType.meta,
            modifier = Modifier.fillMaxWidth().padding(vertical = ShepSpace.screen),
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
        OutlinedTextField(
            value = url,
            onValueChange = { url = it },
            label = { Text("Bridge URL", style = ShepType.fieldLabel) },
            singleLine = true,
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri,
                autoCorrectEnabled = false,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(ShepSpace.medium))
        OutlinedTextField(
            value = token,
            onValueChange = { token = it },
            label = { Text("Token", style = ShepType.fieldLabel) },
            singleLine = true,
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                keyboardType = androidx.compose.ui.text.input.KeyboardType.Password,
                autoCorrectEnabled = false,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(ShepSpace.small))
        Text(
            "Run `shep bridge pair --host <tailnet-ip>` on the server for these values.",
            style = ShepType.meta,
        )
        error?.let {
            Spacer(Modifier.height(ShepSpace.medium))
            Text(it, style = ShepType.meta.copy(color = ShepPalette.red))
        }
        Spacer(Modifier.height(ShepSpace.indent))
        Button(
            onClick = {
                busy = true
                error = null
                onConnect(url.trim(), token.filterNot { it.isWhitespace() }) { failure ->
                    busy = false
                    error = failure
                }
            },
            enabled = !busy && url.isNotBlank() && token.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
        ) {
            Text(if (busy) "Connecting..." else "Connect", style = ShepType.button)
        }
    }
}

// Bridge protocol the app was built against (shep src/protocol/wire.rs
// PROTOCOL_VERSION at vendor time). A mismatch soft-warns; it does not brick a
// personal sideload. Bump alongside the vendored schema (1f follow-up).
const val EXPECTED_PROTOCOL = 16

// Structural subscriptions (no pane arg) that signal the agent list changed
// shape; a re-snapshot on any of these keeps Home in sync without per-pane subs.
private val STRUCTURAL_SUBSCRIPTIONS = listOf(
    "workspace.updated", "workspace.created", "workspace.closed", "workspace.renamed",
    "pane.created", "pane.closed", "pane.exited", "pane.agent_detected", "layout.updated",
)

/** Tint diff lines: green added, red removed, copper hunk headers, dim context. */
fun colorizeDiff(diff: String): androidx.compose.ui.text.AnnotatedString =
    buildAnnotatedString {
        diff.lineSequence().forEach { line ->
            val color = when {
                line.startsWith("+++") || line.startsWith("---") -> ShepPalette.overlay0
                line.startsWith("@@") -> ShepPalette.accent
                line.startsWith("+") -> ShepPalette.green
                line.startsWith("-") -> ShepPalette.red
                else -> ShepPalette.text
            }
            withStyle(SpanStyle(color = color)) { append(line) }
            append("\n")
        }
    }

/**
 * Review & Ship (A5): the workspace's diff via `workspace.diff`, with Request
 * changes (feedback → agent pane + `workspace.set_review_state`) and, for a
 * linked worktree, Ship (`workspace.ship` merge → `worktree.remove` cleanup).
 * Reached through Agents → pane → review.
 */
@Composable
fun ReviewScreen(client: BridgeClient, row: AgentRow, onBack: () -> Unit) {
    var stat by remember { mutableStateOf("") }
    var diff by remember { mutableStateOf(androidx.compose.ui.text.AnnotatedString("")) }
    var status by remember { mutableStateOf("loading diff…") }
    var notice by remember { mutableStateOf<String?>(null) }
    var requesting by remember { mutableStateOf(false) }
    var confirmShip by remember { mutableStateOf(false) }
    var shipping by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    val scroll = rememberScrollState()

    LaunchedEffect(row.workspaceId) {
        withContext(Dispatchers.IO) {
            runCatching { client.call("workspace.diff", JSONObject().put("workspace_id", row.workspaceId), 20) }
        }.onSuccess {
            stat = it.optString("stat")
            val d = it.optString("diff")
            diff = if (d.isEmpty()) androidx.compose.ui.text.AnnotatedString("") else colorizeDiff(d)
            status = if (stat.isEmpty() && d.isEmpty()) "no changes to review" else ""
        }.onFailure { status = "diff failed: ${it.message}" }
    }

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.medium),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                "←",
                style = ShepType.screenTitle.copy(color = ShepPalette.accent),
                modifier = Modifier.clickable { onBack() }.padding(end = ShepSpace.medium),
            )
            Column(Modifier.weight(1f)) {
                Text("review · ${row.workspaceLabel}", style = ShepType.agentName)
                Text(
                    row.worktreeRepo?.let { "$it${if (row.isWorktree) " · worktree" else ""}" } ?: "working tree",
                    style = ShepType.meta,
                )
            }
        }
        notice?.let {
            Text(
                it,
                style = ShepType.meta.copy(color = ShepPalette.peach),
                modifier = Modifier.padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
            )
        }
        if (stat.isNotEmpty()) {
            Text(
                stat,
                style = ShepType.codeSmall.copy(color = ShepPalette.overlay0),
                modifier = Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.small),
            )
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(ShepPalette.panelBg)) {
            if (diff.text.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(status.ifEmpty { "no changes" }, style = ShepType.emptyState)
                }
            } else {
                Text(
                    diff,
                    style = ShepType.codeSmall,
                    modifier = Modifier.fillMaxSize().verticalScroll(scroll).padding(ShepSpace.small),
                )
            }
        }
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.small),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.weight(1f).clip(ShepShape.button).background(ShepPalette.surface0)
                    .clickable { requesting = true }.padding(vertical = ShepSpace.small),
                contentAlignment = Alignment.Center,
            ) { Text("request changes", style = ShepType.action.copy(color = ShepPalette.peach)) }
            if (row.isWorktree) {
                Box(
                    Modifier.weight(1f).clip(ShepShape.button)
                        .background(if (shipping) ShepPalette.surface0 else ShepPalette.accent)
                        .clickable(enabled = !shipping) { confirmShip = true }.padding(vertical = ShepSpace.small),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (shipping) "shipping…" else "ship ⑂",
                        style = ShepType.action.copy(color = ShepPalette.panelBg),
                    )
                }
            }
        }
    }

    if (requesting) {
        RequestChangesSheet(
            onDismiss = { requesting = false },
            onSubmit = { feedback ->
                requesting = false
                scope.launch {
                    val ok = withContext(Dispatchers.IO) {
                        runCatching {
                            // Feedback into the agent pane, then flag the workspace.
                            client.call("agent.send", JSONObject().put("target", row.paneId).put("text", feedback))
                            client.call(
                                "workspace.set_review_state",
                                JSONObject().put("workspace_id", row.workspaceId).put("review_state", "changes_requested"),
                            )
                        }.isSuccess
                    }
                    notice = if (ok) "changes requested — sent to the agent" else "failed to send feedback"
                }
            },
        )
    }

    if (confirmShip) {
        AlertDialog(
            onDismissRequest = { confirmShip = false },
            containerColor = ShepPalette.surfaceDim,
            title = { Text("Ship this worktree?", style = ShepType.sheetTitle) },
            text = {
                Text(
                    "Merge ${row.worktreeRepo ?: "this worktree"}'s branch into its base checkout, then remove the worktree. " +
                        "Refuses if either side is dirty or the merge conflicts. This can't be undone.",
                    style = ShepType.bodySmall,
                )
            },
            confirmButton = {
                Text(
                    "Merge & ship",
                    style = ShepType.button.copy(color = ShepPalette.accent),
                    modifier = Modifier.clickable {
                        confirmShip = false
                        shipping = true
                        scope.launch {
                            val result = withContext(Dispatchers.IO) {
                                runCatching {
                                    val shipped = client.call("workspace.ship", JSONObject().put("workspace_id", row.workspaceId), 30)
                                    // Cleanup: remove the now-merged worktree (async server op).
                                    runCatching {
                                        client.call("worktree.remove", JSONObject().put("workspace_id", row.workspaceId), 30)
                                    }
                                    shipped.optString("message", "shipped")
                                }
                            }
                            shipping = false
                            result.onSuccess { notice = "✓ $it"; onBack() }.onFailure { notice = "ship failed: ${it.message}" }
                        }
                    }.padding(ShepSpace.small),
                )
            },
            dismissButton = {
                Text(
                    "Cancel",
                    style = ShepType.button.copy(color = ShepPalette.overlay0),
                    modifier = Modifier.clickable { confirmShip = false }.padding(ShepSpace.small),
                )
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RequestChangesSheet(onDismiss: () -> Unit, onSubmit: (String) -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    var text by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepPalette.surfaceDim) {
        Column(Modifier.fillMaxWidth().padding(ShepSpace.screen).imePadding()) {
            Text("request changes", style = ShepType.sheetTitle)
            Spacer(Modifier.height(ShepSpace.snug))
            Text(
                "goes straight into the agent's pane and flags the workspace.",
                style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
            )
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                placeholder = { Text("what needs to change…", style = ShepType.fieldLabel) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 6,
            )
            Spacer(Modifier.height(ShepSpace.screen))
            Button(onClick = { onSubmit(text.trim()) }, enabled = text.isNotBlank(), modifier = Modifier.fillMaxWidth()) {
                Text("send to agent", style = ShepType.button)
            }
            Spacer(Modifier.height(ShepSpace.small))
        }
    }
}

@Composable
fun HomeScreen(client: BridgeClient, onOpenPane: (AgentRow) -> Unit, onUnpair: () -> Unit) {
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
        val refreshSignal = kotlinx.coroutines.channels.Channel<Unit>(
            kotlinx.coroutines.channels.Channel.CONFLATED
        )
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
        Row(
            Modifier.fillMaxWidth().background(ShepPalette.surfaceDim).padding(ShepSpace.screen),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("board", style = ShepType.screenTitle)
            Spacer(Modifier.weight(1f))
            Text(status, style = ShepType.meta)
            Spacer(Modifier.width(ShepSpace.medium))
            Text(
                "+ new",
                style = ShepType.actionStrong,
                modifier = Modifier.clickable { showNew = true },
            )
            Spacer(Modifier.width(ShepSpace.medium))
            Text(
                "unpair",
                style = ShepType.meta,
                modifier = Modifier.clickable { onUnpair() },
            )
        }
        val serverProtocol = client.serverProtocol
        if (serverProtocol != null && serverProtocol != EXPECTED_PROTOCOL) {
            Text(
                "⚠ server protocol $serverProtocol · app built for $EXPECTED_PROTOCOL — " +
                    if (serverProtocol > EXPECTED_PROTOCOL) "update the app" else "update the server",
                style = ShepType.meta.copy(color = ShepPalette.panelBg),
                modifier = Modifier
                    .fillMaxWidth()
                    .background(ShepPalette.peach)
                    .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
            )
        }
        notice?.let {
            Text(
                it,
                style = ShepType.meta.copy(color = ShepPalette.peach),
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { notice = null }
                    .padding(horizontal = ShepSpace.screen, vertical = ShepSpace.tight),
            )
        }
        dev.shep.companion.screens.DashboardStrip(totals, host) { statusColor(it) }
        FilterChips(filter, rows) { filter = it }
        if (visibleRows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    if (rows.isEmpty()) "no sessions — start one with + new"
                    else "nothing ${filter.label}",
                    style = ShepType.emptyState,
                )
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.medium),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(visibleRows, key = { it.paneId }) { row ->
                    dev.shep.companion.screens.BoardCard(
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
        dev.shep.companion.screens.NewSessionSheet(
            recentRepos = recentRepos,
            onDismiss = { showNew = false },
            onStart = { cwd, name, runtime ->
                showNew = false
                startSession(cwd, name, runtime)
            },
        )
    }
    renaming?.let { row ->
        dev.shep.companion.screens.RenameSessionSheet(
            row = row,
            onDismiss = { renaming = null },
            onRename = { name ->
                renaming = null
                renameSession(row, name)
            },
        )
    }
}

/** Home filter chips. "attention" (blocked + needs-review) is the default. */
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
fun FilterChips(selected: HomeFilter, rows: List<AgentRow>, onSelect: (HomeFilter) -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
    ) {
        HomeFilter.entries.forEach { entry ->
            val count = rows.count { entry.accepts(it.status) }
            val active = entry == selected
            Box(
                Modifier
                    .clip(ShepShape.pill)
                    .background(if (active) ShepPalette.accent else ShepPalette.surface0)
                    .clickable { onSelect(entry) }
                    .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
            ) {
                Text(
                    "${entry.label} $count",
                    style = ShepType.chip.copy(
                        color = if (active) ShepPalette.panelBg else ShepPalette.subtext0,
                        fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
                    ),
                )
            }
        }
    }
}
