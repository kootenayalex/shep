package dev.shep.companion

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import dev.shep.companion.screens.SessionRuntime
import dev.shep.companion.screens.pane.PaneScreen
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepTheme

/**
 * Screen-level color names, resolved from the one palette.
 *
 * These were hand-picked approximations of the TUI's colors; they now delegate
 * to [ShepPalette], which mirrors `Palette::shep()` exactly. Same names, same
 * call sites, but the app and the desktop finally render in the same ink.
 */
object ShepColors {
    val bg = ShepPalette.panelBg
    val surface = ShepPalette.surfaceDim
    val surfaceHigh = ShepPalette.surface0
    val text = ShepPalette.text
    val subtext = ShepPalette.overlay1
    val copper = ShepPalette.accent  // accent / working
    val green = ShepPalette.green    // idle / approved
    val red = ShepPalette.red        // blocked
    val blue = ShepPalette.blue      // done (unseen)
    val peach = ShepPalette.peach    // warning tier
}

fun statusColor(status: String): Color = when (status) {
    "blocked" -> ShepColors.red
    "working" -> ShepColors.copper
    "done" -> ShepColors.blue
    "idle" -> ShepColors.green
    else -> ShepColors.subtext
}

/** Alias for screens that live outside this file. */
fun statusColorFor(status: String): Color = statusColor(status)

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
        color = ShepColors.bg,
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
            CircularProgressIndicator(color = ShepColors.copper)
            Spacer(Modifier.height(16.dp))
            Text(label, color = ShepColors.subtext)
            error?.let {
                Spacer(Modifier.height(6.dp))
                Text(it, color = ShepColors.peach, fontSize = 12.sp)
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
        val wide = maxWidth >= 720.dp
        val detail = paneDetail

        if (detail != null && !wide) {
            BackHandler { paneDetail = null }
            PaneScreen(client, detail, onBack = { paneDetail = null })
            return@BoxWithConstraints
        }

        Scaffold(
            containerColor = ShepColors.bg,
            bottomBar = {
                NavigationBar(
                    containerColor = ShepColors.surface,
                    modifier = Modifier.navigationBarsPadding(),
                ) {
                    Tab.entries.forEach { entry ->
                        NavigationBarItem(
                            selected = tab == entry,
                            onClick = { tab = entry },
                            icon = { Text(entry.glyph, fontSize = 18.sp) },
                            label = { Text(entry.label, fontSize = 11.sp) },
                            colors = NavigationBarItemDefaults.colors(
                                selectedIconColor = ShepColors.copper,
                                selectedTextColor = ShepColors.copper,
                                unselectedIconColor = ShepColors.subtext,
                                unselectedTextColor = ShepColors.subtext,
                                indicatorColor = ShepColors.surfaceHigh,
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
                                .background(ShepColors.bg),
                        ) {
                            if (detail != null) {
                                PaneScreen(client, detail, onBack = { paneDetail = null })
                            } else {
                                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                                    Text("select an agent", color = ShepColors.subtext)
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
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("shep", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
        }
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text("notify me about", color = ShepColors.text, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(2.dp))
            Text(
                "shep stops sending what is off here, so it costs no battery.",
                color = ShepColors.subtext,
                fontSize = 11.sp,
            )
            Spacer(Modifier.height(8.dp))
            NotifyKind.entries.forEach { kind ->
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable {
                            val next = if (kind in kinds) kinds - kind else kinds + kind
                            kinds = next
                            FcmManager.setKinds(context, next) { status = it }
                        }
                        .padding(vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(kind.label, color = ShepColors.text, fontSize = 14.sp)
                        Text(
                            kind.description,
                            color = ShepColors.subtext,
                            fontSize = 11.sp,
                        )
                    }
                    Switch(
                        checked = kind in kinds,
                        onCheckedChange = { on ->
                            val next = if (on) kinds + kind else kinds - kind
                            kinds = next
                            FcmManager.setKinds(context, next) { status = it }
                        },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = ShepColors.bg,
                            checkedTrackColor = ShepColors.copper,
                            uncheckedThumbColor = ShepColors.subtext,
                            uncheckedTrackColor = ShepColors.surfaceHigh,
                        ),
                    )
                }
            }

            Spacer(Modifier.height(20.dp))
            Text("delivery", color = ShepColors.text, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(6.dp))
            Text(status, color = ShepColors.subtext, fontSize = 13.sp)
            Text(
                if (token != null) "registered with FCM" else "no FCM token yet",
                color = if (token != null) ShepColors.green else ShepColors.peach,
                fontSize = 12.sp,
            )
            Spacer(Modifier.height(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
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
                ) { Text(if (testing) "sending…" else "Send test notification") }
                TextButton(onClick = {
                    FcmManager.register(context, kinds)
                    status = "registering…"
                    token = prefs.getString("fcm_token", null)
                }) { Text("Re-register", color = ShepColors.subtext) }
            }
            testResult?.let {
                Spacer(Modifier.height(10.dp))
                Text(
                    it,
                    color = if (it.startsWith("sent to")) ShepColors.green else ShepColors.peach,
                    fontSize = 12.sp,
                )
            }
            Spacer(Modifier.height(24.dp))
        }
    }
}

/** Color for a task lifecycle state, reusing the shep attention vocabulary. */
fun taskStateColor(state: String): Color = when (state) {
    "blocked" -> ShepColors.red
    "running" -> ShepColors.copper
    "done" -> ShepColors.green
    "cancelled" -> ShepColors.subtext
    else -> ShepColors.blue // todo (queued, unseen)
}

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
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("tasks", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            if (status.isNotEmpty()) {
                Text(status, color = ShepColors.subtext, fontSize = 12.sp)
                Spacer(Modifier.width(12.dp))
            }
            if (tasks.any { !taskIsOpen(it.state) && it.state != "running" }) {
                Text(
                    "clear done",
                    color = ShepColors.subtext,
                    fontSize = 13.sp,
                    modifier = Modifier.clickable {
                        act("cleared finished tasks", "task.clear", JSONObject())
                    },
                )
                Spacer(Modifier.width(12.dp))
            }
            Text(
                "+ new",
                color = ShepColors.copper,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clickable { onShowAddChange(true) },
            )
        }
        notice?.let {
            Text(
                it,
                color = ShepColors.peach,
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
            )
        }
        if (tasks.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no tasks — queue one with + new", color = ShepColors.subtext)
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
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
            .clip(RoundedCornerShape(12.dp))
            .background(ShepColors.surface)
            .padding(14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(10.dp).clip(CircleShape).background(taskStateColor(task.state)))
            Spacer(Modifier.width(8.dp))
            Text("#${task.id}", color = ShepColors.subtext, fontSize = 12.sp)
            Spacer(Modifier.width(8.dp))
            Text(task.state, color = taskStateColor(task.state), fontSize = 12.sp)
            Spacer(Modifier.weight(1f))
            if (task.useWorktree) Text("⑂", color = ShepColors.copper, fontSize = 13.sp)
        }
        Spacer(Modifier.height(6.dp))
        Text(task.prompt, color = ShepColors.text, fontSize = 14.sp, maxLines = 3, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(6.dp))
        Text(
            "${repoName(task.repo)} · ${task.runtime}" + (task.workspaceId?.let { " · $it" } ?: ""),
            color = ShepColors.subtext,
            fontSize = 12.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (taskIsOpen(task.state)) {
                // "send to" leads: handing work to an agent already sitting in
                // the right repo is the cheaper move, and dispatch — which
                // spawns a whole new pane — is the fallback, not the default.
                Box(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(ShepColors.copper)
                        .clickable { onAssign() }
                        .padding(horizontal = 14.dp, vertical = 6.dp),
                ) { Text("send to…", color = ShepColors.bg, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
                Box(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(ShepColors.surfaceHigh)
                        .clickable { onDispatch() }
                        .padding(horizontal = 14.dp, vertical = 6.dp),
                ) { Text("new pane", color = ShepColors.subtext, fontSize = 13.sp) }
                Box(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(ShepColors.surfaceHigh)
                        .clickable { onCancel() }
                        .padding(horizontal = 14.dp, vertical = 6.dp),
                ) { Text("cancel", color = ShepColors.subtext, fontSize = 13.sp) }
            }
            Spacer(Modifier.weight(1f))
            // Always removable. A queue you cannot empty stops being a queue.
            Text(
                "remove",
                color = ShepColors.subtext,
                fontSize = 13.sp,
                modifier = Modifier
                    .clickable { onRemove() }
                    .padding(horizontal = 8.dp, vertical = 6.dp),
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

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepColors.surface) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("new task", color = ShepColors.text, fontSize = 18.sp, fontWeight = FontWeight.Bold)
                Spacer(Modifier.weight(1f))
                Text(
                    "voice",
                    color = ShepColors.copper,
                    fontSize = 14.sp,
                    fontWeight = FontWeight.SemiBold,
                    modifier = Modifier
                        .clip(RoundedCornerShape(14.dp))
                        .background(ShepColors.surfaceHigh)
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
                        .padding(horizontal = 12.dp, vertical = 6.dp),
                )
            }
            voiceError?.let {
                Spacer(Modifier.height(4.dp))
                Text(it, color = ShepColors.peach, fontSize = 12.sp)
            }
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = prompt,
                onValueChange = { prompt = it },
                placeholder = { Text("prompt for the agent…", color = ShepColors.subtext) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 4,
            )
            Spacer(Modifier.height(10.dp))
            OutlinedTextField(
                value = repo,
                onValueChange = { repo = it },
                label = { Text("repo path", color = ShepColors.subtext) },
                placeholder = { Text("/Users/alex/vault/dev/…", color = ShepColors.subtext) },
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
            if (knownRepos.isNotEmpty()) {
                Row(
                    Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(top = 6.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    knownRepos.forEach { r ->
                        Box(
                            Modifier
                                .clip(RoundedCornerShape(14.dp))
                                .background(if (r == repo) ShepColors.copper else ShepColors.surfaceHigh)
                                .clickable { repo = r }
                                .padding(horizontal = 10.dp, vertical = 5.dp),
                        ) {
                            Text(
                                repoName(r),
                                color = if (r == repo) ShepColors.bg else ShepColors.subtext,
                                fontSize = 12.sp,
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.height(12.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("runtime", color = ShepColors.subtext, fontSize = 13.sp)
                Spacer(Modifier.width(12.dp))
                listOf("claude", "opencode").forEach { rt ->
                    Box(
                        Modifier
                            .clip(RoundedCornerShape(14.dp))
                            .background(if (rt == runtime) ShepColors.copper else ShepColors.surfaceHigh)
                            .clickable { runtime = rt }
                            .padding(horizontal = 12.dp, vertical = 6.dp),
                    ) {
                        Text(rt, color = if (rt == runtime) ShepColors.bg else ShepColors.subtext, fontSize = 12.sp)
                    }
                    Spacer(Modifier.width(8.dp))
                }
            }
            Spacer(Modifier.height(8.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Switch(checked = worktree, onCheckedChange = { worktree = it })
                Spacer(Modifier.width(8.dp))
                Text("isolate in a worktree", color = ShepColors.text, fontSize = 14.sp)
            }
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = { onSubmit(prompt.trim(), repo.trim(), runtime, worktree) },
                enabled = prompt.isNotBlank() && repo.isNotBlank(),
                modifier = Modifier.fillMaxWidth(),
            ) { Text("queue task") }
            Spacer(Modifier.height(8.dp))
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
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("memory", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.width(8.dp))
            Text("· user", color = ShepColors.subtext, fontSize = 13.sp)
            Spacer(Modifier.weight(1f))
            Text(
                "+ add",
                color = ShepColors.copper,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clickable { editing = "" },
            )
        }
        val v = view
        if (v != null) {
            val overCap = v.percent >= 80
            Column(Modifier.fillMaxWidth().background(ShepColors.surface).padding(horizontal = 16.dp, vertical = 8.dp)) {
                LinearProgressIndicator(
                    progress = { (v.percent / 100f).coerceIn(0f, 1f) },
                    modifier = Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)),
                    color = if (overCap) ShepColors.peach else ShepColors.copper,
                    trackColor = ShepColors.surfaceHigh,
                )
                Spacer(Modifier.height(4.dp))
                Text(
                    "${v.used}/${v.cap} chars · ${v.percent}%" + if (overCap) " — consolidate soon" else "",
                    color = if (overCap) ShepColors.peach else ShepColors.subtext,
                    fontSize = 12.sp,
                )
            }
        }
        notice?.let {
            Text(it, color = ShepColors.peach, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp))
        }
        if (v == null) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(if (status.isEmpty()) "loading…" else status, color = ShepColors.subtext)
            }
        } else if (v.entries.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no entries yet — add one with + add", color = ShepColors.subtext)
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
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
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(ShepColors.surface).padding(14.dp),
    ) {
        Text(entry, color = ShepColors.text, fontSize = 14.sp)
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("edit", color = ShepColors.copper, fontSize = 13.sp, modifier = Modifier.clickable { onEdit() })
            Text("remove", color = ShepColors.subtext, fontSize = 13.sp, modifier = Modifier.clickable { onRemove() })
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemoryEditSheet(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    var text by remember { mutableStateOf(initial) }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepColors.surface) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Text(
                if (initial.isEmpty()) "add entry" else "edit entry",
                color = ShepColors.text,
                fontSize = 18.sp,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                placeholder = { Text("one fact, present tense…", color = ShepColors.subtext) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 6,
            )
            Spacer(Modifier.height(16.dp))
            Button(
                onClick = { onSave(text.trim()) },
                enabled = text.isNotBlank() && text.trim() != initial,
                modifier = Modifier.fillMaxWidth(),
            ) { Text(if (initial.isEmpty()) "add" else "save") }
            Spacer(Modifier.height(8.dp))
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
        Modifier.fillMaxSize().padding(24.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("shep", color = ShepColors.copper, fontSize = 40.sp, fontWeight = FontWeight.Bold)
        Text(
            "the cockpit in your pocket",
            color = ShepColors.subtext,
            modifier = Modifier.padding(bottom = 24.dp),
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
            modifier = Modifier.fillMaxWidth().height(52.dp),
        ) {
            Text("Scan QR to pair", fontSize = 16.sp)
        }
        Text(
            "— or enter manually —",
            color = ShepColors.subtext,
            fontSize = 12.sp,
            modifier = Modifier.fillMaxWidth().padding(vertical = 16.dp),
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
        OutlinedTextField(
            value = url,
            onValueChange = { url = it },
            label = { Text("Bridge URL") },
            singleLine = true,
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri,
                autoCorrectEnabled = false,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(12.dp))
        OutlinedTextField(
            value = token,
            onValueChange = { token = it },
            label = { Text("Token") },
            singleLine = true,
            keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                keyboardType = androidx.compose.ui.text.input.KeyboardType.Password,
                autoCorrectEnabled = false,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(8.dp))
        Text(
            "Run `shep bridge pair --host <tailnet-ip>` on the server for these values.",
            color = ShepColors.subtext,
            fontSize = 12.sp,
        )
        error?.let {
            Spacer(Modifier.height(12.dp))
            Text(it, color = ShepColors.red)
        }
        Spacer(Modifier.height(20.dp))
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
            modifier = Modifier.fillMaxWidth().height(52.dp),
        ) {
            Text(if (busy) "Connecting..." else "Connect", fontSize = 16.sp)
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
                line.startsWith("+++") || line.startsWith("---") -> ShepColors.subtext
                line.startsWith("@@") -> ShepColors.copper
                line.startsWith("+") -> ShepColors.green
                line.startsWith("-") -> ShepColors.red
                else -> ShepColors.text
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
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("←", color = ShepColors.copper, fontSize = 22.sp, modifier = Modifier.clickable { onBack() }.padding(end = 14.dp))
            Column(Modifier.weight(1f)) {
                Text("review · ${row.workspaceLabel}", color = ShepColors.text, fontWeight = FontWeight.SemiBold)
                Text(
                    row.worktreeRepo?.let { "$it${if (row.isWorktree) " · worktree" else ""}" } ?: "working tree",
                    color = ShepColors.subtext,
                    fontSize = 12.sp,
                )
            }
        }
        notice?.let {
            Text(it, color = ShepColors.peach, fontSize = 12.sp, modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp))
        }
        if (stat.isNotEmpty()) {
            Text(
                stat,
                color = ShepColors.subtext,
                fontFamily = FontFamily.Monospace,
                fontSize = 11.sp,
                modifier = Modifier.fillMaxWidth().background(ShepColors.surface).padding(10.dp),
            )
        }
        Box(Modifier.weight(1f).fillMaxWidth().background(ShepColors.bg)) {
            if (diff.text.isEmpty()) {
                Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                    Text(status.ifEmpty { "no changes" }, color = ShepColors.subtext)
                }
            } else {
                Text(
                    diff,
                    fontFamily = FontFamily.Monospace,
                    fontSize = 11.sp,
                    lineHeight = 15.sp,
                    modifier = Modifier.fillMaxSize().verticalScroll(scroll).padding(10.dp),
                )
            }
        }
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(10.dp),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier.weight(1f).clip(RoundedCornerShape(8.dp)).background(ShepColors.surfaceHigh)
                    .clickable { requesting = true }.padding(vertical = 10.dp),
                contentAlignment = Alignment.Center,
            ) { Text("request changes", color = ShepColors.peach, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
            if (row.isWorktree) {
                Box(
                    Modifier.weight(1f).clip(RoundedCornerShape(8.dp))
                        .background(if (shipping) ShepColors.surfaceHigh else ShepColors.copper)
                        .clickable(enabled = !shipping) { confirmShip = true }.padding(vertical = 10.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(if (shipping) "shipping…" else "ship ⑂", color = ShepColors.bg, fontSize = 13.sp, fontWeight = FontWeight.SemiBold) }
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
            containerColor = ShepColors.surface,
            title = { Text("Ship this worktree?", color = ShepColors.text) },
            text = {
                Text(
                    "Merge ${row.worktreeRepo ?: "this worktree"}'s branch into its base checkout, then remove the worktree. " +
                        "Refuses if either side is dirty or the merge conflicts. This can't be undone.",
                    color = ShepColors.subtext,
                    fontSize = 13.sp,
                )
            },
            confirmButton = {
                Text(
                    "Merge & ship",
                    color = ShepColors.copper,
                    fontWeight = FontWeight.SemiBold,
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
                    }.padding(8.dp),
                )
            },
            dismissButton = {
                Text("Cancel", color = ShepColors.subtext, modifier = Modifier.clickable { confirmShip = false }.padding(8.dp))
            },
        )
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RequestChangesSheet(onDismiss: () -> Unit, onSubmit: (String) -> Unit) {
    val sheetState = rememberModalBottomSheetState()
    var text by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = ShepColors.surface) {
        Column(Modifier.fillMaxWidth().padding(16.dp).imePadding()) {
            Text("request changes", color = ShepColors.text, fontSize = 18.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.height(6.dp))
            Text("goes straight into the agent's pane and flags the workspace.", color = ShepColors.subtext, fontSize = 12.sp)
            Spacer(Modifier.height(12.dp))
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                placeholder = { Text("what needs to change…", color = ShepColors.subtext) },
                modifier = Modifier.fillMaxWidth(),
                maxLines = 6,
            )
            Spacer(Modifier.height(16.dp))
            Button(onClick = { onSubmit(text.trim()) }, enabled = text.isNotBlank(), modifier = Modifier.fillMaxWidth()) {
                Text("send to agent")
            }
            Spacer(Modifier.height(8.dp))
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
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("board", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            Text(status, color = ShepColors.subtext, fontSize = 12.sp)
            Spacer(Modifier.width(12.dp))
            Text(
                "+ new",
                color = ShepColors.copper,
                fontSize = 14.sp,
                fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clickable { showNew = true },
            )
            Spacer(Modifier.width(12.dp))
            Text(
                "unpair",
                color = ShepColors.subtext,
                fontSize = 12.sp,
                modifier = Modifier.clickable { onUnpair() },
            )
        }
        val serverProtocol = client.serverProtocol
        if (serverProtocol != null && serverProtocol != EXPECTED_PROTOCOL) {
            Text(
                "⚠ server protocol $serverProtocol · app built for $EXPECTED_PROTOCOL — " +
                    if (serverProtocol > EXPECTED_PROTOCOL) "update the app" else "update the server",
                color = ShepColors.bg,
                fontSize = 12.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(ShepColors.peach)
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            )
        }
        notice?.let {
            Text(
                it,
                color = ShepColors.peach,
                fontSize = 12.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { notice = null }
                    .padding(horizontal = 16.dp, vertical = 4.dp),
            )
        }
        dev.shep.companion.screens.DashboardStrip(totals, host) { statusColor(it) }
        FilterChips(filter, rows) { filter = it }
        if (visibleRows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    if (rows.isEmpty()) "no sessions — start one with + new"
                    else "nothing ${filter.label}",
                    color = ShepColors.subtext,
                )
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
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
            .background(ShepColors.surface)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        HomeFilter.entries.forEach { entry ->
            val count = rows.count { entry.accepts(it.status) }
            val active = entry == selected
            Box(
                Modifier
                    .clip(RoundedCornerShape(16.dp))
                    .background(if (active) ShepColors.copper else ShepColors.surfaceHigh)
                    .clickable { onSelect(entry) }
                    .padding(horizontal = 12.dp, vertical = 6.dp),
            ) {
                Text(
                    "${entry.label} $count",
                    color = if (active) ShepColors.bg else ShepColors.subtext,
                    fontSize = 12.sp,
                    fontWeight = if (active) FontWeight.SemiBold else FontWeight.Normal,
                )
            }
        }
    }
}

@Composable
fun AgentCard(row: AgentRow, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(ShepColors.surface)
            .clickable { onClick() }
            .padding(16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(12.dp).clip(CircleShape).background(statusColor(row.status))
        )
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(row.agent, color = ShepColors.text, fontWeight = FontWeight.SemiBold)
                row.contextPercent?.let {
                    Spacer(Modifier.width(8.dp))
                    Text("$it%", color = ShepColors.subtext, fontSize = 12.sp)
                }
                row.memoryPercent?.takeIf { it >= 80 }?.let {
                    Spacer(Modifier.width(6.dp))
                    Text("mem $it%", color = ShepColors.peach, fontSize = 11.sp)
                }
                when (row.reviewState) {
                    "needs_review" -> ReviewBadge("◆", ShepColors.peach)
                    "changes_requested" -> ReviewBadge("↺", ShepColors.peach)
                    "approved" -> ReviewBadge("✓", ShepColors.green)
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (row.isWorktree) {
                    Text("⑂ ", color = ShepColors.copper, fontSize = 12.sp)
                }
                Text(
                    row.worktreeRepo?.let { "$it · ${row.workspaceLabel}" } ?: row.workspaceLabel,
                    color = ShepColors.subtext,
                    fontSize = 13.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Column(horizontalAlignment = Alignment.End) {
            Text(row.status, color = statusColor(row.status), fontSize = 13.sp)
            row.customStatus?.let {
                Text(
                    it,
                    color = ShepColors.subtext,
                    fontSize = 11.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
fun ReviewBadge(glyph: String, color: Color) {
    Spacer(Modifier.width(8.dp))
    Text(glyph, color = color, fontSize = 13.sp)
}
