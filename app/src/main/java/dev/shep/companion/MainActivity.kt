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
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import dev.shep.companion.screens.BoardScreen
import dev.shep.companion.screens.MemoryScreen
import dev.shep.companion.screens.PairingScreen
import dev.shep.companion.screens.ServerScreen
import dev.shep.companion.screens.SpacesScreen
import dev.shep.companion.screens.TasksScreen
import dev.shep.companion.screens.pane.PaneScreen
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.LoadingState
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepTheme
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import dev.shep.companion.ui.theme.ShepMotion

// Bridge protocol the app was built against (shep src/protocol/wire.rs
// PROTOCOL_VERSION at vendor time). A mismatch soft-warns; it does not brick a
// personal sideload. Bump alongside the vendored schema (1f follow-up).
const val EXPECTED_PROTOCOL = 16

/** An agent state's colour, for the places that tint a label rather than draw a glyph. */
fun statusColor(status: String): Color = ShepSemantic.agentColor(status)

class MainActivity : ComponentActivity() {
    // Pane id from a `shep://pane?pane=…` notification tap; consumed by NavShell.
    private val deepLinkPane = mutableStateOf<String?>(null)
    // A6: `shep://tasks/new` (launcher shortcut / widget) → Tasks tab, add sheet.
    private val deepLinkNewTask = mutableStateOf(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        // Before super: this is what swaps Theme.Shep (the splash) for
        // Theme.Shep.Main. Without the call the activity keeps the splash
        // theme for its whole life, and the splash theme's window background
        // is the platform default — which is white, and shows through
        // everywhere Compose does not paint: behind the status bar, and in
        // the recents card.
        installSplashScreen()
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
        return data.scheme == "shep" && data.host == "tasks" &&
            data.pathSegments.firstOrNull() == "new"
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
    val context = LocalContext.current

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
                LoadingState("connecting…")
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
                LoadingState("reconnecting…", detail = connectError)
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

/**
 * The paired experience: a bottom-nav Scaffold over the five destinations, with
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
    // A tree node waiting to be resolved into the row the pane view needs.
    var openPaneNode by remember { mutableStateOf<Pair<PaneNode, String>?>(null) }
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
    //
    // The snapshot only answers with *agents*, so a plain shell — which the
    // tree happily lists and ripples — resolved to nothing and the tap did
    // nothing at all. Falling back to the node means every row in the tree
    // opens something, which is the only honest reading of a tappable row.
    LaunchedEffect(openPaneNode) {
        val (node, spaceLabel) = openPaneNode ?: return@LaunchedEffect
        val row = withContext(Dispatchers.IO) {
            runCatching { parseSnapshot(client.call("session.snapshot")) }.getOrNull()
        }?.find { it.paneId == node.paneId }
        paneDetail = row ?: node.asAgentRow(spaceLabel)
        openPaneNode = null
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
        // memory — no full-screen push). Phone: the detail pushes over.
        //
        // Measured against the *smaller* dimension, not the width: a phone in
        // landscape is around 800dp wide and was tripping the tablet branch,
        // docking a cramped board beside a pane on a five-inch screen.
        val wide = minOf(maxWidth, maxHeight) >= ShepSize.twoPaneWidth
        val detail = paneDetail

        if (detail != null && !wide) {
            BackHandler { paneDetail = null }
            PaneScreen(client, detail, onBack = { paneDetail = null })
            return@BoxWithConstraints
        }

        Scaffold(
            containerColor = ShepPalette.panelBg,
            bottomBar = {
                // No navigationBarsPadding here: the Surface above already
                // applies it once, and applying it twice inset the bar by the
                // gesture pill's height on top of itself.
                NavigationBar(containerColor = ShepPalette.surfaceDim) {
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
                            BoardScreen(
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
                                EmptyState("select an agent")
                            }
                        }
                    }
                } else {
                    // A crossfade, not a slide: these are five peers, not a
                    // stack, and a directional transition would claim an order
                    // the nav bar does not have. Short enough that switching
                    // tabs still feels like switching, not like waiting.
                    Crossfade(
                        targetState = tab,
                        animationSpec = tween(ShepMotion.ENTER_MS),
                        label = "tab",
                    ) { current ->
                        when (current) {
                            Tab.Agents -> BoardScreen(
                                client = client,
                                onOpenPane = { paneDetail = it },
                                onUnpair = onUnpair,
                            )
                            Tab.Spaces -> SpacesScreen(
                                client = client,
                                // The tree knows a pane; the pane view wants
                                // the board's row for it, so resolve through
                                // the same path a notification tap uses rather
                                // than inventing a second, thinner pane view.
                                onOpenPane = { node, label -> openPaneNode = node to label },
                            )
                            Tab.Tasks -> TasksScreen(
                                client = client,
                                showAdd = tasksShowAdd,
                                onShowAddChange = { tasksShowAdd = it },
                            )
                            Tab.Memory -> MemoryScreen(client)
                            Tab.Shep -> ServerScreen()
                        }
                    }
                }
            }
        }
    }
}
