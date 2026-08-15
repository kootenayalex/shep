package dev.shep.companion.terminal

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import androidx.compose.material3.minimumInteractiveComponentSize

/**
 * Keys a phone keyboard doesn't have but a terminal needs.
 *
 * Modifiers are sticky in the way terminal apps have trained people to expect:
 * tap arms for one keypress, double-tap locks until tapped off.
 *
 * Every name here is one the shep API actually accepts (`parse_key_combo` in
 * src/config/keybinds.rs) — an unknown name is rejected server-side and the
 * keypress silently does nothing.
 */
@Composable
fun KeyBar(
    onKey: (TerminalKey) -> Unit,
    modifier: Modifier = Modifier,
) {
    var ctrl by remember { mutableStateOf(ModifierState.Off) }
    var alt by remember { mutableStateOf(ModifierState.Off) }

    fun fire(name: String) {
        val prefix = buildString {
            if (ctrl != ModifierState.Off) append("ctrl+")
            if (alt != ModifierState.Off) append("alt+")
        }
        onKey(TerminalKey.Named(prefix + name))
        if (ctrl == ModifierState.Armed) ctrl = ModifierState.Off
        if (alt == ModifierState.Armed) alt = ModifierState.Off
    }

    Column(
        modifier
            .fillMaxWidth()
            .background(ShepPalette.surfaceDim)
            .padding(vertical = ShepSpace.snug),
        verticalArrangement = Arrangement.spacedBy(ShepSpace.snug),
    ) {
        // Answers to agent prompts are the highest-frequency taps, so they lead.
        Row(
            Modifier.fillMaxWidth().padding(horizontal = ShepSpace.small),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.snug),
        ) {
            Key("y", Modifier.weight(1f), ShepPalette.green) { onKey(TerminalKey.Text("y")) }
            Key("n", Modifier.weight(1f), ShepPalette.red) { onKey(TerminalKey.Text("n")) }
            Key("↵", Modifier.weight(1f)) { fire("enter") }
            Key("esc", Modifier.weight(1f)) { fire("esc") }
            Key("⇥", Modifier.weight(1f)) { fire("tab") }
        }
        Row(
            Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = ShepSpace.small),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.snug),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            StickyKey("ctrl", ctrl) {
                ctrl = ctrl.advance(it)
            }
            StickyKey("alt", alt) {
                alt = alt.advance(it)
            }
            Key("↑") { fire("up") }
            Key("↓") { fire("down") }
            Key("←") { fire("left") }
            Key("→") { fire("right") }
            Key("^C") { onKey(TerminalKey.Named("ctrl+c")) }
            Key("^D") { onKey(TerminalKey.Named("ctrl+d")) }
            Key("^L") { onKey(TerminalKey.Named("ctrl+l")) }
            Key("^R") { onKey(TerminalKey.Named("ctrl+r")) }
            Key("home") { fire("home") }
            Key("end") { fire("end") }
            Key("pgup") { fire("pageup") }
            Key("pgdn") { fire("pagedown") }
            Key("del") { fire("delete") }
        }
    }
}

enum class ModifierState { Off, Armed, Locked;
    /** Tap arms, tapping an armed modifier locks it, tapping a locked one clears. */
    fun advance(doubleTap: Boolean): ModifierState = when {
        doubleTap -> if (this == Locked) Off else Locked
        this == Off -> Armed
        this == Armed -> Off
        else -> Off
    }
}

/**
 * One key.
 *
 * Clipped before it is clickable, so the ripple follows the rounded corner
 * instead of flashing a rectangle over it, and the padding sits inside the
 * touch target rather than outside — this bar is the app's primary terminal
 * input and every key was about 30dp tall.
 */
@Composable
private fun Key(
    label: String,
    modifier: Modifier = Modifier,
    color: Color = ShepPalette.text,
    onClick: () -> Unit,
) {
    KeyFace(
        label = label,
        ink = color,
        background = ShepPalette.surface0,
        outline = ShepPalette.surface1,
        modifier = modifier,
        onClick = onClick,
    )
}

@Composable
private fun StickyKey(
    label: String,
    state: ModifierState,
    onTap: (doubleTap: Boolean) -> Unit,
) {
    val active = state != ModifierState.Off
    KeyFace(
        label = if (state == ModifierState.Locked) "$label•" else label,
        tag = label,
        ink = if (active) ShepPalette.panelBg else ShepPalette.text,
        background = if (active) ShepPalette.accent else ShepPalette.surface0,
        outline = if (active) ShepPalette.accent else ShepPalette.surface1,
        onClick = { onTap(state == ModifierState.Armed) },
    )
}

@Composable
private fun KeyFace(
    label: String,
    ink: Color,
    background: Color,
    outline: Color,
    modifier: Modifier = Modifier,
    tag: String = label,
    onClick: () -> Unit,
) {
    Box(
        modifier
            .testTag("key-$tag")
            .minimumInteractiveComponentSize()
            .clip(ShepShape.key)
            .background(background)
            .border(ShepSize.border, outline, ShepShape.key)
            .clickable(onClick = onClick)
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        contentAlignment = Alignment.Center,
    ) {
        Text(label, style = ShepType.key.copy(color = ink))
    }
}
