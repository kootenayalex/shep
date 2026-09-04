package dev.shep.companion

import org.json.JSONArray
import org.json.JSONObject

/** One agent row on the home screen, joined with its workspace label. */
data class AgentRow(
    val terminalId: String,
    val paneId: String,
    val workspaceId: String,
    val workspaceLabel: String,
    val agent: String,
    val status: String,
    val contextPercent: Int?,
    val reviewState: String,
    // Meta surfaced from the snapshot. Branch/±/age are NOT in session.snapshot
    // (git status is a separate event stream) — that meta needs an API extension.
    val customStatus: String?,
    val worktreeRepo: String?,
    val isWorktree: Boolean,
    val memoryPercent: Int?,
    // Placement and display facts, present only from `session.overview`. A
    // server old enough to lack that method leaves them null and the board
    // degrades to what `session.snapshot` can answer.
    val tabName: String? = null,
    val paneNumber: Int? = null,
    val branch: String? = null,
    val displayAgent: String? = null,
    /**
     * What to call this agent, decided by the server so the desktop board and
     * the phone cannot call the same agent different things. Falls back to
     * [agent] against a server too old to send it.
     */
    val displayName: String? = null,
    val activityLine: String? = null,
    /**
     * The last few lines of the agent's screen, oldest first, for a surface
     * with room for more than one. Empty against a server too old to send it;
     * [activityLine] is always the last entry when both are present.
     */
    val activityLines: List<String> = emptyList(),
    val cwd: String? = null,
    val stateAgeSeconds: Long? = null,
    val queuedInput: Int = 0,
    /**
     * A state someone set by hand, overriding what shep detected. Null when
     * the agent is showing its detected state, which is nearly always.
     */
    val manualState: ManualState? = null,
) {
    /**
     * Where this agent lives, the way the desktop board writes it: the tab's
     * name when it has one, and a pane number only when the tab holds more
     * than one pane. `t2·p1` meant nothing to read at a glance.
     */
    val location: String?
        get() {
            val tab = tabName?.takeIf { it.isNotBlank() }
            val pane = paneNumber?.let { "p$it" }
            return when {
                tab != null && pane != null -> "$tab·$pane"
                tab != null -> tab
                else -> pane
            }
        }
}

/** Session-wide counts behind the dashboard strip. */
data class SessionTotals(
    val agents: Int = 0,
    val blocked: Int = 0,
    val done: Int = 0,
    val working: Int = 0,
    val idle: Int = 0,
    val attention: Int = 0,
    val workspaces: Int = 0,
    val tabs: Int = 0,
    val panes: Int = 0,
    val queuedInput: Int = 0,
    val pendingTasks: Int? = null,
)

/**
 * Facts about the machine the session runs on. All optional: a host that
 * cannot answer reports nothing rather than a plausible zero, and the strip
 * renders an em dash.
 */
data class SessionHost(
    val version: String? = null,
    val loadPercent: Int? = null,
    val cores: Int? = null,
    val memoryPercent: Int? = null,
    val memoryTotalBytes: Long? = null,
    val memoryUsedBytes: Long? = null,
)

/**
 * A manual state, as the server names it. [name] is the wire id (`blocked`,
 * `idle`, or a configured custom name), [label] what to print, and [tier] the
 * appearance family — one of the seven `ManualStateTier` names in
 * src/api/schema/common.rs. The tier crosses the wire so a custom state can
 * carry its configured ink; colours and glyphs themselves never do.
 */
data class ManualState(
    val name: String,
    val label: String,
    val tier: String,
)

/** The whole board in one payload. */
data class SessionOverview(
    val totals: SessionTotals,
    val host: SessionHost,
    val agents: List<AgentRow>,
    /**
     * Every state the picker may offer beyond the builtins, straight from the
     * server's `[[states.custom]]` config. Empty against a server too old to
     * send it, or one with nothing configured.
     */
    val customStates: List<ManualState> = emptyList(),
)

private fun JSONObject.optManualState(key: String): ManualState? {
    val o = optJSONObject(key) ?: return null
    return parseManualState(o)
}

