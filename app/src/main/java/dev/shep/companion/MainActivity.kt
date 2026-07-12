package dev.shep.companion

import android.content.Context
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
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
                ShepApp(getSharedPreferences("shep", Context.MODE_PRIVATE))
            }
        }
    }
}

sealed class Screen {
    data object Pairing : Screen()
    data object Home : Screen()
    data class Pane(val row: AgentRow) : Screen()
}

@Composable
fun ShepApp(prefs: android.content.SharedPreferences) {
    var client by remember { mutableStateOf<BridgeClient?>(null) }
    var screen by remember { mutableStateOf<Screen>(Screen.Pairing) }
    var connectError by remember { mutableStateOf<String?>(null) }
    val scope = rememberCoroutineScope()

    fun connect(url: String, token: String, onDone: (String?) -> Unit) {
        scope.launch {
            val fresh = BridgeClient(url, token)
            val error = withContext(Dispatchers.IO) {
                runCatching { fresh.connect() }.getOrElse { it.message ?: "connection failed" }
            }
            if (error == null) {
                prefs.edit().putString("url", url).putString("token", token).apply()
                client?.close()
                client = fresh
                screen = Screen.Home
            }
            onDone(error)
        }
    }

    // Auto-reconnect with saved pairing on launch.
    LaunchedEffect(Unit) {
        val url = prefs.getString("url", null)
        val token = prefs.getString("token", null)
        if (url != null && token != null) {
            connect(url, token) { error -> connectError = error }
        }
    }

    Surface(Modifier.fillMaxSize().statusBarsPadding(), color = ShepColors.bg) {
        when (val current = screen) {
            is Screen.Pairing -> PairingScreen(
                initialUrl = prefs.getString("url", "") ?: "",
                initialToken = prefs.getString("token", "") ?: "",
                lastError = connectError,
                onConnect = { url, token, onDone -> connect(url, token, onDone) },
            )
            is Screen.Home -> {
                val active = client
                if (active == null) {
                    screen = Screen.Pairing
                } else {
                    HomeScreen(
                        client = active,
                        onOpenPane = { screen = Screen.Pane(it) },
                        onUnpair = {
                            active.close()
                            client = null
                            screen = Screen.Pairing
                        },
                    )
                }
            }
            is Screen.Pane -> {
                val active = client
                if (active == null) {
                    screen = Screen.Pairing
                } else {
                    BackHandler { screen = Screen.Home }
                    PaneScreen(active, current.row, onBack = { screen = Screen.Home })
                }
            }
        }
    }
}

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

    Column(
        Modifier.fillMaxSize().padding(24.dp).verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("shep", color = ShepColors.copper, fontSize = 40.sp, fontWeight = FontWeight.Bold)
        Text(
            "the cockpit in your pocket",
            color = ShepColors.subtext,
            modifier = Modifier.padding(bottom = 32.dp),
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

@Composable
fun HomeScreen(client: BridgeClient, onOpenPane: (AgentRow) -> Unit, onUnpair: () -> Unit) {
    var rows by remember { mutableStateOf<List<AgentRow>>(emptyList()) }
    var status by remember { mutableStateOf("connecting") }

    LaunchedEffect(client) {
        while (isActive) {
            val result = withContext(Dispatchers.IO) {
                runCatching { client.call("session.snapshot") }
            }
            result.onSuccess {
                rows = parseSnapshot(it)
                status = "live · shep ${client.serverVersion ?: ""}".trim()
            }.onFailure {
                status = "reconnect: ${it.message}"
            }
            delay(2000)
        }
    }

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
        if (rows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("no agents running", color = ShepColors.subtext)
            }
        } else {
            LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(rows, key = { it.paneId }) { row -> AgentCard(row) { onOpenPane(row) } }
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
                when (row.reviewState) {
                    "needs_review" -> ReviewBadge("◆", ShepColors.peach)
                    "changes_requested" -> ReviewBadge("↺", ShepColors.peach)
                    "approved" -> ReviewBadge("✓", ShepColors.green)
                }
            }
            Text(
                row.workspaceLabel,
                color = ShepColors.subtext,
                fontSize = 13.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        Text(row.status, color = statusColor(row.status), fontSize = 13.sp)
    }
}

@Composable
fun ReviewBadge(glyph: String, color: Color) {
    Spacer(Modifier.width(8.dp))
    Text(glyph, color = color, fontSize = 13.sp)
}

@Composable
fun PaneScreen(client: BridgeClient, row: AgentRow, onBack: () -> Unit) {
    var paneText by remember { mutableStateOf("") }
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
                            .put("format", "text"),
                    )
                }.getOrNull()
            }
            if (read != null) {
                paneText = read.optJSONObject("read")?.optString("text") ?: read.optString("text")
                val snapshot = withContext(Dispatchers.IO) {
                    runCatching { client.call("session.snapshot") }.getOrNull()
                }
                snapshot?.let { snap ->
                    parseSnapshot(snap).find { it.paneId == row.paneId }
                        ?.let { liveStatus = it.status }
                }
                scroll.scrollTo(scroll.maxValue)
            }
            delay(1500)
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
            paneText.ifEmpty { "reading pane..." },
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
