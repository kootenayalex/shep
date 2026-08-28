package dev.shep.companion.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Text
import androidx.compose.material3.minimumInteractiveComponentSize
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import dev.shep.companion.Tab
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepShape
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/** The phone translation of shep's desktop hint bar. */
@Composable
fun HintBar(selected: Tab, onSelect: (Tab) -> Unit) {
    Row(
        Modifier
            .background(ShepPalette.surfaceDim)
            .horizontalScroll(rememberScrollState())
            .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.small),
        horizontalArrangement = Arrangement.spacedBy(ShepSpace.small),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Tab.entries.forEach { tab ->
            Row(
                Modifier
                    .minimumInteractiveComponentSize()
                    .clip(ShepShape.button)
                    .background(if (tab == selected) ShepPalette.accentDim else ShepPalette.surface0)
                    .clickable { onSelect(tab) }
                    .padding(horizontal = ShepSpace.medium, vertical = ShepSpace.snug),
                horizontalArrangement = Arrangement.spacedBy(ShepSpace.snug),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(tab.shortcut, style = ShepType.key.copy(color = ShepPalette.accent))
                Text(
                    tab.label,
                    style = ShepType.hint.copy(
                        color = if (tab == selected) ShepPalette.text else ShepPalette.subtext0,
                    ),
                )
            }
        }
    }
}