private fun parseManualState(o: JSONObject): ManualState? {
    val name = o.optStringOrNull("name") ?: return null
    return ManualState(
        name = name,
        label = o.optStringOrNull("label") ?: name,
        tier = o.optStringOrNull("tier") ?: "absent",
    )
}

private fun JSONObject.optIntOrNull(key: String): Int? =
    if (has(key) && !isNull(key)) optInt(key) else null

private fun JSONObject.optLongOrNull(key: String): Long? =
    if (has(key) && !isNull(key)) optLong(key) else null

private fun JSONObject.optStringOrNull(key: String): String? =
    if (has(key) && !isNull(key)) optString(key).takeIf { it.isNotEmpty() } else null

private fun JSONObject.optStringList(key: String): List<String> {
    val array = optJSONArray(key) ?: return emptyList()
    return (0 until array.length()).mapNotNull { array.optString(it).takeIf(String::isNotEmpty) }
}

/**
 * Parse a `session.overview` result. The server already sorts agents in
 * attention order, so this preserves array order rather than re-sorting —
 * that is what keeps the phone and the desktop board showing the same thing.
 */
fun parseOverview(result: JSONObject): SessionOverview? {
    val overview = result.optJSONObject("overview") ?: return null
    val t = overview.optJSONObject("totals") ?: JSONObject()
    val h = overview.optJSONObject("host") ?: JSONObject()
    val agents = mutableListOf<AgentRow>()
    val array = overview.optJSONArray("agents") ?: JSONArray()
    for (i in 0 until array.length()) {
        val a = array.optJSONObject(i) ?: continue
        agents.add(
            AgentRow(
                terminalId = a.optString("pane_id"),
                paneId = a.optString("pane_id"),
                workspaceId = a.optString("workspace_id"),
                workspaceLabel = a.optString("workspace_label"),
                agent = a.optStringOrNull("name") ?: "agent",
                status = a.optString("agent_status", "unknown"),
                contextPercent = a.optIntOrNull("context_percent"),
                // Review state is not part of the overview; the board's job is
                // agent state. The review flow still reads it per-agent.
                reviewState = "",
                customStatus = a.optStringOrNull("custom_status"),
                worktreeRepo = null,
                isWorktree = false,
                memoryPercent = null,
                tabName = a.optStringOrNull("tab_name"),
                paneNumber = a.optIntOrNull("pane_number"),
                branch = a.optStringOrNull("branch"),
                displayAgent = a.optStringOrNull("display_agent"),
                displayName = a.optStringOrNull("display_name"),
                activityLine = a.optStringOrNull("activity_line"),
                activityLines = a.optStringList("activity_lines"),
                cwd = a.optStringOrNull("cwd"),
                stateAgeSeconds = a.optLongOrNull("state_age_seconds"),
                queuedInput = a.optInt("queued_input", 0),
                manualState = a.optManualState("manual_state"),
            )
        )
    }
    val customStates = mutableListOf<ManualState>()
    val customArray = overview.optJSONArray("custom_states") ?: JSONArray()
    for (i in 0 until customArray.length()) {
        customArray.optJSONObject(i)?.let(::parseManualState)?.let(customStates::add)
    }
    return SessionOverview(
        totals = SessionTotals(
            agents = t.optInt("agents"),
            blocked = t.optInt("blocked"),
            done = t.optInt("done"),
            working = t.optInt("working"),
            idle = t.optInt("idle"),
            attention = t.optInt("attention"),
            workspaces = t.optInt("workspaces"),
            tabs = t.optInt("tabs"),
            panes = t.optInt("panes"),
            queuedInput = t.optInt("queued_input"),
            pendingTasks = t.optIntOrNull("pending_tasks"),
        ),
        host = SessionHost(
            version = h.optStringOrNull("version"),
            loadPercent = h.optIntOrNull("load_percent"),
            cores = h.optIntOrNull("cores"),
            memoryPercent = h.optIntOrNull("memory_percent"),
            memoryTotalBytes = h.optLongOrNull("memory_total_bytes"),
            memoryUsedBytes = h.optLongOrNull("memory_used_bytes"),
        ),
        agents = agents,
        customStates = customStates,
    )
}

/**
 * Totals derived from rows alone, for a server too old to serve
 * `session.overview`. Session shape and host vitals are simply unknown there —
 * they stay zero/null rather than being guessed at.
 */
