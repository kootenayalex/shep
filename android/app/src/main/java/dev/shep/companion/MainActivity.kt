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
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.core.app.NotificationManagerCompat
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.shep.companion.screens.ChannelsScreen
import dev.shep.companion.screens.MemoryScreen
import dev.shep.companion.screens.PairingScreen
import dev.shep.companion.screens.ServerScreen
import dev.shep.companion.screens.TasksScreen
import dev.shep.companion.screens.pane.PaneScreen
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.HintBar
import dev.shep.companion.ui.components.LoadingState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.NoticeTone
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
import org.json.JSONObject
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import dev.shep.companion.ui.theme.ShepMotion

// Bridge protocol the app was built against (shep src/protocol/wire.rs
// PROTOCOL_VERSION at vendor time). A mismatch soft-warns; it does not brick a
// personal sideload. Bump alongside the vendored schema (1f follow-up).
const val EXPECTED_PROTOCOL = 16

/** An agent state's colour, for the places that tint a label rather than draw a glyph. */
fun statusColor(status: String): Color = ShepSemantic.agentColor(status)

/**
 * What to say on the offline banner.
 *
 * A transport error is the wrong thing to put in front of someone whose only
 * available action is to wait — `timed out connecting to ws://100.83.179.75:7431/`
 * reads as a fault when the app is already handling it. The one error worth
 * naming is the one that will never heal on its own: a rejected token needs
 * hands on a keyboard.
 */
fun offlineNotice(error: String?): String =
    if (error != null && error.contains("unauthorized", ignoreCase = true)) {
        "pairing rejected — re-pair from `shep bridge pair`"
    } else {
        "offline · reconnecting…"
    }

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

/**
 * Hint-bar destinations. Shortcuts mirror the TUI vocabulary without adding
 * an icon dependency or making the phone carry a second navigation model.
 *
 * Four is the Hick's-law ceiling used by the desktop and the prototype.
 */
enum class Tab(val label: String, val shortcut: String) {
    Agents("agents", "a"),
    Tasks("tasks", "t"),
    Memory("memory", "m"),
    Shep("shep", "s"),
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
    val context = LocalContext.current
    var firstConnect by remember { mutableStateOf(PairingStore.isPaired(context)) }
    // Whether the socket we are holding is believed good. A drop no longer
    // unmounts the shell, so this drives a banner rather than a whole screen.
    var online by remember { mutableStateOf(false) }
    // Bumped by a dropped socket to kick the reconnect loop below.
    var reconnectSignal by remember { mutableStateOf(0) }
    // A live socket is only worth holding while somebody is looking at it.
    var foreground by remember { mutableStateOf(true) }
    val scope = rememberCoroutineScope()

    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> foreground = true
                Lifecycle.Event.ON_STOP -> foreground = false
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }

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
        }
    }

    // Establish a fresh connection from the saved pairing. Used for the first
    // auto-connect and for every reconnect; wires onDisconnect so a dropped
    // tailnet socket self-heals instead of stranding the screen.
    suspend fun establish(): String? {
        val saved = PairingStore.load(context) ?: return "no saved pairing"
        val fresh = BridgeClient(saved.url, saved.token)
        val error = withContext(Dispatchers.IO) {
            runCatching { fresh.connect() }.getOrElse { it.message ?: "connection failed" }
        }
        if (error != null) return error
        fresh.onDisconnect = { reason ->
            online = false
            connectError = reason ?: "disconnected"
            reconnectSignal += 1
        }
        // Retire the old socket without letting its own close callback run: it
        // would report a disconnect we caused deliberately, and send the loop
        // below round again immediately after it had just succeeded.
        client?.let {
            it.onDisconnect = null
            it.close()
        }
        client = fresh
        online = true
        return null
    }

    fun pairAndConnect(url: String, token: String, onDone: (String?) -> Unit) {
        scope.launch {
            PairingStore.save(context, url, token)
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
        if (PairingStore.isPaired(context)) {
            val error = establish()
            if (error == null) paired = true else connectError = error
            firstConnect = false
        }
    }

    // One place decides whether a socket should exist, and dials until it does.
    //
    // Leaving the app used to strand this. The socket died while the phone was
    // away, the retry loop kept dialling a sleeping radio with a backoff that
    // grew to 15s, and coming back showed you the stale timeout from an attempt
    // made while the screen was off — every single time. Retrying is now a
    // foreground activity, and returning to the app *checks* the socket rather
    // than trusting it.
    LaunchedEffect(paired, foreground, reconnectSignal) {
        if (!paired || !foreground) return@LaunchedEffect

        // An open-looking socket can be dead: a connection broken while the
        // phone dozed stays "open" to OkHttp until a ping eventually times out.
        // Asking the server is the only answer worth acting on, and when it
        // answers — the common case for a short absence — there is nothing to
        // reconnect and no banner to show.
        val held = client
        if (held != null && held.isOpen) {
            val alive = withContext(Dispatchers.IO) {
                runCatching { held.call("ping", timeoutSeconds = 3) }.isSuccess
            }
            if (alive) {
                online = true
                connectError = null
                return@LaunchedEffect
            }
        }

        online = false
        // Starts short: the first dial after you return is the one that decides
        // whether the app feels alive, and the long waits only earn their keep
        // once several attempts have already failed.
        var backoff = 400L
        while (isActive) {
            val error = establish()
            if (error == null) {
                connectError = null
                break
            }
            connectError = error
            delay(backoff)
            backoff = (backoff * 2).coerceAtMost(10000L)
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
                val saved = PairingStore.load(context)
                PairingScreen(
                    initialUrl = saved?.url ?: "",
                    initialToken = saved?.token ?: "",
                    lastError = connectError,
                    onConnect = { url, token, onDone -> pairAndConnect(url, token, onDone) },
                )
            }
        } else {
            val active = client
            if (active == null) {
                LoadingState("reconnecting…", detail = connectError)
            } else {
                // The shell stays mounted through a drop. Screens keep the last
                // thing they read, which is stale but true-as-of-a-moment-ago;
                // unmounting threw you out of whatever pane you were reading
                // and back to the board, which is a worse answer to a blip.
                Column(Modifier.fillMaxSize()) {
                    if (!online) {
                        Notice(offlineNotice(connectError), tone = NoticeTone.Alert)
                    }
                    NavShell(
                        client = active,
                        deepLinkPane = deepLinkPane,
                        onDeepLinkConsumed = onDeepLinkConsumed,
                        newTask = newTask,
                        onNewTaskConsumed = onNewTaskConsumed,
                        onUnpair = {
                            paired = false
                            online = false
                            PairingStore.clear(context)
                            active.onDisconnect = null
                            active.close()
                            client = null
                        },
                    )
                }
            }
        }
    }
}

