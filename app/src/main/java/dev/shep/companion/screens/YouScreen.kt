package dev.shep.companion.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dev.shep.companion.BridgeClient
import dev.shep.companion.ui.components.ShepChip
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSpace

/** The two things that are about you rather than about the session. */
enum class YouSection(val label: String) {
    Shep("shep"),
    Memory("memory"),
}

/**
 * Settings and memory, behind one nav entry.
 *
 * Both used to be top-level tabs, which spent two of five slots on screens
 * opened once a week. A switch costs one tap and gives the list the room it
 * needed.
 */
@Composable
fun YouScreen(client: BridgeClient) {
    var section by remember { mutableStateOf(YouSection.Shep) }
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(ShepPalette.panelBg)
                .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.tight),
            horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        ) {
            YouSection.entries.forEach { entry ->
                ShepChip(entry.label, entry == section) { section = entry }
            }
        }
        when (section) {
            YouSection.Shep -> ServerScreen()
            YouSection.Memory -> MemoryScreen(client)
        }
    }
}