fun totalsFromRows(rows: List<AgentRow>): SessionTotals = SessionTotals(
    agents = rows.size,
    blocked = rows.count { it.status == "blocked" },
    done = rows.count { it.status == "done" },
    working = rows.count { it.status == "working" },
    idle = rows.count { it.status == "idle" },
    attention = rows.count { it.status == "blocked" || it.status == "done" },
    queuedInput = rows.sumOf { it.queuedInput },
)

/** `1234567890` -> `1.1G`, matching the desktop strip. */
fun humanBytes(bytes: Long): String {
    val units = listOf(1L shl 30 to "G", 1L shl 20 to "M", 1L shl 10 to "K")
    for ((scale, suffix) in units) {
        if (bytes >= scale) return String.format("%.1f%s", bytes.toDouble() / scale, suffix)
    }
    return "${bytes}B"
}

/** `4m`, `2h` — the desktop board's compact age format. */
fun formatAge(seconds: Long): String = when {
    seconds < 60 -> "${seconds}s"
    seconds < 3600 -> "${seconds / 60}m"
    seconds < 86400 -> "${seconds / 3600}h"
    else -> "${seconds / 86400}d"
}

/**
 * What this agent is doing, in words, right now.
 *
 * The board says `blocked` and quotes the agent's own screen, which is exactly
 * right for someone who has read a hundred of these rows and wrong for the
 * first ten. This is the same three facts — state, what it is chewing on, how
 * long — said as a sentence, above the raw lines rather than instead of them.
 *
 * Two things it must never render, because Maestro anchors on both elsewhere in
 * the hierarchy: a bare `live` (flow 07 taps the first of those by index) and
 * anything full-matching `\S+ blocked` (flow 13).
 */
fun nowLine(
    status: String,
    manualLabel: String?,
    ageSeconds: Long?,
    activityLine: String?,
): String {
    val trimmed = activityLine?.let { trimActivity(it) }?.takeIf { it.isNotEmpty() }
    val head = when {
        // A label set by hand wins over anything shep detected: the point of
        // setting one is that shep had it wrong.
        manualLabel != null -> manualLabel
        status == "blocked" ->
            if (trimmed != null) "waiting for you — $trimmed" else "waiting for you"
        status == "working" -> if (trimmed != null) "working · $trimmed" else "working"
        status == "done" -> "finished — ready for you to look at"
        status == "idle" -> "idle"
        else -> status
    }
    val line = if (ageSeconds == null) head else "$head · ${formatAge(ageSeconds)}"
    // Maestro matches the *whole* text of an element, and flow 07 taps the
    // first one that reads exactly `live` — the out toggle. This line renders
    // above it, so a state literally called `live` with no age beside it would
    // sit in front of the toggle and take the tap.
    return if (line == "live") "running" else line
}

/**
 * One line of an agent's screen, reduced to something that fits in a sentence:
 * no spinner frames, no prompt marker, no runs of whitespace, no essay.
 */
internal fun trimActivity(line: String): String {
    val stripped = line
        .trim()
        .trimStart('⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦',
            '⠧', '⠇', '⠏', '•', '●', '│', '┃', '╭',
            '╰', '─', '✱', '✹', '*', '>', ' ')
        .trim()
    val collapsed = stripped.replace(Regex("""\s+"""), " ")
    return if (collapsed.length <= ACTIVITY_MAX) {
        collapsed
    } else {
        collapsed.take(ACTIVITY_MAX).trimEnd() + "…"
    }
}

private const val ACTIVITY_MAX = 60

/** Sort weight: blocked demands attention first, then done (unseen), working, idle. */
fun statusPriority(status: String): Int = when (status) {
    "blocked" -> 0
    "done" -> 1
    "working" -> 2
    "idle" -> 3
    else -> 4
}

