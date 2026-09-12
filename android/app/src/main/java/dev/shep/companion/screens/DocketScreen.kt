package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.shep.companion.BridgeClient
import dev.shep.companion.DOCKET_REPEATS
import dev.shep.companion.Docket
import dev.shep.companion.DocketItem
import dev.shep.companion.DocketKind
import dev.shep.companion.DocketLane
import dev.shep.companion.DocketStatus
import dev.shep.companion.docketDueLabel
import dev.shep.companion.docketLanes
import dev.shep.companion.isDocketDate
import dev.shep.companion.parseDocket
import dev.shep.companion.parseDocketItem
import dev.shep.companion.ui.components.ActionText
import dev.shep.companion.ui.components.EmptyState
import dev.shep.companion.ui.components.LoadingState
import dev.shep.companion.ui.components.Notice
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.ShepCard
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.components.ShepSheet
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSemantic
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

/** Which sheet is open over the list, if any. */
private sealed interface DocketSheet {
    data object Add : DocketSheet
    data class Edit(val item: DocketItem) : DocketSheet
    data class Promote(val item: DocketItem) : DocketSheet
    data class Actions(val item: DocketItem) : DocketSheet
}

/**
 * Docket tab: the personal assistant's list, in the desktop board's lanes —
 * inbox, due, slated, recurring, done — with the board's verbs. The overseer
 * proposes into the inbox; this is where those proposals get promoted or
 * discarded from the sofa instead of the desk.
 *
 * Backed by `docket.list/add/update/promote/complete/discard` over the relay.
 * There are no docket events, so the list polls every five seconds while it is
 * on screen and re-reads at once after every mutation — the same shape as the
 * pane's transcript poll, for the same reason.
 */
