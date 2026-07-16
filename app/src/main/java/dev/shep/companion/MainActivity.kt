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

// Shep theme: warm graphite + copper, same vocabulary as the TUI.
object ShepColors {
    val bg = Color(0xFF141210)
    val surface = Color(0xFF1E1B18)
    val surfaceHigh = Color(0xFF282420)
    val text = Color(0xFFEDE7DF)
    val subtext = Color(0xFF9C948A)
    val copper = Color(0xFFE09A55)   // accent / working
    val green = Color(0xFF9BC177)    // idle / approved
    val red = Color(0xFFD9695F)      // blocked
    val blue = Color(0xFF7FA8C9)     // done (unseen)
    val peach = Color(0xFFE0B085)    // warning tier
}

fun statusColor(status: String): Color = when (status) {
    "blocked" -> ShepColors.red
    "working" -> ShepColors.copper
    "done" -> ShepColors.blue
    "idle" -> ShepColors.green
    else -> ShepColors.subtext
}

class MainActivity : ComponentActivity() {
    // Pane id from a `shep://pane?pane=…` notification tap; consumed by NavShell.
    private val deepLinkPane = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        deepLinkPane.value = paneFromIntent(intent)
        setContent {
            MaterialTheme(
                colorScheme = darkColorScheme(
                    primary = ShepColors.copper,
                    background = ShepColors.bg,
                    surface = ShepColors.surface,
                    surfaceVariant = ShepColors.surfaceHigh,
                    onPrimary = ShepColors.bg,
                    onBackground = ShepColors.text,
                    onSurface = ShepColors.text,
                )
            ) {
                ShepApp(
                    getSharedPreferences("shep", Context.MODE_PRIVATE),
                    deepLinkPane = deepLinkPane.value,
                    onDeepLinkConsumed = { deepLinkPane.value = null },
                )
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        paneFromIntent(intent)?.let { deepLinkPane.value = it }
    }

    private fun paneFromIntent(intent: Intent?): String? {
        val data = intent?.data ?: return null
        if (data.scheme != "shep" || data.host != "pane") return null
        return data.getQueryParameter("pane")?.takeIf { it.isNotBlank() }
    }
}

/** Bottom-nav destinations. Glyphs mirror the TUI vocabulary (no icon dep). */
enum class Tab(val label: String, val glyph: String) {
    Agents("agents", "◫"),
    Tasks("tasks", "☰"),
    Memory("memory", "✦"),
    Shep("shep", "⚙"),
}

@Composable
fun ShepApp(
    prefs: android.content.SharedPreferences,
    deepLinkPane: String? = null,
    onDeepLinkConsumed: () -> Unit = {},
) {
    var client by remember { mutableStateOf<BridgeClient?>(null) }
    var paired by remember { mutableStateOf(false) }
    var connectError by remember { mutableStateOf<String?>(null) }
    // Bumped by a dropped socket to kick the reconnect loop below.
    var reconnectSignal by remember { mutableStateOf(0) }
    val scope = rememberCoroutineScope()
    val context = androidx.compose.ui.platform.LocalContext.current

    // Ask for POST_NOTIFICATIONS (Android 13+) so A3 pages can show.
    val notifPermission = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* granted or not — push still registers; the OS just suppresses posts */ }

