package dev.shep.companion.screens

/** One file in a change, as `git diff --stat` describes it. */
data class FileStat(
    val path: String,
    val added: Int,
    val removed: Int,
    val binary: Boolean = false,
)

/** A whole change: the summary line's totals, plus the rows above it. */
data class DiffStat(
    val files: Int,
    val added: Int,
    val removed: Int,
    val perFile: List<FileStat>,
)

/**
 * Read `git diff --stat` back into numbers.
 *
 * The server sends the stat as text because that is what git prints, and the
 * companion showed it as text: a monospace block of pipes and plus signs that
 * answers "how big is this" only if you already know how to read one. Parsing
 * it here rather than adding an API field keeps this to a client change — and
 * a stat this cannot read is not a failure, it falls back to the raw block.
 */
fun parseDiffStat(stat: String): DiffStat? {
    val perFile = mutableListOf<FileStat>()
    var files: Int? = null
    var added = 0
    var removed = 0
    for (raw in stat.lineSequence()) {
        val line = raw.trim()
        if (line.isEmpty()) continue
        val summary = SUMMARY.find(line)
        if (summary != null) {
            files = summary.groupValues[1].toIntOrNull() ?: continue
            added = INSERTIONS.find(line)?.groupValues?.get(1)?.toIntOrNull() ?: 0
            removed = DELETIONS.find(line)?.groupValues?.get(1)?.toIntOrNull() ?: 0
            continue
        }
        val bar = line.indexOf('|')
        if (bar <= 0) continue
        val path = line.take(bar).trim()
        val counts = line.substring(bar + 1).trim()
        if (path.isEmpty()) continue
        if (counts.startsWith("Bin")) {
            perFile += FileStat(path, added = 0, removed = 0, binary = true)
            continue
        }
        // `12 +++----`: the number is the total, the marks are the split. Both
        // are approximations once git scales the bar to the terminal width, so
        // the marks decide the ratio and the number decides the size.
        val total = counts.takeWhile { it.isDigit() }.toIntOrNull() ?: continue
        val plus = counts.count { it == '+' }
        val minus = counts.count { it == '-' }
        val marks = plus + minus
        val fileAdded = if (marks == 0) total else Math.round(total * plus.toFloat() / marks)
        perFile += FileStat(path, added = fileAdded, removed = total - fileAdded)
    }
    val count = files ?: return null
    return DiffStat(files = count, added = added, removed = removed, perFile = perFile)
}

private val SUMMARY = Regex("""(\d+) files? changed""")
private val INSERTIONS = Regex("""(\d+) insertions?\(\+\)""")
private val DELETIONS = Regex("""(\d+) deletions?\(-\)""")

/**
 * Cut a unified diff into one hunk block per file, keyed by its b-side path,
 * so a file row can show its own code and nothing else.
 *
 * A diff with no `diff --git` header at all — a stat-only response, or a
 * server that formats differently — comes back as one entry under `""`, which
 * the caller shows whole rather than dropping.
 */
fun splitDiffByFile(diff: String): Map<String, String> {
    if (diff.isEmpty()) return emptyMap()
    val out = LinkedHashMap<String, StringBuilder>()
    var current = out.getOrPut("") { StringBuilder() }
    for (line in diff.lineSequence()) {
        val header = HEADER.find(line)
        if (header != null) {
            current = out.getOrPut(header.groupValues[2]) { StringBuilder() }
            continue
        }
        current.append(line).append('\n')
    }
    if (out[""]?.isBlank() == true && out.size > 1) out.remove("")
    return out.mapValues { it.value.toString().trimEnd('\n') }
}

private val HEADER = Regex("""^diff --git a/(\S+) b/(\S+)""")

/** The one line at the top of review: who wrote this, and how big it is. */
fun reviewSummary(agent: String, stat: DiffStat): String {
    val files = if (stat.files == 1) "1 file" else "${stat.files} files"
    val added = if (stat.added == 1) "1 line added" else "${stat.added} lines added"
    return "written by $agent · $files · $added, ${stat.removed} removed"
}