fun parseSnapshot(result: JSONObject): List<AgentRow> {
    val snapshot = result.optJSONObject("snapshot") ?: return emptyList()
    val workspaceLabels = mutableMapOf<String, String>()
    val reviewStates = mutableMapOf<String, String>()
    val memoryPercents = mutableMapOf<String, Int>()
    val worktreeRepos = mutableMapOf<String, String>()
    val worktreeLinked = mutableMapOf<String, Boolean>()
    val workspaces = snapshot.optJSONArray("workspaces")
    if (workspaces != null) {
        for (i in 0 until workspaces.length()) {
            val ws = workspaces.getJSONObject(i)
            val id = ws.getString("workspace_id")
            workspaceLabels[id] = ws.optString("label")
            reviewStates[id] = ws.optString("review_state", "none")
            if (ws.has("memory_usage_percent")) memoryPercents[id] = ws.getInt("memory_usage_percent")
            ws.optJSONObject("worktree")?.let { wt ->
                worktreeRepos[id] = wt.optString("repo_name")
                worktreeLinked[id] = wt.optBoolean("is_linked_worktree")
            }
        }
    }
    val agents = snapshot.optJSONArray("agents") ?: return emptyList()
    val rows = mutableListOf<AgentRow>()
    for (i in 0 until agents.length()) {
        val agent = agents.getJSONObject(i)
        val workspaceId = agent.optString("workspace_id")
        rows.add(
            AgentRow(
                terminalId = agent.optString("terminal_id"),
                paneId = agent.optString("pane_id"),
                workspaceId = workspaceId,
                workspaceLabel = workspaceLabels[workspaceId] ?: workspaceId,
                agent = agent.optString("display_agent")
                    .ifEmpty { agent.optString("agent") }
                    .ifEmpty { "shell" },
                status = agent.optString("agent_status", "unknown"),
                contextPercent = if (agent.has("context_percent")) agent.getInt("context_percent") else null,
                reviewState = reviewStates[workspaceId] ?: "none",
                customStatus = agent.optString("custom_status").ifEmpty { null },
                worktreeRepo = worktreeRepos[workspaceId],
                isWorktree = worktreeLinked[workspaceId] ?: false,
                memoryPercent = memoryPercents[workspaceId],
            )
        )
    }
    return rows.sortedWith(compareBy({ statusPriority(it.status) }, { it.workspaceLabel }))
}

/** One queued/dispatched task from `task.list` (A4). */
data class TaskRow(
    val id: Long,
    val prompt: String,
    val repo: String,
    val runtime: String,
    val useWorktree: Boolean,
    val state: String,
    val workspaceId: String?,
    val assignedPaneId: String? = null,
)

/** Zeigarnik order: open loops first (running, blocked, todo), finished last. */
fun taskStatePriority(state: String): Int = when (state) {
    "running" -> 0
    "blocked" -> 1
    "todo" -> 2
    "done" -> 3
    "cancelled" -> 4
    else -> 5
}

/** A task is still actionable (dispatchable / cancellable) while unfinished. */
fun taskIsOpen(state: String): Boolean = state == "todo" || state == "blocked"

fun parseTasks(result: JSONObject): List<TaskRow> {
    val arr = result.optJSONArray("tasks") ?: return emptyList()
    val rows = mutableListOf<TaskRow>()
    for (i in 0 until arr.length()) {
        val t = arr.getJSONObject(i)
        rows.add(
            TaskRow(
                id = t.optLong("id"),
                prompt = t.optString("prompt"),
                repo = t.optString("repo"),
                runtime = t.optString("runtime", "claude"),
                useWorktree = t.optBoolean("use_worktree"),
                state = t.optString("state", "todo"),
                // org.json's optString returns the literal "null" for a JSON null,
                // so guard with isNull before falling back to empty→null.
                workspaceId = if (t.isNull("workspace_id")) null
                else t.optString("workspace_id").ifEmpty { null },
                assignedPaneId = if (t.isNull("assigned_pane_id")) null
                else t.optString("assigned_pane_id").ifEmpty { null },
            )
        )
    }
    return rows.sortedWith(compareBy({ taskStatePriority(it.state) }, { -it.id }))
}

/** Basename of a repo path, for compact display. */
fun repoName(path: String): String = path.trimEnd('/').substringAfterLast('/').ifEmpty { path }

/** A memory file's entries + cap usage from `memory.show`/add/replace/remove (A4). */
data class MemoryView(
    val kind: String,
    val entries: List<String>,
    val used: Int,
    val cap: Int,
    val percent: Int,
)