    // Once paired, register for UnifiedPush and (13+) request the notif permission.
    // Runs whenever pairing flips true; UnifiedPush.registerApp is idempotent.
    LaunchedEffect(paired) {
        if (paired) {
            if (Build.VERSION.SDK_INT >= 33) {
                notifPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
            }
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

    Surface(Modifier.fillMaxSize().statusBarsPadding(), color = ShepColors.bg) {
        if (!paired) {
            PairingScreen(
                initialUrl = prefs.getString("url", "") ?: "",
                initialToken = prefs.getString("token", "") ?: "",
                lastError = connectError,
                onConnect = { url, token, onDone -> pairAndConnect(url, token, onDone) },
            )
        } else {
            val active = client
            if (active == null) {
                ReconnectingScreen(connectError)
            } else {
                NavShell(
                    client = active,
                    deepLinkPane = deepLinkPane,
                    onDeepLinkConsumed = onDeepLinkConsumed,
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
fun ReconnectingScreen(error: String?) {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            CircularProgressIndicator(color = ShepColors.copper)
            Spacer(Modifier.height(16.dp))
            Text("reconnecting…", color = ShepColors.subtext)
            error?.let {
                Spacer(Modifier.height(6.dp))
                Text(it, color = ShepColors.peach, fontSize = 12.sp)
            }
        }
    }
}

/**
 * The paired experience: a bottom-nav Scaffold over the four destinations, with
 * the pane view pushed as a full-screen detail over the Agents tab. A3 deep-links
 * route here by setting the tab + selecting a pane.
 */
@Composable
fun NavShell(
    client: BridgeClient,
    onUnpair: () -> Unit,
    deepLinkPane: String? = null,
    onDeepLinkConsumed: () -> Unit = {},
) {
    var tab by remember { mutableStateOf(Tab.Agents) }
    var paneDetail by remember { mutableStateOf<AgentRow?>(null) }

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

    val detail = paneDetail
    if (detail != null) {
        BackHandler { paneDetail = null }
        PaneScreen(client, detail, onBack = { paneDetail = null })
        return
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
            when (tab) {
                Tab.Agents -> HomeScreen(
                    client = client,
                    onOpenPane = { paneDetail = it },
                    onUnpair = onUnpair,
                )
                Tab.Tasks -> ComingSoon("tasks", "dispatch & track worktree tasks — A4")
                Tab.Memory -> ComingSoon("memory", "USER + repo memory, search & cap — A4")
                Tab.Shep -> ShepScreen()
            }
        }
    }
}

/** Settings tab: A3 push status + re-register, over the future review/ship home. */
@Composable
fun ShepScreen() {
    val context = androidx.compose.ui.platform.LocalContext.current
    val prefs = remember { context.getSharedPreferences("shep", Context.MODE_PRIVATE) }
    var status by remember { mutableStateOf(prefs.getString("push_status", "not registered") ?: "") }
    var endpoint by remember { mutableStateOf(prefs.getString("push_endpoint", null)) }
    val scope = rememberCoroutineScope()

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("shep", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
        }
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text("push notifications", color = ShepColors.text, fontWeight = FontWeight.SemiBold)
            Spacer(Modifier.height(6.dp))
            Text(status, color = ShepColors.subtext, fontSize = 13.sp)
            endpoint?.let {
                Spacer(Modifier.height(4.dp))
                Text(it, color = ShepColors.subtext, fontSize = 11.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            Spacer(Modifier.height(12.dp))
            Button(onClick = {
                scope.launch {
                    // Show the immediate outcome; the endpoint (if any) arrives
                    // asynchronously via PushReceiver.onNewEndpoint into prefs.
                    status = withContext(Dispatchers.IO) { PushManager.register(context) }
                    endpoint = prefs.getString("push_endpoint", null)
                }
            }) { Text("Re-register push") }
            Spacer(Modifier.height(24.dp))
            Text("review, ship & settings — A5", color = ShepColors.subtext, fontSize = 12.sp)
        }
    }
}

/** Placeholder for a not-yet-built tab, so A4/A5 slot in without re-architecting. */
@Composable
fun ComingSoon(title: String, blurb: String) {
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(title, color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
        }
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text("coming soon", color = ShepColors.subtext)
                Spacer(Modifier.height(6.dp))
                Text(blurb, color = ShepColors.subtext, fontSize = 12.sp)
            }
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

@Composable
fun HomeScreen(client: BridgeClient, onOpenPane: (AgentRow) -> Unit, onUnpair: () -> Unit) {
    var rows by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var status by remember { mutableStateOf("connecting") }
    var filter by remember { mutableStateOf(HomeFilter.Attention) }

    // Event-driven refresh: subscribe to structural events + a per-pane
    // agent_status_changed sub for each live pane, and re-snapshot on any event
    // (bridge relays each as a channel line). Keepalive poll is only a backstop.
    LaunchedEffect(client) {
        val refreshSignal = kotlinx.coroutines.channels.Channel<Unit>(
            kotlinx.coroutines.channels.Channel.CONFLATED
        )
        var subscribedPanes: Set<String> = emptySet()
        var subChannel = -1L

        suspend fun refresh() {
            val result = withContext(Dispatchers.IO) {
                runCatching { client.call("session.snapshot") }
            }
            result.onSuccess {
                rows = parseSnapshot(it)
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

    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("agents", color = ShepColors.text, fontSize = 22.sp, fontWeight = FontWeight.Bold)
            Spacer(Modifier.weight(1f))
            Text(status, color = ShepColors.subtext, fontSize = 12.sp)
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
        FilterChips(filter, rows) { filter = it }
        if (visibleRows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(
                    if (rows.isEmpty()) "no agents running" else "nothing ${filter.label}",
                    color = ShepColors.subtext,
                )
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(visibleRows, key = { it.paneId }) { row -> AgentCard(row) { onOpenPane(row) } }
            }
        }
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

@Composable
fun PaneScreen(client: BridgeClient, row: AgentRow, onBack: () -> Unit) {
    var paneText by remember { mutableStateOf(androidx.compose.ui.text.AnnotatedString("")) }
    var liveStatus by remember { mutableStateOf(row.status) }
    var composer by remember { mutableStateOf("") }
    var notice by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()
    val scroll = rememberScrollState()

    fun sendKeys(vararg keys: String) {
        scope.launch(Dispatchers.IO) {
            runCatching {
                client.call(
                    "pane.send_keys",
                    JSONObject().put("pane_id", row.paneId).put("keys", JSONArray(keys.toList())),
                )
            }.onFailure { notice = it.message }
        }
    }

    fun sendPrompt(queue: Boolean) {
        val text = composer.trim()
        if (text.isEmpty()) return
        scope.launch(Dispatchers.IO) {
            runCatching {
                client.call(
                    "agent.send",
                    JSONObject().put("target", row.paneId).put("text", text).put("queue", queue),
                )
            }.onSuccess {
                composer = ""
                notice = if (queue) "queued — fires on idle" else "sent"
            }.onFailure { notice = it.message }
        }
    }

    LaunchedEffect(row.paneId) {
        while (isActive) {
            val read = withContext(Dispatchers.IO) {
                runCatching {
                    client.call(
                        "agent.read",
                        JSONObject()
                            .put("target", row.paneId)
                            .put("source", "visible")
                            .put("format", "ansi"),
                    )
                }.getOrNull()
            }
            if (read != null) {
                val raw = read.optJSONObject("read")?.optString("text") ?: read.optString("text")
                paneText = ansiToAnnotated(raw, ShepColors.text)
                val snapshot = withContext(Dispatchers.IO) {
                    runCatching { client.call("session.snapshot") }.getOrNull()
                }
                snapshot?.let { snap ->
                    parseSnapshot(snap).find { it.paneId == row.paneId }
                        ?.let { liveStatus = it.status }
                }
                scroll.scrollTo(scroll.maxValue)
            }
            delay(1200)
        }
    }

    Column(Modifier.fillMaxSize().imePadding()) {
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("←", color = ShepColors.copper, fontSize = 22.sp, modifier = Modifier.clickable { onBack() }.padding(end = 14.dp))
            Box(Modifier.size(10.dp).clip(CircleShape).background(statusColor(liveStatus)))
            Spacer(Modifier.width(8.dp))
            Column {
                Text("${row.agent} · $liveStatus", color = ShepColors.text, fontWeight = FontWeight.SemiBold)
                Text(row.workspaceLabel, color = ShepColors.subtext, fontSize = 12.sp)
            }
        }
        Text(
            if (paneText.text.isEmpty()) androidx.compose.ui.text.AnnotatedString("reading pane...") else paneText,
            color = ShepColors.text,
            fontFamily = FontFamily.Monospace,
            fontSize = 11.sp,
            lineHeight = 15.sp,
            modifier = Modifier
                .weight(1f)
                .fillMaxWidth()
                .background(ShepColors.bg)
                .verticalScroll(scroll)
                .padding(10.dp),
        )
        notice?.let {
            Text(
                it,
                color = ShepColors.peach,
                fontSize = 12.sp,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 4.dp),
            )
        }
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(horizontal = 8.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            QuickKey("y") { sendKeys("y") }
            QuickKey("n") { sendKeys("n") }
            QuickKey("enter") { sendKeys("enter") }
            QuickKey("esc") { sendKeys("esc") }
            QuickKey("↑") { sendKeys("up") }
            QuickKey("↓") { sendKeys("down") }
        }
        Row(
            Modifier.fillMaxWidth().background(ShepColors.surface).padding(8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = composer,
                onValueChange = { composer = it },
                placeholder = { Text("prompt...", color = ShepColors.subtext) },
                modifier = Modifier.weight(1f),
                maxLines = 3,
            )
            Spacer(Modifier.width(8.dp))
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Button(onClick = { sendPrompt(false) }, contentPadding = PaddingValues(horizontal = 14.dp, vertical = 4.dp)) {
                    Text("send")
                }
                Text(
                    "queue ⇥",
                    color = ShepColors.copper,
                    fontSize = 13.sp,
                    modifier = Modifier.clickable { sendPrompt(true) }.padding(4.dp),
                )
            }
        }
    }
}

@Composable
fun RowScope.QuickKey(label: String, onClick: () -> Unit) {
    Box(
        Modifier
            .weight(1f)
            .height(40.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(ShepColors.surfaceHigh)
            .clickable { onClick() },
        contentAlignment = Alignment.Center,
    ) {
        Text(label, color = ShepColors.text, fontSize = 14.sp)
    }
}
