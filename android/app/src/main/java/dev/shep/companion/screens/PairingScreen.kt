package dev.shep.companion.screens

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.theme.ShepPalette
import dev.shep.companion.ui.theme.ShepSize
import dev.shep.companion.ui.theme.ShepSpace
import dev.shep.companion.ui.theme.ShepType

/** Parse a `shep://pair?url=…&token=…` payload from a scanned QR. */
fun parsePairingUri(raw: String): Pair<String, String>? = runCatching {
    val uri = android.net.Uri.parse(raw.trim())
    if (uri.scheme == "shep" && uri.host == "pair") {
        val u = uri.getQueryParameter("url")
        val t = uri.getQueryParameter("token")
        if (!u.isNullOrBlank() && !t.isNullOrBlank()) return@runCatching u to t
    }
    null
}.getOrNull()

/**
 * The only screen a stranger sees.
 *
 * `imePadding` matters more here than anywhere else in the app: two of the
 * three fields sit below the fold once the keyboard is up, and this screen had
 * none — the token field went under the IME the moment you tapped it.
 */
@Composable
fun PairingScreen(
    initialUrl: String,
    initialToken: String,
    lastError: String?,
    onConnect: (String, String, (String?) -> Unit) -> Unit,
) {
    var url by remember { mutableStateOf(initialUrl.ifEmpty { "ws://100.64.0.0:7431/" }) }
    var token by remember { mutableStateOf(initialToken) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf(lastError) }

    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        val contents = result.contents ?: return@rememberLauncherForActivityResult
        val parsed = parsePairingUri(contents)
        if (parsed == null) {
            error = "unrecognized QR — expected `shep://pair`"
        } else {
            url = parsed.first
            token = parsed.second
            busy = true
            error = null
            onConnect(parsed.first.trim(), parsed.second.filterNot { it.isWhitespace() }) { failure ->
                busy = false
                error = failure
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .imePadding()
            .padding(ShepSpace.section),
        verticalArrangement = Arrangement.Center,
    ) {
        Text("shep", style = ShepType.hero)
        Text(
            "the cockpit in your pocket",
            style = ShepType.meta,
            modifier = Modifier.padding(bottom = ShepSpace.section),
        )
        ShepButton(
            "Scan QR to pair",
            onClick = {
                error = null
                scanLauncher.launch(
                    ScanOptions().apply {
                        setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                        setPrompt("Scan the QR from `shep bridge pair`")
                        setBeepEnabled(false)
                        setOrientationLocked(false)
                    }
                )
            },
            enabled = !busy,
            modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
        )
        Text(
            "— or enter manually —",
            style = ShepType.meta,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth().padding(vertical = ShepSpace.screen),
        )
        OutlinedTextField(
            value = url,
            onValueChange = { url = it },
            label = { Text("Bridge URL", style = ShepType.fieldLabel) },
            textStyle = ShepType.field,
            singleLine = true,
            keyboardOptions = KeyboardOptions(
                keyboardType = KeyboardType.Uri,
                autoCorrectEnabled = false,
            ),
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(ShepSpace.medium))
        OutlinedTextField(
            value = token,
            onValueChange = { token = it },
            label = { Text("Token", style = ShepType.fieldLabel) },
            textStyle = ShepType.field,
            singleLine = true,
            keyboardOptions = KeyboardOptions(
                keyboardType = KeyboardType.Password,
                autoCorrectEnabled = false,
            ),
            // A token on this screen is one the bridge refused, or the one
            // saved from last time; either way nobody can read it, so the way
            // to replace it is a clear button, not a caret in the middle of it.
            trailingIcon = {
                if (token.isNotEmpty()) {
                    IconButton(onClick = { token = "" }) {
                        Icon(Icons.Filled.Close, contentDescription = "clear token")
                    }
                }
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.height(ShepSpace.small))
        Text(
            "Run `shep bridge pair --host <tailnet-ip>` on the server for these values.",
            style = ShepType.meta,
        )
        error?.let {
            Spacer(Modifier.height(ShepSpace.medium))
            Text(it, style = ShepType.meta.copy(color = ShepPalette.red))
        }
        Spacer(Modifier.height(ShepSpace.indent))
        ShepButton(
            if (busy) "Connecting..." else "Connect",
            onClick = {
                busy = true
                error = null
                onConnect(url.trim(), token.filterNot { it.isWhitespace() }) { failure ->
                    busy = false
                    error = failure
                }
            },
            enabled = !busy && url.isNotBlank() && token.isNotBlank(),
            modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
        )
    }
}