fun parseMemory(result: JSONObject): MemoryView {
    val entriesArr: JSONArray = result.optJSONArray("entries") ?: JSONArray()
    val entries = (0 until entriesArr.length()).map { entriesArr.getString(it) }
    return MemoryView(
        kind = result.optString("kind", "user"),
        entries = entries,
        used = result.optInt("used"),
        cap = result.optInt("cap"),
        percent = result.optInt("percent"),
    )
}

/**
 * The session's shape: groups, their tabs, and the panes inside them.
 *
 * The board answers "who needs me", ordered by attention and flat on purpose.
 * This answers the other question — "what is open, and where" — which is the
 * one you need to close something or start something beside it. Both come from
 * the same server, so a rename in either place is the same rename.
 *
 * "Group" is the word every shep surface uses; the API says `workspace`.
 */
data class PaneNode(
    val paneId: String,
    val tabId: String,
    val agent: String?,
    val status: String,
    val cwd: String?,
    val focused: Boolean,
)

/**
 * What the pane view needs, from what the tree knows.
 *
 * `session.snapshot` answers with agents, so a plain shell is simply not in it
 * — and the tree lists shells. This fills in the facts a tree node cannot know
 * (branch, context, review state) with the same nulls a snapshot-only server
 * would give, which the pane view already handles.
 */
fun PaneNode.asAgentRow(workspaceLabel: String? = null): AgentRow = AgentRow(
    terminalId = "",
    paneId = paneId,
    workspaceId = tabId.substringBefore(":"),
    workspaceLabel = workspaceLabel ?: tabId.substringBefore(":"),
    agent = agent ?: "shell",
    status = status,
    contextPercent = null,
    reviewState = "none",
    customStatus = null,
    worktreeRepo = null,
    isWorktree = false,
    memoryPercent = null,
    cwd = cwd,
)

data class TabNode(
    val tabId: String,
    val workspaceId: String,
    val label: String,
    val number: Int,
    val status: String,
    val focused: Boolean,
    val panes: List<PaneNode>,
)

data class GroupNode(
    val workspaceId: String,
    val label: String,
    val number: Int,
    val status: String,
    val reviewState: String,
    val focused: Boolean,
    val activeTabId: String?,
    val worktreeRepo: String?,
    val isWorktree: Boolean,
    val tabs: List<TabNode>,
) {
    /**
     * Closing the last tab in a group is refused by the server — the group is
     * what you close instead. Asking here keeps the UI from offering a button
     * that can only return an error.
     */
    val hasOnlyOneTab: Boolean get() = tabs.size <= 1
}

/**
 * Build the group → tab → pane tree from one `session.snapshot`.
 *
 * The snapshot is three flat lists plus ids, so this is the join. Order is the
 * server's own: groups and tabs come back in session order, which is the order
 * the desktop shows them in, and re-sorting would be the phone inventing a
 * second arrangement of the same session.
 */
fun parseTree(result: JSONObject): List<GroupNode> {
    val snapshot = result.optJSONObject("snapshot") ?: return emptyList()

    val panesByTab = mutableMapOf<String, MutableList<PaneNode>>()
    val panes = snapshot.optJSONArray("panes") ?: JSONArray()
    for (i in 0 until panes.length()) {
        val p = panes.optJSONObject(i) ?: continue
        val tabId = p.optString("tab_id")
        panesByTab.getOrPut(tabId) { mutableListOf() }.add(
            PaneNode(
                paneId = p.optString("pane_id"),
                tabId = tabId,
                agent = p.optStringOrNull("label")
                    ?: p.optStringOrNull("display_agent")
                    ?: p.optStringOrNull("agent"),
                status = p.optString("agent_status", "unknown"),
                cwd = p.optStringOrNull("cwd"),
                focused = p.optBoolean("focused"),
            )
        )
    }

    val tabsByWorkspace = mutableMapOf<String, MutableList<TabNode>>()
    val tabs = snapshot.optJSONArray("tabs") ?: JSONArray()
    for (i in 0 until tabs.length()) {
        val t = tabs.optJSONObject(i) ?: continue
        val workspaceId = t.optString("workspace_id")
        val tabId = t.optString("tab_id")
        tabsByWorkspace.getOrPut(workspaceId) { mutableListOf() }.add(
            TabNode(
                tabId = tabId,
                workspaceId = workspaceId,
                label = t.optString("label"),
                number = t.optInt("number"),
                status = t.optString("agent_status", "unknown"),
                focused = t.optBoolean("focused"),
                panes = panesByTab[tabId].orEmpty(),
            )
        )
    }

    val groups = mutableListOf<GroupNode>()
    val workspaces = snapshot.optJSONArray("workspaces") ?: JSONArray()
    for (i in 0 until workspaces.length()) {
        val w = workspaces.optJSONObject(i) ?: continue
        val id = w.optString("workspace_id")
        val worktree = w.optJSONObject("worktree")
        groups.add(
            GroupNode(
                workspaceId = id,
                label = w.optString("label").ifEmpty { id },
                number = w.optInt("number"),
                status = w.optString("agent_status", "unknown"),
                reviewState = w.optString("review_state", "none"),
                focused = w.optBoolean("focused"),
                activeTabId = w.optStringOrNull("active_tab_id"),
                worktreeRepo = worktree?.optStringOrNull("repo_name"),
                isWorktree = worktree?.optBoolean("is_linked_worktree") ?: false,
                tabs = tabsByWorkspace[id].orEmpty(),
            )
        )
    }
    return groups
}

