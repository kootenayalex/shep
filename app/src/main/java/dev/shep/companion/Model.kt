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
    val activityLine: String? = null,
    val cwd: String? = null,
    val stateAgeSeconds: Long? = null,
    val queuedInput: Int = 0,
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

/**
 * Increasingly-specific names for one agent, shortest first.
 *
 * The first entry is what shep itself calls the agent — a custom name once it
 * has been renamed, otherwise the runtime ("claude", "shell"). Each further
 * entry pins it down by one more fact. [distinctNames] picks the shortest one
 * that is not shared with another agent on the board.
 */
fun nameCandidates(row: AgentRow): List<String> {
    val candidates = mutableListOf(row.agent)
    var accumulated = row.agent
    listOfNotNull(
        row.workspaceLabel.takeIf { it.isNotBlank() },
        row.branch,
        row.tabName?.takeIf { it.isNotBlank() },
        row.paneNumber?.let { "p$it" },
    ).forEach {
        accumulated = "$accumulated · $it"
        candidates.add(accumulated)
    }
    return candidates
}

/**
 * A name per agent that no other agent on the board answers to, keyed by pane id.
 *
 * Five Claude sessions in the same repo are all "claude · shep · master" —
 * true, and useless. This spends detail only where it buys a distinction: an
 * agent that is already the only "claude" stays "claude", and only the ones
 * that collide grow a workspace, a branch, a tab, a pane number. The pane id is
 * the last resort precisely because `w2:p1` is what we are trying to avoid
 * showing; it appears only when nothing else separates two agents.
 */
fun distinctNames(rows: List<AgentRow>): Map<String, String> {
    val candidates = rows.associate { it.paneId to nameCandidates(it) }
    val resolved = mutableMapOf<String, String>()
    val depth = candidates.values.maxOfOrNull { it.size } ?: 0
    for (level in 0 until depth) {
        val pending = rows.filter { it.paneId !in resolved }.associate { row ->
            val options = candidates.getValue(row.paneId)
            row.paneId to (options.getOrNull(level) ?: options.last())
        }
        val counts = pending.values.groupingBy { it }.eachCount()
        pending.forEach { (paneId, name) -> if (counts[name] == 1) resolved[paneId] = name }
    }
    rows.forEach { row ->
        if (row.paneId !in resolved) {
            resolved[row.paneId] = "${candidates.getValue(row.paneId).last()} · ${row.paneId}"
        }
    }
    return resolved
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

/** The whole board in one payload. */
data class SessionOverview(
    val totals: SessionTotals,
    val host: SessionHost,
    val agents: List<AgentRow>,
)

private fun JSONObject.optIntOrNull(key: String): Int? =
    if (has(key) && !isNull(key)) optInt(key) else null

private fun JSONObject.optLongOrNull(key: String): Long? =
    if (has(key) && !isNull(key)) optLong(key) else null

private fun JSONObject.optStringOrNull(key: String): String? =
    if (has(key) && !isNull(key)) optString(key).takeIf { it.isNotEmpty() } else null

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
                activityLine = a.optStringOrNull("activity_line"),
                cwd = a.optStringOrNull("cwd"),
                stateAgeSeconds = a.optLongOrNull("state_age_seconds"),
                queuedInput = a.optInt("queued_input", 0),
            )
        )
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