/**
 * The paired experience: a hint-bar Scaffold over the four destinations, with
 * the pane view pushed as a full-screen detail over the Chats tab on phones, or
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
    // Hoisted from TasksScreen so the new-task deep-link can pre-open the sheet.
    var tasksShowAdd by remember { mutableStateOf(false) }
    // Hoisted out of the list for a different reason: opening a pane replaces
    // the whole scaffold on a phone, so a group collapsed in the list itself is
    // re-expanded the moment you look at an agent and come back.
    var collapsedSpaces by remember { mutableStateOf<Set<String>>(emptySet()) }
    val context = LocalContext.current

    // Opening an agent answers its notification. The local one comes down at
    // once, and shep is told the pane was seen so it withdraws the notification
    // on every other device too — and the desk's own unseen mark clears. This is
    // the single place that happens, whether the pane came from the list, the
    // tablet dock, or a notification tap.
    LaunchedEffect(paneDetail?.paneId) {
        val opened = paneDetail ?: return@LaunchedEffect
        runCatching {
            NotificationManagerCompat.from(context).cancel(notificationIdFor(opened.paneId))
        }
        withContext(Dispatchers.IO) {
            runCatching {
                client.call("pane.mark_seen", JSONObject().put("pane_id", opened.paneId))
            }
        }
    }

    // A notification tap (shep://pane?pane=…) resolves the pane id to its row via
    // a one-shot snapshot and pushes the pane detail; falls back to the Chats
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
                HintBar(tab, onSelect = { tab = it })
            },
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                if (wide && tab == Tab.Agents) {
                    Row(Modifier.fillMaxSize()) {
                        Box(Modifier.weight(1f)) {
                            ChannelsScreen(
                                client = client,
                                onOpenPane = { paneDetail = it },
                                onUnpair = onUnpair,
                                collapsed = collapsedSpaces,
                                onCollapsedChange = { collapsedSpaces = it },
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
                    // A crossfade, not a slide: these are peers, not a stack,
                    // and a directional transition would claim an order the nav
                    // bar does not have. Short enough that switching tabs still
                    // feels like switching, not like waiting.
                    Crossfade(
                        targetState = tab,
                        animationSpec = tween(ShepMotion.ENTER_MS),
                        label = "tab",
                    ) { current ->
                        when (current) {
                            Tab.Agents -> ChannelsScreen(
                                client = client,
                                onOpenPane = { paneDetail = it },
                                onUnpair = onUnpair,
                                collapsed = collapsedSpaces,
                                onCollapsedChange = { collapsedSpaces = it },
                            )
                            Tab.Tasks -> TasksScreen(
                                client = client,
                                showAdd = tasksShowAdd,
                                onShowAddChange = { tasksShowAdd = it },
                            )
                            Tab.Memory -> MemoryScreen(client)
                            Tab.Shep -> ServerScreen(client = client, onRePair = onUnpair)
                        }
                    }
                }
            }
        }
    }
}
