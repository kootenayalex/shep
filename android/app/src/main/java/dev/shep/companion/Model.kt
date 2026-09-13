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
    /**
     * Commits this group's branch is ahead of / behind its upstream, from
     * `session.overview` (`git_ahead` / `git_behind` on the agent). Null
     * without an upstream, and null against a server too old to send them —
     * the board's `↑N not pushed` hint is simply absent rather than a zero.
     */
    val gitAhead: Int? = null,
    val gitBehind: Int? = null,
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
                gitAhead = a.optIntOrNull("git_ahead"),
                gitBehind = a.optIntOrNull("git_behind"),
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

/**
 * An age the server stated [sinceMs] milliseconds ago, carried forward on the
 * local clock.
 *
 * The server answers "this agent has been idle 4m" as of the moment it answered.
 * An idle agent then sends no events, so nothing rebuilds the row and the number
 * sits at 4m for as long as the screen is open — which is why the counter only
 * appeared to move when you left the board and came back.
 */
fun ageCarriedForward(stateAgeSeconds: Long?, sinceMs: Long): Long? =
    stateAgeSeconds?.let { it + (sinceMs.coerceAtLeast(0L) / 1000L) }

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
 * first ten. This is the same facts — state, and what it is chewing on — said
 * as a sentence, above the raw lines rather than instead of them.
 *
 * How long it has been that way is deliberately NOT in here. It rides beside
 * this line instead, so it can sit in a column under the state word rather than
 * trailing off the end of a sentence that ellipsises before you reach it.
 *
 * Two things it must never render, because Maestro anchors on both elsewhere in
 * the hierarchy: a bare `live` (flow 07 taps the first of those by index) and
 * anything full-matching `\S+ blocked` (flow 13). Without an age to suffix, the
 * `live` guard below is the only thing standing between a state named `live`
 * and that anchor.
 */