// --------------------------------------------------------------------------
// Transcript — the recorded view of a pane
// --------------------------------------------------------------------------

/** One tool the agent reached for, with what came back. */
data class ToolCall(
    val name: String,
    val summary: String,
    /** null when the call is still outstanding — the agent is mid-turn. */
    val ok: Boolean?,
    val preview: String,
)

/**
 * A piece of an assistant reply, in the order it happened.
 *
 * Prose and tool calls interleave — "I'll check the logs", run Bash, "found
 * it" — and a reply rendered as a paragraph followed by a list of tools loses
 * which sentence each call belongs to.
 */
sealed interface Block {
    data class Prose(val text: String) : Block
    data class Tool(val call: ToolCall) : Block
}

data class Turn(
    val role: String,
    val ts: String,
    val text: String,
    val thinking: String,
    val blocks: List<Block>,
)

/**
 * A pane's conversation.
 *
 * [source] is how sure the server is that this is the right session:
 * `reported` (the agent said so), `only` (nothing else has run in this
 * directory), or `matched` (fingerprinted against the pane's screen). The UI
 * shows the difference — a matched transcript is a good guess, not a fact.
 */
data class Transcript(
    val sessionId: String?,
    val source: String,
    val truncated: Boolean,
    val turns: List<Turn>,
)

fun parseTranscript(result: JSONObject): Transcript? {
    val t = result.optJSONObject("transcript") ?: return null
    val arr = t.optJSONArray("turns")
    val turns = mutableListOf<Turn>()
    for (i in 0 until (arr?.length() ?: 0)) {
        val turn = arr?.optJSONObject(i) ?: continue
        val blocksArr = turn.optJSONArray("blocks")
        val blocks = mutableListOf<Block>()
        for (j in 0 until (blocksArr?.length() ?: 0)) {
            val block = blocksArr?.optJSONObject(j) ?: continue
            when (block.optString("kind")) {
                "text" -> block.optString("text")
                    .takeIf { it.isNotBlank() }
                    ?.let { blocks.add(Block.Prose(it)) }
                "tool" -> {
                    val res = if (block.isNull("result")) null else block.optJSONObject("result")
                    blocks.add(
                        Block.Tool(
                            ToolCall(
                                name = block.optString("name", "tool"),
                                summary = block.optString("summary"),
                                ok = res?.optBoolean("ok"),
                                preview = res?.optString("preview").orEmpty(),
                            )
                        )
                    )
                }
            }
        }
        turns.add(
            Turn(
                role = turn.optString("role", "assistant"),
                ts = turn.optString("ts"),
                text = turn.optString("text"),
                thinking = turn.optString("thinking"),
                blocks = blocks,
            )
        )
    }
    return Transcript(
        sessionId = t.optStringOrNull("session_id"),
        source = t.optString("source", "matched"),
        truncated = t.optBoolean("truncated"),
        turns = turns,
    )
}
