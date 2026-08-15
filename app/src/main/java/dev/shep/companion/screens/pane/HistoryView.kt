package dev.shep.companion.screens.pane

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import dev.shep.companion.AgentRow
import dev.shep.companion.BridgeClient
import dev.shep.companion.ansiToAnnotated
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Scrollback for a pane.
 *
 * The live grid is observe-only and cannot scroll — the server drops input from
 * observers, and the one bincode mode that *can* scroll would resize the user's
 * real terminal. So history is a separate read: `agent.read source=recent` over
 * the JSON API, which touches nothing.
 */
@Composable
fun HistoryView(
    client: BridgeClient,
    row: AgentRow,
    onBack: () -> Unit,
    lines: Int = 500,
) {
    var text by remember { mutableStateOf(AnnotatedString("")) }
    var error by remember { mutableStateOf<String?>(null) }
    val scroll = rememberScrollState()

    LaunchedEffect(row.paneId) {
        val result = withContext(Dispatchers.IO) {
            runCatching {
                client.call(
                    "agent.read",
                    JSONObject()
                        .put("target", row.paneId)
                        .put("source", "recent")
                        .put("lines", lines)
                        .put("format", "ansi"),
                )
            }
        }
        result.onSuccess { read ->
            val raw = read.optJSONObject("read")?.optString("text") ?: read.optString("text")
            text = ansiToAnnotated(raw, ShepPalette.text)
            scroll.scrollTo(scroll.maxValue)
        }.onFailure { error = it.message }
    }

    Column(Modifier.fillMaxSize().background(ShepPalette.panelBg)) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(ShepPalette.surfaceDim)
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.medium),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            Text(
                "‹",
                style = ShepType.wordmark.copy(color = ShepPalette.accent),
                modifier = Modifier.clickable { onBack() }.padding(end = ShepSpace.tight),
            )
            Text("history", style = ShepType.agentName.copy(color = ShepPalette.accent))
            Text("${row.paneId} · last $lines lines", style = ShepType.paneId)
        }
        error?.let {
            Text(
                it,
                style = ShepType.hint.copy(color = ShepPalette.red),
                modifier = Modifier.padding(ShepSpace.medium),
            )
        }
        Text(
            text,
            style = ShepType.codeSmall.copy(color = ShepPalette.text),
            modifier = Modifier
                .testTag("history-text")
                .fillMaxSize()
                .verticalScroll(scroll)
                .padding(ShepSpace.small),
        )
    }
}
