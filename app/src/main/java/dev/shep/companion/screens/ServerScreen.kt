package dev.shep.companion.screens

import android.content.Context
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import dev.shep.companion.FcmManager
import dev.shep.companion.NotifyKind
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.ScreenHeader
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType
import androidx.compose.material3.minimumInteractiveComponentSize

/**
 * Settings tab: what shep will notify about, and whether push works at all.
 *
 * The toggles change what the *server* sends, not just what this phone shows.
 * That way a muted kind costs no radio wake, and the choice survives a
 * reinstall. The test button exists because a broken push setup looks exactly
 * like a quiet one — there is no other way to tell them apart.
 */
@Composable
fun ServerScreen() {
    val context = LocalContext.current
    val prefs = remember { context.getSharedPreferences("shep", Context.MODE_PRIVATE) }
    var status by remember { mutableStateOf(prefs.getString("push_status", "not registered") ?: "") }
    var token by remember { mutableStateOf(prefs.getString("fcm_token", null)) }
    var kinds by remember { mutableStateOf(FcmManager.selectedKinds(context)) }
    var testResult by remember { mutableStateOf<String?>(null) }
    var testing by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        ScreenHeader("shep")
        Column(Modifier.fillMaxWidth().padding(ShepSpace.screen)) {
            Text("notify me about", style = ShepType.sectionLabel)
            Spacer(Modifier.height(ShepSpace.hair))
            Text(
                "shep stops sending what is off here, so it costs no battery.",
                style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
            )
            Spacer(Modifier.height(ShepSpace.small))
            NotifyKind.entries.forEach { kind ->
                fun toggle(on: Boolean) {
                    val next = if (on) kinds + kind else kinds - kind
                    kinds = next
                    FcmManager.setKinds(context, next) { status = it }
                }
                Row(
                    Modifier
                        .fillMaxWidth()
                        // The whole row is the target, not just the switch —
                        // and the padding is inside it, so the touch area is
                        // the height you can see rather than the label's.
                        .minimumInteractiveComponentSize()
                        .clickable { toggle(kind !in kinds) }
                        .padding(vertical = ShepSpace.small),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(kind.label, style = ShepType.itemName)
                        // What the toggle actually costs you, in prose.
                        Text(
                            kind.description,
                            style = ShepType.bodySmall.copy(color = ShepPalette.overlay0),
                        )
                    }
                    Switch(
                        checked = kind in kinds,
                        onCheckedChange = { toggle(it) },
                        colors = SwitchDefaults.colors(
                            checkedThumbColor = ShepPalette.panelBg,
                            checkedTrackColor = ShepPalette.accent,
                            uncheckedThumbColor = ShepPalette.overlay0,
                            uncheckedTrackColor = ShepPalette.surface0,
                        ),
                    )
                }
            }

            Spacer(Modifier.height(ShepSpace.section))
            Text("delivery", style = ShepType.sectionLabel)
            Spacer(Modifier.height(ShepSpace.snug))
            Text(status, style = ShepType.state.copy(color = ShepPalette.overlay0))
            Text(
                if (token != null) "registered with FCM" else "no FCM token yet",
                style = ShepType.meta.copy(
                    color = if (token != null) ShepPalette.green else ShepPalette.peach,
                ),
            )
            Spacer(Modifier.height(ShepSpace.medium))
            Row(horizontalArrangement = Arrangement.spacedBy(ShepSpace.medium)) {
                ShepButton(
                    text = if (testing) "sending…" else "send test notification",
                    onClick = {
                        testing = true
                        testResult = null
                        FcmManager.sendTest(context) {
                            testResult = it
                            testing = false
                        }
                    },
                    enabled = !testing,
                )
                ShepButton(
                    text = "re-register",
                    tone = ButtonTone.Quiet,
                    onClick = {
                        FcmManager.register(context, kinds)
                        status = "registering…"
                        token = prefs.getString("fcm_token", null)
                    },
                )
            }
            testResult?.let {
                Spacer(Modifier.height(ShepSpace.small))
                Text(
                    it,
                    style = ShepType.meta.copy(
                        color = if (it.startsWith("sent to")) ShepPalette.green else ShepPalette.peach,
                    ),
                )
            }
            Spacer(Modifier.height(ShepSpace.section))
        }
    }
}