@Composable
fun DocketScreen(client: BridgeClient) {
    var view by remember { mutableStateOf<Docket?>(null) }
    var status by remember { mutableStateOf("loading") }
    var notice by remember { mutableStateOf<String?>(null) }
    var sheet by remember { mutableStateOf<DocketSheet?>(null) }
    var collapsed by remember { mutableStateOf<Set<DocketLane>>(emptySet()) }
    val scope = rememberCoroutineScope()

    suspend fun refresh() {
        withContext(Dispatchers.IO) { runCatching { client.call("docket.list") } }
            .onSuccess { view = parseDocket(it); status = "" }
            .onFailure { if (view == null) status = "reconnect: ${it.message}" }
    }

    // Poll only while someone is looking: the tab is disposed when it is not
    // current, and the lifecycle gate covers the app going to the background.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var active by remember { mutableStateOf(true) }
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> active = true
                Lifecycle.Event.ON_STOP -> active = false
                else -> {}
            }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(client, active) {
        if (!active) return@LaunchedEffect
        while (true) {
            refresh()
            delay(5000)
        }
    }

    /**
     * One mutating call. The returned `{item}` is swapped into the list at
     * once so the row moves lanes before the next poll lands, then the list is
     * re-read so the store's ordering (and any overseer additions) win.
     */
    fun mutate(method: String, params: JSONObject, label: String) {
        scope.launch {
            withContext(Dispatchers.IO) { runCatching { client.call(method, params) } }
                .onSuccess { result ->
                    val current = view
                    val returned = result.optJSONObject("item")
                    if (current != null && returned != null) {
                        val item = parseDocketItem(returned, current.today)
                        val rest = current.items.filterNot { it.id == item.id }
                        view = current.copy(items = listOf(item) + rest)
                    }
                    notice = label
                    sheet = null
                    refresh()
                }
                .onFailure { notice = "$method failed: ${it.message}" }
        }
    }

    Column(Modifier.fillMaxSize()) {
        ScreenHeader("docket") {
            ActionText("+ add", style = ShepType.actionStrong) { sheet = DocketSheet.Add }
        }
        notice?.let { Notice(it, onDismiss = { notice = null }) }
        val v = view
        when {
            v == null && status.isEmpty() -> LoadingState("loading…")
            v == null -> LoadingState("reconnecting…", detail = status)
            v.items.none { it.status != DocketStatus.Discarded } -> EmptyState(
                "nothing on the docket",
                body = "the overseer proposes into inbox; + add for your own",
                actionLabel = "+ add",
                onAction = { sheet = DocketSheet.Add },
            )
            else -> LazyColumn(
                Modifier.fillMaxSize(),
                contentPadding = PaddingValues(bottom = ShepSpace.section),
            ) {
                docketLanes(v).forEach { (lane, rows) ->
                    val isCollapsed = lane in collapsed
                    item(key = "lane-${lane.name}") {
                        LaneHeader(
                            lane = lane,
                            count = rows.size,
                            overdue = rows.count { it.overdue },
                            collapsed = isCollapsed,
                            onToggle = {
                                collapsed = if (isCollapsed) collapsed - lane else collapsed + lane
                            },
                        )
                    }
                    if (!isCollapsed) {
                        items(rows, key = { it.id }) { row ->
                            DocketRow(
                                item = row,
                                today = v.today,
                                onClick = { sheet = DocketSheet.Edit(row) },
                                onLongClick = {
                                    if (row.status == DocketStatus.Done) {
                                        // Done is a receipt: the API refuses to
                                        // discard or reopen it, so there is no
                                        // sheet to offer.
                                        notice = "#${row.id} is done — nothing more to do with it"
                                    } else {
                                        sheet = DocketSheet.Actions(row)
                                    }
                                },
                                modifier = Modifier.padding(
                                    horizontal = ShepSpace.listGutter,
                                    vertical = ShepSpace.tight,
                                ),
                            )
                        }
                    }
                }
            }
        }
    }

    when (val open = sheet) {
        null -> {}
        DocketSheet.Add -> DocketEditSheet(
            title = "add to docket",
            initial = null,
            promoting = false,
            onDismiss = { sheet = null },
            onSave = { fields ->
                val params = JSONObject()
                    .put("title", fields.title)
                    .put("kind", fields.kind.wire)
                fields.due?.let { params.put("due", it) }
                fields.repeat?.let { params.put("repeat", it) }
                fields.notes?.let { params.put("notes", it) }
                mutate("docket.add", params, "added")
            },
        )
        is DocketSheet.Edit -> DocketEditSheet(
            title = "#${open.item.id}",
            initial = open.item,
            promoting = false,
            onDismiss = { sheet = null },
            onSave = { fields ->
                // Only what changed: the server leaves omitted fields alone,
                // and it cannot clear a date or a repeat once set.
                val params = JSONObject().put("id", open.item.id)
                if (fields.title != open.item.title) params.put("title", fields.title)
                if (fields.notes != null && fields.notes != open.item.notes) params.put("notes", fields.notes)
                if (fields.due != null && fields.due != open.item.due) params.put("due", fields.due)
                if (fields.repeat != null && fields.repeat != open.item.repeat) params.put("repeat", fields.repeat)
                if (fields.kind != open.item.kind) params.put("kind", fields.kind.wire)
                mutate("docket.update", params, "updated #${open.item.id}")
            },
        )
        is DocketSheet.Promote -> DocketEditSheet(
            title = "promote #${open.item.id}",
            initial = open.item.copy(kind = DocketKind.Slated),
            promoting = true,
            onDismiss = { sheet = null },
            onSave = { fields ->
                val params = JSONObject().put("id", open.item.id).put("kind", fields.kind.wire)
                fields.due?.let { params.put("due", it) }
                fields.repeat?.let { params.put("repeat", it) }
                mutate("docket.promote", params, "promoted #${open.item.id} as ${fields.kind.wire}")
            },
        )
        is DocketSheet.Actions -> DocketActionsSheet(
            item = open.item,
            onDismiss = { sheet = null },
            onPromoteSlated = { sheet = DocketSheet.Promote(open.item) },
            onPromoteWeekly = {
                mutate(
                    "docket.promote",
                    JSONObject().put("id", open.item.id).put("kind", DocketKind.Recurring.wire).put("repeat", "1w"),
                    "promoted #${open.item.id} as recurring",
                )
            },
            onDone = {
                mutate("docket.complete", JSONObject().put("id", open.item.id), "done #${open.item.id}")
            },
            onEdit = { sheet = DocketSheet.Edit(open.item) },
            onDiscard = {
                mutate("docket.discard", JSONObject().put("id", open.item.id), "discarded #${open.item.id}")
            },
        )
    }
}

/**
 * A lane heading. The count rides the right edge, and the due lane adds its
 * overdue count in peach with the same `!` the cards carry — the one heading
 * that says more than a number.
 */
@Composable
private fun LaneHeader(
    lane: DocketLane,
    count: Int,
    overdue: Int,
    collapsed: Boolean,
    onToggle: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .background(ShepPalette.panelBg)
            .minimumInteractiveComponentSize()
            .clickable(onClick = onToggle)
            .padding(start = ShepSpace.medium, end = ShepSpace.screen),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            if (collapsed) "▸" else "▾",
            style = ShepType.state.copy(color = ShepPalette.overlay1),
        )
        Spacer(Modifier.width(ShepSpace.snug))
        Text(lane.title, style = ShepType.sectionLabel)
        Spacer(Modifier.weight(1f))
        Text("$count", style = ShepType.badge.copy(color = ShepPalette.overlay0))
        if (overdue > 0) {
            Spacer(Modifier.width(ShepSpace.snug))
            Text("!$overdue", style = ShepType.badge.copy(color = ShepPalette.peach))
        }
    }
}