fun nowLine(
    status: String,
    manualLabel: String?,
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
    // Maestro matches the *whole* text of an element, and flow 07 taps the
    // first one that reads exactly `live` — the out toggle. This line renders
    // above it, so a state literally called `live` would sit in front of the
    // toggle and take the tap.
    return if (head == "live") "running" else head
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
// Todos — the checklist the agent is working through right now
// --------------------------------------------------------------------------

/**
 * One item on the agent's own checklist.
 *
 * [activeForm] is how the agent phrases the work while it is happening
 * ("Writing the v3 authority docs") as opposed to [subject], which is how it
 * names the item ("Build v3 scaffolding"). Showing the active form for the
 * in-progress item is what makes the list read as live.
 */
data class TodoItem(
    val id: String,
    val subject: String,
    val activeForm: String,
    val description: String,
    val status: String,
    val blockedBy: List<String>,
)

/**
 * The checklist for one pane.
 *
 * [source] is where it came from: `store` (the harness's own task files) or
 * `transcript` (folded back out of the session, because the store had been
 * emptied). Both are the agent's real state; the difference only matters when
 * something looks stale.
 */
data class Todos(
    val sessionId: String?,
    val source: String,
    val items: List<TodoItem>,
)

fun todoIsOpen(status: String): Boolean = status != "completed" && status != "cancelled"

fun parseTodos(result: JSONObject): Todos? {
    val root = result.optJSONObject("todos") ?: return null
    val arr = root.optJSONArray("items")
    val items = mutableListOf<TodoItem>()
    for (i in 0 until (arr?.length() ?: 0)) {
        val item = arr?.optJSONObject(i) ?: continue
        val blockedArr = item.optJSONArray("blockedBy")
        val blockedBy = mutableListOf<String>()
        for (j in 0 until (blockedArr?.length() ?: 0)) {
            blockedArr?.optString(j)?.takeIf { it.isNotBlank() }?.let { blockedBy.add(it) }
        }
        items.add(
            TodoItem(
                id = item.optString("id"),
                subject = item.optString("subject"),
                activeForm = item.optString("activeForm"),
                description = item.optString("description"),
                status = item.optString("status", "pending"),
                blockedBy = blockedBy,
            )
        )
    }
    return Todos(
        sessionId = root.optStringOrNull("session_id"),
        source = root.optString("source", "transcript"),
        items = items,
    )
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

// ---------------------------------------------------------------------------
// Docket — the personal assistant's list, served by `docket.*`.
// ---------------------------------------------------------------------------

/** Where a docket item came from and how it is handled. Wire: `DocketKind` in src/api/schema/docket.rs. */
enum class DocketKind(val wire: String) {
    Captured("captured"),
    Slated("slated"),
    Recurring("recurring");

    companion object {
        fun parse(raw: String?): DocketKind = entries.firstOrNull { it.wire == raw } ?: Captured
    }
}

/** Lifecycle of a docket item. Wire: `DocketStatus` in src/api/schema/docket.rs. */
enum class DocketStatus(val wire: String) {
    Inbox("inbox"),
    Open("open"),
    Done("done"),
    Discarded("discarded");

    companion object {
        fun parse(raw: String?): DocketStatus = entries.firstOrNull { it.wire == raw } ?: Inbox
    }
}

/** The repeat intervals the server knows (`DocketRepeat`), in the order the chips show them. */
val DOCKET_REPEATS = listOf("1d", "1w", "2w", "1m")

/**
 * One docket row.
 *
 * [sourceLabel] is computed once here — `memory.md:12` for a file, `pane p3`
 * for a session — the way the desktop's `source_tags` does, so the card does
 * not carry path logic. [overdue] is the server's verdict (`due < today` on an
 * open item), and [dueToday] is derived against the list's own `today` so the
 * phone's clock never disagrees with the store's.
 */
data class DocketItem(
    val id: Long,
    val title: String,
    val kind: DocketKind,
    val status: DocketStatus,
    val due: String?,
    val repeat: String?,
    val sourceLabel: String?,
    /**
     * The raw `source.kind`, unlabelled. `proposals` filters on it: an inbox
     * item the overseer captured says `situation`, and that is what tells a
     * proposal apart from something you wrote down yourself.
     */
    val sourceKind: String?,
    val notes: String?,
    val overdue: Boolean,
    val updated: String,
    val dueToday: Boolean,
)

/** `docket.list`: the store's `today` (`YYYY-MM-DD`) and every item it returned. */
data class Docket(
    val today: String,
    val items: List<DocketItem>,
)

/**
 * `{"file": "/a/b/memory.md", "line": 12}` -> `memory.md:12`;
 * `{"pane": "p3", …}` -> `pane p3`; the overseer's `{"kind": "situation",
 * "ref": "SHEP"}` -> `situation SHEP`. Anything else is not worth a row.
 */
fun docketSourceLabel(source: JSONObject?): String? {
    source ?: return null
    source.optStringOrNull("file")?.let { file ->
        val base = file.trimEnd('/').substringAfterLast('/').ifEmpty { file }
        val line = source.optIntOrNull("line")
        return if (line != null) "$base:$line" else base
    }
    source.optStringOrNull("pane")?.let { return "pane $it" }
    val ref = source.optStringOrNull("ref") ?: return null
    val kind = source.optStringOrNull("kind")
    return if (kind != null) "$kind $ref" else ref
}

/** One item as `docket.list` and every mutation (`{item}`) return it. */
fun parseDocketItem(o: JSONObject, today: String): DocketItem {
    val due = o.optStringOrNull("due")
    val status = DocketStatus.parse(o.optStringOrNull("status"))
    return DocketItem(
        id = o.optLong("id"),
        title = o.optStringOrNull("title") ?: "",
        kind = DocketKind.parse(o.optStringOrNull("kind")),
        status = status,
        due = due,
        repeat = o.optStringOrNull("repeat"),
        sourceLabel = docketSourceLabel(o.optJSONObject("source")),
        sourceKind = o.optJSONObject("source")?.optStringOrNull("kind"),
        notes = o.optStringOrNull("notes"),
        overdue = o.optBoolean("overdue", false),
        updated = o.optStringOrNull("updated") ?: "",
        dueToday = status == DocketStatus.Open && due != null && due == today,
    )
}

fun parseDocket(result: JSONObject): Docket {
    val today = result.optStringOrNull("today") ?: ""
    val arr = result.optJSONArray("items") ?: JSONArray()
    val items = (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }
        .map { parseDocketItem(it, today) }
    return Docket(today = today, items = items)
}

/**
 * `overdue 3d` / `due today` / `in 5d` / `—`, matching the desktop's `DueLabel`.
 * Days are counted on the calendar, not the clock: both strings are
 * `YYYY-MM-DD`, so the arithmetic is on local dates.
 */
fun docketDueLabel(due: String?, today: String): String {
    if (due == null) return "—"
    val days = runCatching {
        java.time.temporal.ChronoUnit.DAYS.between(
            java.time.LocalDate.parse(today),
            java.time.LocalDate.parse(due),
        )
    }.getOrNull() ?: return due
    return when {
        days < 0 -> "overdue ${-days}d"
        days == 0L -> "due today"
        else -> "in ${days}d"
    }
}

/** `YYYY-MM-DD` and a real calendar date; the light validation the edit sheet does. */
fun isDocketDate(text: String): Boolean =
    Regex("""\d{4}-\d{2}-\d{2}""").matches(text) &&
        runCatching { java.time.LocalDate.parse(text) }.isSuccess

/** The docket's lanes in the desktop board's reading order (`DocketLane` in src/ui/board.rs). */
enum class DocketLane(val title: String) {
    Inbox("inbox"),
    Due("due"),
    Slated("slated"),
    Recurring("recurring"),
    Done("done"),
}

/** How many finished items the done lane keeps; the desktop's `DONE_LANE_LIMIT`. */
const val DOCKET_DONE_LIMIT = 10

/**
 * Bucket items into lanes exactly as the desktop's `docket_board_model` does:
 * inbox; open items due today or earlier (overdue first, as the store orders
 * them); open one-offs dated later or undated; open recurring items not yet
 * due; the newest ten done items. Discarded items are not on the board at all,
 * and an empty lane keeps its place.
 */
fun docketLanes(docket: Docket): List<Pair<DocketLane, List<DocketItem>>> {
    val lanes = DocketLane.entries.associateWith { mutableListOf<DocketItem>() }
    for (item in docket.items) {
        val lane = when (item.status) {
            DocketStatus.Inbox -> DocketLane.Inbox
            DocketStatus.Open -> when {
                item.overdue || item.dueToday -> DocketLane.Due
                item.kind == DocketKind.Recurring -> DocketLane.Recurring
                else -> DocketLane.Slated
            }
            DocketStatus.Done -> DocketLane.Done
            DocketStatus.Discarded -> continue
        }
        lanes.getValue(lane).add(item)
    }
    val due = lanes.getValue(DocketLane.Due)
    due.sortWith(compareByDescending<DocketItem> { it.overdue }.thenBy { it.due ?: "" })
    val done = lanes.getValue(DocketLane.Done)
    val newestDone = done.sortedWith(compareByDescending<DocketItem> { it.updated }.thenByDescending { it.id })
        .take(DOCKET_DONE_LIMIT)
    return DocketLane.entries.map { lane ->
        lane to if (lane == DocketLane.Done) newestDone else lanes.getValue(lane).toList()
    }
}

// ---------------------------------------------------------------------------
// Overseer — what the plugin last sensed and said, served by `overseer.*`.
// ---------------------------------------------------------------------------

/**
 * `shep doctor`'s verdict on one check, as the overseer copied it into its
 * situation. Wire: `OverseerHealthLevel` in src/api/schema/overseer.rs.
 */
enum class HealthLevel(val wire: String) {
    Ok("ok"),
    Warn("warn"),
    Fail("fail");

    companion object {
        fun parse(raw: String?): HealthLevel = entries.firstOrNull { it.wire == raw } ?: Ok
    }
}

/** One health check and what it found. Wire: `OverseerHealthFinding`. */
data class HealthFinding(
    val level: HealthLevel,
    val check: String,
    val detail: String,
    /** What would fix it, when the check knows. The board shows it on a long press. */
    val fix: String?,
)

/** Who said a chat line. Wire: `OverseerChatRole` in src/api/schema/overseer.rs. */
enum class ChatRole(val wire: String) {
    You("you"),
    Overseer("overseer");

    companion object {
        fun parse(raw: String?): ChatRole = entries.firstOrNull { it.wire == raw } ?: Overseer
    }
}

/**
 * One line of the chat with the overseer — one line of the plugin's
 * `chat.jsonl`. Wire: `OverseerChatTurn`.
 *
 * [at] is unix seconds and is half of what identifies a turn: the same turn
 * reaches the board twice, once as an `overseer.chat_turn` event and once in
 * the next sample's tail, and `(at, role)` is what tells those two copies
 * apart from two real turns.
 */
data class ChatTurn(val at: Long, val role: ChatRole, val text: String)

/**
 * What the overseer knows right now. Wire: `OverseerSample` in
 * src/api/schema/overseer.rs, built by `App::overseer_sample_info`
 * (src/app/overseer.rs).
 *
 * Every field is something the server read off the plugin's state dir, so the
 * phone's board and the desktop's board are looking at the same files rather
 * than at two guesses about them.
 */
data class OverseerSample(
    val pluginLinked: Boolean,
    val sampled: Boolean,
    /** `BOARD.md`, header dropped, one entry per non-empty line. */
    val narrative: List<String>,
    /** `brain` or `deterministic` — who wrote the narrative. */
    val source: String,
    /** `hh:mm` of the last tick, when the situation says. */
    val tickAt: String?,
    val situationAgeSeconds: Long?,
    val brainAgeSeconds: Long?,
    /** The headless runtime the chat asks. Null means the chat cannot ask anything. */
    val runtime: String?,
    val tickInFlight: Boolean,
    val health: List<HealthFinding>,
    /** The tail of the chat, oldest first. */
    val chat: List<ChatTurn>,
    val chatTotal: Long,
    val chatPending: Boolean,
    val sessionId: String?,
    val sessionStarted: Boolean,
)

/** One `chat.jsonl` line as the sample and the `overseer.chat_turn` event send it. */
fun parseChatTurn(obj: JSONObject): ChatTurn = ChatTurn(
    at = obj.optLong("at"),
    role = ChatRole.parse(obj.optStringOrNull("role")),
    text = obj.optString("text"),
)

/** One health finding as `overseer.sample` sends it. */
private fun parseHealthFinding(obj: JSONObject): HealthFinding = HealthFinding(
    level = HealthLevel.parse(obj.optStringOrNull("level")),
    check = obj.optString("check"),
    detail = obj.optString("detail"),
    fix = obj.optStringOrNull("fix"),
)

/**
 * Parse an `overseer.sample` result (`{sample: {…}}`).
 *
 * A payload that is not one — or nothing at all — parses to the same shape
 * with `sampled` false, so the board renders its agent-side regions and says
 * out loud that the overseer is missing rather than showing a screen of blanks.
 */
fun parseOverseerSample(result: JSONObject): OverseerSample {
    val s = result.optJSONObject("sample") ?: JSONObject()
    val healthArr = s.optJSONArray("health") ?: JSONArray()
    val chatArr = s.optJSONArray("chat") ?: JSONArray()
    val session = s.optJSONObject("session") ?: JSONObject()
    return OverseerSample(
        pluginLinked = s.optBoolean("plugin_linked", false),
        sampled = s.optBoolean("sampled", false),
        narrative = s.optStringList("narrative"),
        source = s.optStringOrNull("source") ?: "deterministic",
        tickAt = s.optStringOrNull("tick_at"),
        situationAgeSeconds = s.optLongOrNull("situation_age_seconds"),
        brainAgeSeconds = s.optLongOrNull("brain_age_seconds"),
        runtime = s.optStringOrNull("runtime"),
        tickInFlight = s.optBoolean("tick_in_flight", false),
        health = (0 until healthArr.length()).mapNotNull { healthArr.optJSONObject(it) }
            .map(::parseHealthFinding),
        chat = (0 until chatArr.length()).mapNotNull { chatArr.optJSONObject(it) }
            .map(::parseChatTurn),
        chatTotal = s.optLong("chat_total"),
        chatPending = s.optBoolean("chat_pending", false),
        sessionId = session.optStringOrNull("id"),
        sessionStarted = session.optBoolean("started", false),
    )
}

/**
 * The narrative's opening sentence — what the agents strip has room for.
 *
 * The desktop's `OverseerSample::first_sentence` (src/app/overseer.rs:242) over
 * the same lines: skip a header (`# BOARD — …` from the tick's template, or
 * `OVERSEER · …`), take the first line left, and stop at the first sentence
 * end. The wire already drops the header for `narrative`, but a board written
 * by an older tick still carries one, and a strip that opens with
 * `# BOARD — 07:08` says nothing at all.
 */
fun firstSentence(narrative: List<String>): String? {
    val lines = narrative.map { it.trim() }.filter { it.isNotEmpty() }
    val first = lines.firstOrNull() ?: return null
    val index = if (first.startsWith("OVERSEER ·") || first.startsWith("# ")) 1 else 0
    val line = lines.getOrNull(index) ?: return null
    val end = listOf(". ", "! ", "? ")
        .mapNotNull { mark -> line.indexOf(mark).takeIf { it >= 0 } }
        .minOrNull()
        ?.plus(1)
        ?: line.length
    return line.take(end).trim().takeIf { it.isNotEmpty() }
}

/** How many proposals the board shows at once; the desktop's `MAX_PROPOSALS`. */
const val MAX_PROPOSALS = 5

/**
 * The proposals waiting on the board: inbox items the overseer captured
 * (`source.kind == "situation"`), newest first, at most [MAX_PROPOSALS].
 *
 * The desktop's `app::overseer::proposals` (src/app/overseer.rs:1113) over the
 * same rows. Anything you wrote down yourself stays in the docket region: the
 * point of the split is that these are somebody else's suggestions and you
 * have not agreed to them yet.
 */
fun proposals(docket: Docket): List<DocketItem> = docket.items
    .filter { it.status == DocketStatus.Inbox && it.sourceKind == "situation" }
    .sortedWith(compareByDescending<DocketItem> { it.updated }.thenByDescending { it.id })
    .take(MAX_PROPOSALS)

/**
 * The docket region of the board: the due lane whole, then the inbox items not
 * already showing as proposals, with the heading's three counts.
 *
 * Mirrors `overseer_model` in src/ui/overseer.rs:218-239 — same lanes, same
 * subtraction, same counts — so the heading on the phone reads what the
 * heading at the desk reads.
 */
data class BoardDocket(
    val rows: List<DocketItem>,
    val due: Int,
    val overdue: Int,
    val inbox: Int,
)

fun boardDocket(docket: Docket, proposals: List<DocketItem>): BoardDocket {
    val lanes = docketLanes(docket).toMap()
    val due = lanes[DocketLane.Due].orEmpty()
    val inbox = lanes[DocketLane.Inbox].orEmpty()
    val proposed = proposals.map { it.id }.toSet()
    return BoardDocket(
        rows = due + inbox.filterNot { it.id in proposed },
        due = due.size,
        overdue = due.count { it.overdue },
        inbox = inbox.size,
    )
}

/**
 * One row of the board's `needs you` region: why this agent is waiting on a
 * human, and what its second line says about it.
 *
 * [detail] is the desktop's `said` — the last thing the agent's screen showed,
 * else what it says it is doing. [ahead] is how many commits a finished
 * agent's branch has that its upstream does not, which is the one thing left
 * to do about an agent that is otherwise done.
 */
data class NeedsYouRow(
    val row: AgentRow,
    val blocked: Boolean,
    val detail: String,
    val ahead: Int?,
)

/**
 * Who is waiting on you, in the desktop's order: blocked first, then finished
 * and not yet looked at, board order holding inside each
 * (`overseer_model`, src/ui/overseer.rs:189-216).
 *
 * `done` is already the server's word for "finished and unseen" — the seen
 * split happens server-side — so this needs no second opinion about it.
 */
fun needsYou(rows: List<AgentRow>): List<NeedsYouRow> {
    fun said(row: AgentRow): String =
        row.activityLine?.let { trimActivity(it) }?.takeIf { it.isNotEmpty() }
            ?: row.customStatus
            ?: row.status
    val blocked = rows.filter { it.status == "blocked" }
        .map { NeedsYouRow(it, blocked = true, detail = said(it), ahead = null) }
    val done = rows.filter { it.status == "done" }
        .map { NeedsYouRow(it, blocked = false, detail = said(it), ahead = it.gitAhead) }
    return blocked + done
}

/**
 * Whether a failed call means "this server does not have that method".
 *
 * Worth being strict about: latching a fallback on *any* error means one
 * dropped packet permanently downgrades the connection to the thinner method,
 * and nothing short of a restart puts it back. shep rejects an unknown method
 * while deserialising the request, so the message says so.
 *
 * Lives here rather than beside one screen because two of them now ask it —
 * the agents list about `session.overview`, the board about `overseer.sample`.
 */
fun looksUnsupported(message: String?): Boolean {
    val text = message?.lowercase() ?: return false
    return "unknown variant" in text || "unknown method" in text || "unsupported" in text
}
