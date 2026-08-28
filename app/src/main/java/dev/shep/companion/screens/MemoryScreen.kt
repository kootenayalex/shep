package dev.shep.companion.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.shep.companion.BridgeClient
import dev.shep.companion.MemoryView
import dev.shep.companion.parseMemory
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.LoadingState
import dev.shep.companion.ui.components.Meter
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/**
 * Memory tab (A4): the USER profile's entries with a cap meter and add / edit /
 * remove. Backed by the bridge-local `memory.show/add/replace/remove`.
 */
@Composable
fun MemoryScreen(client: BridgeClient) {
    var view by remember { mutableStateOf<MemoryView?>(null) }
    var status by remember { mutableStateOf("loading") }
    var notice by remember { mutableStateOf<String?>(null) }
    // Existing entry text, or "" for a new entry.
    var editing by remember { mutableStateOf<String?>(null) }
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
        ScreenHeader("memory") {
            ActionText("+ add", style = ShepType.actionStrong) { editing = "" }
        }
        view?.let { v ->
            val overCap = v.percent >= 80
            ShepCard {
                Text(
                    "${v.kind.uppercase()}.md · ${v.used}/${v.cap} chars",
                    style = ShepType.sectionLabel,
                )
                Spacer(Modifier.height(ShepSpace.small))
                Meter(
                    fraction = v.percent / 100f,
                    color = if (overCap) ShepPalette.peach else ShepPalette.accent,
                    modifier = Modifier.fillMaxWidth(),
                )
                Spacer(Modifier.height(ShepSpace.tight))
                Text(
                    "${v.percent}%" +
                        if (overCap) " — consolidate soon" else "",
                    style = ShepType.meta.copy(
                        color = if (overCap) ShepPalette.peach else ShepPalette.overlay0,
                    ),
                )
            }
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        val v = view
        when {
            v == null && status.isEmpty() -> LoadingState("loading…")
            v == null -> LoadingState("reconnecting…", detail = status)
            v.entries.isEmpty() -> EmptyState("no entries yet — add one with + add")
            else -> LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(ShepSpace.listGutter),
                verticalArrangement = Arrangement.spacedBy(ShepSpace.small),
            ) {
                items(v.entries, key = { it }) { entry ->
                    MemoryCard(
                        entry = entry,
                        onEdit = { editing = entry },
                        onRemove = {
                            mutate("memory.remove", JSONObject().put("find", entry), "removed")
                        },
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
                    mutate(
                        "memory.replace",
                        JSONObject().put("find", target).put("text", text),
                        "updated",
                    )
                }
            },
        )
    }
}

@Composable
fun MemoryCard(entry: String, onEdit: () -> Unit, onRemove: () -> Unit) {
    ShepCard {
        // A memory entry is a sentence a person wrote. Sans.
        Text(entry, style = ShepType.body)
        Spacer(Modifier.height(ShepSpace.tight))
        Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.small)) {
            ActionText(
                "edit",
                style = ShepType.action.copy(color = ShepPalette.accent),
                onClick = onEdit,
            )
            ActionText("remove", onClick = onRemove)
        }
    }
}

@Composable
fun MemoryEditSheet(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var text by remember { mutableStateOf(initial) }
    ShepSheet(
        title = if (initial.isEmpty()) "add entry" else "edit entry",
        onDismiss = onDismiss,
    ) {
        OutlinedTextField(
            value = text,
            onValueChange = { text = it },
            placeholder = { Text("one fact, present tense…", style = ShepType.fieldLabel) },
            textStyle = ShepType.body,
            modifier = Modifier.fillMaxWidth(),
            maxLines = 6,
        )
        Spacer(Modifier.height(ShepSpace.medium))
        ShepButton(
            if (initial.isEmpty()) "add" else "save",
            onClick = { onSave(text.trim()) },
            enabled = text.isNotBlank() && text.trim() != initial,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}