/**
 * The docket card from `docs/DESIGN-LANGUAGE.md`: at most four rows, and only
 * the rows the item has something to put on. The id leads the title because
 * the id is what the CLI verbs take.
 */
@Composable
private fun DocketRow(
    item: DocketItem,
    today: String,
    onClick: () -> Unit,
    onLongClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val look = ShepSemantic.docket(item.status.wire, overdue = item.overdue, dueToday = item.dueToday)
    val dateInk = when {
        item.overdue -> ShepPalette.peach
        item.dueToday -> ShepPalette.yellow
        else -> ShepPalette.overlay0
    }
    ShepCard(
        modifier = modifier.semantics { contentDescription = "docket item ${item.id}, ${look.description}" },
        onClick = onClick,
        onLongClick = onLongClick,
    ) {
        Row(verticalAlignment = Alignment.Top) {
            Text(
                look.glyph,
                style = ShepType.stateGlyphSmall.copy(
                    color = look.color,
                    fontWeight = if (item.overdue) FontWeight.Bold else ShepType.stateGlyphSmall.fontWeight,
                ),
            )
            Spacer(Modifier.width(ShepSpace.small))
            Text("#${item.id}", style = ShepType.meta)
            Spacer(Modifier.width(ShepSpace.snug))
            Text(
                item.title,
                style = ShepType.itemName.copy(
                    color = if (item.status == DocketStatus.Done) ShepPalette.overlay1 else ShepPalette.text,
                ),
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
        }
        Spacer(Modifier.height(ShepSpace.tight))
        Row(verticalAlignment = Alignment.CenterVertically) {
            Spacer(Modifier.width(ShepSpace.indent))
            Text(item.kind.wire, style = ShepType.metaSmall)
            Text(" · ", style = ShepType.metaSmall)
            Text(docketDueLabel(item.due, today), style = ShepType.metaSmall.copy(color = dateInk))
            item.repeat?.let {
                Text(" · ", style = ShepType.metaSmall)
                Text("every $it", style = ShepType.metaSmall)
            }
        }
        item.sourceLabel?.let {
            Row {
                Spacer(Modifier.width(ShepSpace.indent))
                Text(it, style = ShepType.metaSmall.copy(color = ShepPalette.teal), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        item.notes?.lineSequence()?.firstOrNull { it.isNotBlank() }?.let { first ->
            Row {
                Spacer(Modifier.width(ShepSpace.indent))
                Text(
                    first,
                    style = ShepType.bodySmall.copy(fontStyle = FontStyle.Italic, color = ShepPalette.subtext0),
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/**
 * The verbs the desktop board has on a card: `p` promote, `d` done, `x`
 * discard, plus edit. An inbox item is promoted or discarded; an open item is
 * done, edited or discarded. Done items never reach here — the API keeps them
 * as a receipt and refuses both discard and reopen.
 */
@Composable
private fun DocketActionsSheet(
    item: DocketItem,
    onDismiss: () -> Unit,
    onPromoteSlated: () -> Unit,
    onPromoteWeekly: () -> Unit,
    onDone: () -> Unit,
    onEdit: () -> Unit,
    onDiscard: () -> Unit,
) {
    ShepSheet(title = "#${item.id}", onDismiss = onDismiss) {
        Text(item.title, style = ShepType.summary, maxLines = 3, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(ShepSpace.small))
        when (item.status) {
            DocketStatus.Inbox -> {
                DocketSheetRow("promote as slated", hint = "one-off, pick a date", onClick = onPromoteSlated)
                DocketSheetRow("promote as recurring", hint = "weekly, from today", onClick = onPromoteWeekly)
                DocketSheetRow("edit", hint = "title, notes", onClick = onEdit)
            }
            DocketStatus.Open -> {
                DocketSheetRow(
                    "done",
                    hint = if (item.repeat != null) "rolls the date forward" else "settles it",
                    onClick = onDone,
                )
                DocketSheetRow("edit", hint = "title, date, repeat, notes", onClick = onEdit)
            }
            DocketStatus.Done, DocketStatus.Discarded -> {}
        }
        DocketSheetRow("discard", tone = ShepPalette.red, onClick = onDiscard)
    }
}

/** One line in a sheet: what it does, and — when it is not obvious — what that means. */
@Composable
private fun DocketSheetRow(
    label: String,
    hint: String? = null,
    tone: Color = ShepPalette.text,
    onClick: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clickable(onClick = onClick)
            .padding(vertical = ShepSpace.tight),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = ShepType.action.copy(color = tone), modifier = Modifier.weight(1f))
        hint?.let { Text(it, style = ShepType.metaSmall, maxLines = 1) }
    }
}

/** What the edit sheet hands back. Blank optional fields are `null`, never `""`. */
private data class DocketFields(
    val title: String,
    val kind: DocketKind,
    val due: String?,
    val repeat: String?,
    val notes: String?,
)

/**
 * One sheet for add, edit and promote. [promoting] narrows it to what
 * `docket.promote` takes — kind (slated or recurring), date, repeat — and
 * shows the title read-only; the other two modes edit every field. The date is
 * typed as `YYYY-MM-DD`, the way the CLI and the desktop take it, and the save
 * button waits until it parses.
 */
@Composable
private fun DocketEditSheet(
    title: String,
    initial: DocketItem?,
    promoting: Boolean,
    onDismiss: () -> Unit,
    onSave: (DocketFields) -> Unit,
) {
    var text by remember { mutableStateOf(initial?.title ?: "") }
    var notes by remember { mutableStateOf(initial?.notes ?: "") }
    var due by remember { mutableStateOf(initial?.due ?: "") }
    var repeat by remember { mutableStateOf(initial?.repeat) }
    var kind by remember { mutableStateOf(initial?.kind ?: DocketKind.Captured) }
    val dueValid = due.isBlank() || isDocketDate(due.trim())
    val needsRepeat = kind == DocketKind.Recurring && repeat == null
    val canSave = text.isNotBlank() && dueValid && !needsRepeat

    ShepSheet(title = title, onDismiss = onDismiss) {
        if (promoting) {
            Text(text, style = ShepType.summary, maxLines = 3, overflow = TextOverflow.Ellipsis)
        } else {
            OutlinedTextField(
                value = text,
                onValueChange = { text = it },
                label = { Text("title", style = ShepType.fieldLabel) },
                textStyle = ShepType.field,
                modifier = Modifier.fillMaxWidth(),
                singleLine = true,
            )
        }
        Spacer(Modifier.height(ShepSpace.medium))
        Text("kind", style = ShepType.sectionLabel)
        Spacer(Modifier.height(ShepSpace.snug))
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            val kinds = if (promoting) listOf(DocketKind.Slated, DocketKind.Recurring) else DocketKind.entries
            kinds.forEach { option ->
                ShepChip(option.wire, option == kind) {
                    kind = option
                    if (option == DocketKind.Recurring && repeat == null) repeat = "1w"
                }
            }
        }
        Spacer(Modifier.height(ShepSpace.medium))
        OutlinedTextField(
            value = due,
            onValueChange = { due = it },
            label = { Text("due YYYY-MM-DD", style = ShepType.fieldLabel) },
            textStyle = ShepType.field,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            isError = !dueValid,
            supportingText = if (dueValid) null else ({ Text("not a date", style = ShepType.metaSmall.copy(color = ShepPalette.peach)) }),
        )
        Spacer(Modifier.height(ShepSpace.medium))
        Text("repeat", style = ShepType.sectionLabel)
        Spacer(Modifier.height(ShepSpace.snug))
        Row(
            Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            ShepChip("none", repeat == null) { repeat = null }
            DOCKET_REPEATS.forEach { option ->
                ShepChip(option, option == repeat) { repeat = option }
            }
        }
        if (needsRepeat) {
            Spacer(Modifier.height(ShepSpace.tight))
            Text("a recurring item needs a repeat", style = ShepType.metaSmall.copy(color = ShepPalette.peach))
        }
        if (!promoting) {
            Spacer(Modifier.height(ShepSpace.medium))
            OutlinedTextField(
                value = notes,
                onValueChange = { notes = it },
                label = { Text("notes", style = ShepType.fieldLabel) },
                textStyle = ShepType.body,
                modifier = Modifier.fillMaxWidth(),
                maxLines = 4,
            )
        }
        initial?.sourceLabel?.takeIf { !promoting }?.let {
            Spacer(Modifier.height(ShepSpace.small))
            Text("source $it", style = ShepType.metaSmall.copy(color = ShepPalette.teal))
        }
        Spacer(Modifier.height(ShepSpace.medium))
        ShepButton(
            when {
                promoting -> "promote"
                initial == null -> "add"
                else -> "save"
            },
            enabled = canSave,
            modifier = Modifier.fillMaxWidth(),
        ) {
            onSave(
                DocketFields(
                    title = text.trim(),
                    kind = kind,
                    due = due.trim().ifBlank { null },
                    repeat = repeat,
                    notes = notes.trim().ifBlank { null },
                ),
            )
        }
    }
}
