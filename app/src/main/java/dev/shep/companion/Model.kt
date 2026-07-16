package dev.shep.companion

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
)

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
