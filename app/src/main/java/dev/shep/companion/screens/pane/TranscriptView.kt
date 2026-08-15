package dev.shep.companion.screens.pane

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.withStyle
import dev.shep.companion.Block
import dev.shep.companion.ToolCall
import dev.shep.companion.Transcript
import dev.shep.companion.Turn
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import androidx.compose.material3.minimumInteractiveComponentSize

/**
 * A pane's conversation, read like a chat.
 *
 * The live view answers "what is on the screen right now"; this answers "what
 * has this agent and I actually said to each other", which is the question you
 * have when you pick the phone up after an hour away. It reads the agent's own
 * session log rather than the terminal, so it is not affected by scrollback,
 * `/clear`, or the TUI redrawing itself.
 */
@Composable
fun TranscriptView(
    transcript: Transcript?,
    error: String?,
    loading: Boolean,
    onRawScrollback: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val listState = rememberLazyListState()
    val turns = transcript?.turns.orEmpty()

    // Land at the newest turn, and stay there as new ones arrive — the same
    // contract every chat client has.
    LaunchedEffect(turns.size) {
        if (turns.isNotEmpty()) listState.scrollToItem(turns.size - 1)
    }

    Column(modifier) {
        if (error != null) {
            TranscriptNotice(error, ShepPalette.red, ShepPalette.redDim)
            Text(
                "open raw scrollback instead",
                style = ShepType.hint.copy(color = ShepPalette.accent),
                modifier = Modifier
                    .testTag("transcript-fallback")
                    .clickable { onRawScrollback() }
                    .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
            )
        } else if (transcript?.source == "matched") {
            // Honest about the guess: nothing told us which session this pane is
            // running, so it was fingerprinted against what is on the screen.
            TranscriptNotice(
                "matched by output — `shep integration install claude` for an exact match",
                ShepPalette.peach,
                ShepPalette.peachDim,
            )
        }

        if (turns.isEmpty()) {
            Box(Modifier.fillMaxWidth().padding(ShepSpace.section), contentAlignment = Alignment.Center) {
                Text(
                    if (loading) "reading the transcript…" else "nothing recorded yet",
                    style = ShepType.hint.copy(color = ShepPalette.overlay0),
                )
            }
            return@Column
        }

        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxWidth().testTag("transcript"),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(
                horizontal = ShepSpace.medium,
                vertical = ShepSpace.small,
            ),
            verticalArrangement = Arrangement.spacedBy(ShepSpace.medium),
        ) {
            if (transcript?.truncated == true) {
                item {
                    Text(
                        "… earlier turns not shown",
                        style = ShepType.hint.copy(color = ShepPalette.overlay0),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
            itemsIndexed(turns) { index, turn ->
                when (turn.role) {
                    "user" -> UserTurn(turn)
                    "system" -> SystemTurn(turn)
                    else -> AssistantTurn(turn, index)
                }
            }
        }
    }
}

@Composable
private fun TranscriptNotice(message: String, ink: androidx.compose.ui.graphics.Color, bg: androidx.compose.ui.graphics.Color) {
    Text(
        message,
        style = ShepType.hint.copy(color = ink),
        modifier = Modifier
            .testTag("transcript-notice")
            .fillMaxWidth()
            .background(bg)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
    )
}

/** What Alex said: a bubble, right-aligned, the way every chat client does it. */
@Composable
private fun UserTurn(turn: Turn) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        Box(
            Modifier
                .fillMaxWidth(0.88f)
                .background(ShepPalette.accentDim, ShepShape.pill)
                .border(ShepSize.border, ShepPalette.accent.copy(alpha = 0.35f), ShepShape.pill)
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        ) {
            Text(
                turn.text,
                style = ShepType.body,
            )
        }
    }
}

/** Not a message — something the harness did. Rendered so it can't be mistaken. */
@Composable
private fun SystemTurn(turn: Turn) {
    Text(
        "· ${turn.text}",
        style = ShepType.hint.copy(color = ShepPalette.overlay0),
        modifier = Modifier.fillMaxWidth().padding(horizontal = ShepSpace.tight),
    )
}

