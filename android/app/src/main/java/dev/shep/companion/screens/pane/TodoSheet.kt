package dev.shep.companion.screens.pane

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import dev.shep.companion.TodoItem
import dev.shep.companion.Todos
import dev.shep.companion.todoIsOpen
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/**
 * The checklist the agent is working through, read-only.
 *
 * A sheet rather than a third output mode: `out live | recorded` is two states
 * shown as the one word you are not on, and a third would turn a toggle into a
 * cycle. This is also genuinely a glance rather than a place you sit — you open
 * it to see how far along a run is, then go back to the terminal.
 *
 * Nothing here is editable. The list is the agent's own working state; a phone
 * reaching in to tick an item off would be telling the agent something it did
 * not do.
 */
@Composable
fun TodoSheet(
    todos: Todos?,
    error: String?,
    loading: Boolean,
    onDismiss: () -> Unit,
) {
    ShepSheet(title = "todos", onDismiss = onDismiss, showCancel = true) {
        when {
            error != null -> Text(
                error,
                style = ShepType.meta.copy(color = ShepPalette.peach),
                modifier = Modifier.testTag("todos-error"),
            )

            todos == null && loading -> Text("reading the session…", style = ShepType.meta)

            todos == null || todos.items.isEmpty() -> Text(
                "no checklist in this session",
                style = ShepType.emptyState,
                modifier = Modifier.testTag("todos-empty"),
            )

            else -> {
                val open = todos.items.count { todoIsOpen(it.status) }
                Text(
                    "$open of ${todos.items.size} left" +
                        if (todos.source == "transcript") " · folded from the transcript" else "",
                    style = ShepType.meta.copy(color = ShepPalette.subtext0),
                )
                LazyColumn(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(max = ShepSize.sheetListMax)
                        .padding(top = ShepSpace.small),
                    verticalArrangement = Arrangement.spacedBy(ShepSpace.tight),
                ) {
                    items(todos.items, key = { it.id }) { TodoRow(it) }
                }
            }
        }
    }
}

@Composable
private fun TodoRow(item: TodoItem) {
    val (glyph, colour) = todoAppearance(item.status)
    // An item in flight says what it is doing; everything else says what it is.
    val label = item.activeForm
        .takeIf { item.status == "in_progress" && it.isNotBlank() }
        ?: item.subject
    Row(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        verticalAlignment = Alignment.Top,
    ) {
        Text(glyph, style = ShepType.viewTitle.copy(color = colour))
        Column(Modifier.weight(1f)) {
            Text(
                label,
                style = if (item.status == "completed") {
                    ShepType.meta.copy(color = ShepPalette.overlay1)
                } else {
                    ShepType.body
                },
            )
            if (item.blockedBy.isNotEmpty()) {
                Text(
                    "waiting on ${item.blockedBy.joinToString(", ") { "#$it" }}",
                    style = ShepType.meta.copy(color = ShepPalette.peach),
                )
            }
        }
    }
}

/**
 * Statuses borrow the agent-state tiers so the two never disagree on screen:
 * working is yellow, done is blue, and anything not started yet is overlay ink
 * rather than a colour, because a pending item is not a state worth interrupting
 * anyone for.
 */
private fun todoAppearance(status: String): Pair<String, Color> = when (status) {
    "completed" -> "✓" to ShepPalette.blue
    "in_progress" -> "◐" to ShepPalette.yellow
    "cancelled" -> "·" to ShepPalette.overlay0
    else -> "◦" to ShepPalette.overlay0
}