@Composable
private fun AssistantTurn(turn: Turn, index: Int) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(ShepSpace.snug)) {
        if (turn.thinking.isNotBlank()) {
            Expandable(
                label = "thinking",
                labelColor = ShepPalette.mauve,
                body = turn.thinking,
                tag = "thinking-$index",
            )
        }
        // In source order, so a tool call sits between the sentence that
        // announced it and the one that reports what it found.
        turn.blocks.forEachIndexed { blockIndex, block ->
            when (block) {
                is Block.Prose -> Text(
                    renderMarkdown(block.text),
                    style = ShepType.body,
                )
                is Block.Tool -> ToolRow(block.call, "tool-$index-$blockIndex")
            }
        }
    }
}

/**
 * One tool call, collapsed to a line.
 *
 * The name and the telling argument are what you scan; the output is behind a
 * tap because on a phone a 600-character result buries the reply it belongs to.
 */
@Composable
private fun ToolRow(tool: ToolCall, tag: String) {
    var open by remember { mutableStateOf(false) }
    val mark = when (tool.ok) {
        true -> "✓"
        false -> "✗"
        null -> "…"
    }
    val markColor = when (tool.ok) {
        true -> ShepPalette.green
        false -> ShepPalette.red
        null -> ShepPalette.overlay0
    }
    Column(
        Modifier
            .fillMaxWidth()
            .minimumInteractiveComponentSize()
            .clip(ShepShape.button)
            .background(ShepPalette.surfaceDim)
            .clickable(enabled = tool.preview.isNotBlank()) { open = !open }
            .padding(ShepSpace.small),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.snug),
    ) {
        Row(
            Modifier.fillMaxWidth().testTag(tag),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                if (tool.preview.isBlank()) "▪" else if (open) "▾" else "▸",
                style = ShepType.badge.copy(color = ShepPalette.overlay0),
            )
            Text(
                tool.name,
                style = ShepType.badge.copy(color = ShepPalette.teal),
            )
            Text(
                tool.summary,
                style = ShepType.paneId,
                maxLines = 1,
                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            Text(mark, style = ShepType.badge.copy(color = markColor))
        }
        if (open && tool.preview.isNotBlank()) {
            Text(
                tool.preview,
                style = ShepType.codeSmall.copy(color = ShepPalette.subtext0),
            )
        }
    }
}

@Composable
private fun Expandable(label: String, labelColor: androidx.compose.ui.graphics.Color, body: String, tag: String) {
    var open by remember { mutableStateOf(false) }
    Column(
        Modifier.fillMaxWidth().clickable { open = !open }.testTag(tag),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.tight),
    ) {
        Text(
            "${if (open) "▾" else "▸"} $label",
            style = ShepType.badge.copy(color = labelColor),
        )
        if (open) {
            Text(
                body,
                style = ShepType.bodySmall,
            )
        }
    }
}

/**
 * Just enough Markdown for agent prose.
 *
 * Bold, inline code and bullets are what Claude actually emits in a reply; a
 * full Markdown renderer would be a dependency and a layout engine for three
 * inline styles. Fenced blocks are given the terminal's own typeface so code
 * still reads as code.
 */
internal fun renderMarkdown(source: String): AnnotatedString = buildAnnotatedString {
    var fenced = false
    val lines = source.split("\n")
    lines.forEachIndexed { index, raw ->
        if (raw.trimStart().startsWith("```")) {
            fenced = !fenced
            return@forEachIndexed
        }
        if (fenced) {
            withStyle(ShepType.code.toSpanStyle()) {
                append(raw)
            }
        } else {
            val bulleted = raw.replaceFirst(Regex("^(\\s*)[-*] "), "$1• ")
            appendInline(bulleted)
        }
        if (index != lines.lastIndex) append("\n")
    }
}

/** `**bold**`, `` `code` `` and `## headings` within one line. */
private fun androidx.compose.ui.text.AnnotatedString.Builder.appendInline(line: String) {
    val heading = Regex("^#{1,6} ").find(line)
    if (heading != null) {
        withStyle(SpanStyle(fontWeight = FontWeight.Bold, color = ShepPalette.accent)) {
            append(line.substring(heading.value.length))
        }
        return
    }
    var index = 0
    val pattern = Regex("\\*\\*(.+?)\\*\\*|`([^`]+)`")
    for (match in pattern.findAll(line)) {
        append(line.substring(index, match.range.first))
        val bold = match.groupValues[1]
        if (bold.isNotEmpty()) {
            withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(bold) }
        } else {
            withStyle(
                ShepType.code.toSpanStyle().copy(color = ShepPalette.teal),
            ) { append(match.groupValues[2]) }
        }
        index = match.range.last + 1
    }
    append(line.substring(index))
}
