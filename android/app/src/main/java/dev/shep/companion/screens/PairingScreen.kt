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
import dev.shep.companion.CLAIM_CODE_LENGTH
import dev.shep.companion.Tab
import dev.shep.companion.isCompleteClaimCode
import dev.shep.companion.normalizeClaimCode
import dev.shep.companion.net.hostOf
import dev.shep.companion.ui.components.ButtonTone
import dev.shep.companion.ui.components.ExplainLine
import dev.shep.companion.ui.components.ExplainRow
import dev.shep.companion.ui.components.LoadingState
import dev.shep.companion.ui.components.ShepButton
import dev.shep.companion.ui.components.StepRow
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

/** Where the one screen a stranger sees currently is. */
private enum class PairStep { Steps, Code, Connecting, Linked, Failed }

/**
 * The only screen a stranger sees.
 *
 * It used to be a hero, a scan button, and two fields — bridge URL and token —
 * which between them assume you know there is a server, that it has a URL,
 * that the URL is a `ws://` one, and that a token exists to be pasted. Now it
 * is the two things you actually do (run a command, scan or type a code), and
 * the fields are still there under a disclosure for scripts and for anyone who
 * would rather paste.
 *
 * `imePadding` matters more here than anywhere else in the app: the fields sit
 * below the fold once the keyboard is up, and this screen had none — the token
 * field went under the IME the moment you tapped it.
 */
@Composable
fun PairingScreen(
    initialUrl: String,
    initialToken: String,
    lastError: String?,
    onConnect: (String, String, (String?) -> Unit) -> Unit,
    onClaim: (String, String, (String?) -> Unit) -> Unit = { _, _, done -> done("not supported") },
    onEnter: (Tab) -> Unit = {},
) {
    var url by remember { mutableStateOf(initialUrl.ifEmpty { "ws://100.64.0.0:7431/" }) }
    var token by remember { mutableStateOf(initialToken) }
    var host by remember {
        mutableStateOf(initialUrl.substringAfter("://", "").substringBefore("/").ifEmpty { "" })
    }
    var code by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf(lastError) }
    var step by remember { mutableStateOf(PairStep.Steps) }
    val linkedTo = remember(url, host) {
        host.ifBlank { hostOf(url.substringAfter("://", url)) ?: "your computer" }
    }

    fun connect(u: String, t: String) {
        busy = true
        error = null
        onConnect(u.trim(), t.filterNot { it.isWhitespace() }) { failure ->
            busy = false
            error = failure
            step = if (failure == null) PairStep.Linked else PairStep.Failed
        }
    }

    val scanLauncher = rememberLauncherForActivityResult(ScanContract()) { result ->
        val contents = result.contents ?: return@rememberLauncherForActivityResult
        val parsed = parsePairingUri(contents)
        if (parsed == null) {
            error = "that QR is not a shep pairing code"
        } else {
            url = parsed.first
            token = parsed.second
            step = PairStep.Connecting
            connect(parsed.first, parsed.second)
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
        when (step) {
            PairStep.Connecting -> LoadingState(
                "linking to $linkedTo…",
                detail = "checking that this phone and that computer can reach each other",
            )

            PairStep.Linked -> {
                Text("linked to $linkedTo", style = ShepType.hero)
                Text(
                    "this phone can now see and answer the agents running on that computer.",
                    style = ShepType.bodySmall,
                    modifier = Modifier.padding(vertical = ShepSpace.section),
                )
                ShepButton(
                    "go to agents",
                    onClick = { onEnter(Tab.Agents) },
                    modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
                )
                Spacer(Modifier.height(ShepSpace.medium))
                ShepButton(
                    "choose what to be notified about",
                    tone = ButtonTone.Quiet,
                    onClick = { onEnter(Tab.Shep) },
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            PairStep.Failed -> {
                Text("couldn't reach $linkedTo", style = ShepType.hero)
                Text(
                    "your phone and that computer have to be on the same network — the same " +
                        "wi-fi, or the same tailnet. nothing here goes over the internet.",
                    style = ShepType.bodySmall,
                    modifier = Modifier.padding(top = ShepSpace.medium),
                )
                error?.let {
                    Text(
                        it,
                        style = ShepType.metaSmall.copy(color = ShepPalette.red),
                        modifier = Modifier.padding(top = ShepSpace.small),
                    )
                }
                Spacer(Modifier.height(ShepSpace.section))
                ShepButton(
                    "try again",
                    onClick = { step = PairStep.Steps },
                    modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
                )
                Spacer(Modifier.height(ShepSpace.medium))
                ShepButton(
                    "enter the code by hand",
                    tone = ButtonTone.Quiet,
                    onClick = { step = PairStep.Code },
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            PairStep.Code -> {
                Text("type the code", style = ShepType.hero)
                Text(
                    "`shep bridge pair` prints both of these.",
                    style = ShepType.meta,
                    modifier = Modifier.padding(bottom = ShepSpace.section),
                )
                OutlinedTextField(
                    value = host,
                    onValueChange = { host = it },
                    label = { Text("computer", style = ShepType.fieldLabel) },
                    supportingText = {
                        Text(
                            "the name or address you would ssh to",
                            style = ShepType.metaSmall,
                        )
                    },
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
                    value = code,
                    onValueChange = { code = normalizeClaimCode(it).take(CLAIM_CODE_LENGTH) },
                    label = { Text("code", style = ShepType.fieldLabel) },
                    supportingText = {
                        Text(
                            "the $CLAIM_CODE_LENGTH characters shown next to it",
                            style = ShepType.metaSmall,
                        )
                    },
                    textStyle = ShepType.field,
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                error?.let {
                    Spacer(Modifier.height(ShepSpace.medium))
                    Text(it, style = ShepType.meta.copy(color = ShepPalette.red))
                }
                Spacer(Modifier.height(ShepSpace.indent))
                ShepButton(
                    if (busy) "linking…" else "link",
                    onClick = {
                        busy = true
                        error = null
                        step = PairStep.Connecting
                        onClaim(host.trim(), code) { failure ->
                            busy = false
                            error = failure
                            step = if (failure == null) PairStep.Linked else PairStep.Code
                        }
                    },
                    enabled = !busy && host.isNotBlank() && isCompleteClaimCode(code),
                    modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
                )
            }

            PairStep.Steps -> {
                Text("shep", style = ShepType.hero)
                Text(
                    "the cockpit in your pocket",
                    style = ShepType.meta,
                    modifier = Modifier.padding(bottom = ShepSpace.section),
                )
                StepRow(1, "on your computer, run `shep bridge pair`")
                Spacer(Modifier.height(ShepSpace.small))
                StepRow(
                    2,
                    "scan the square it draws",
                    detail = "or enter the computer's name and the $CLAIM_CODE_LENGTH-character " +
                        "code printed under it",
                )
                Spacer(Modifier.height(ShepSpace.section))
                ShepButton(
                    "scan the code",
                    onClick = {
                        error = null
                        scanLauncher.launch(
                            ScanOptions().apply {
                                setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                setPrompt("point the camera at the square shep drew")
                                setBeepEnabled(false)
                                setOrientationLocked(false)
                            }
                        )
                    },
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
                )
                Spacer(Modifier.height(ShepSpace.medium))
                ShepButton(
                    "enter the code",
                    tone = ButtonTone.Quiet,
                    onClick = { error = null; step = PairStep.Code },
                    enabled = !busy,
                    modifier = Modifier.fillMaxWidth(),
                )
                error?.let {
                    Spacer(Modifier.height(ShepSpace.medium))
                    Text(it, style = ShepType.meta.copy(color = ShepPalette.red))
                }
                Spacer(Modifier.height(ShepSpace.section))
                ExplainRow("why do I need a computer?") {
                    ExplainLine(
                        "shep runs there",
                        "the agents are processes on your own machine, with your own files. " +
                            "this app is a remote control for them, not a place they live.",
                    )
                    ExplainLine(
                        "same network",
                        "the phone reaches the computer directly over your wi-fi or tailnet. " +
                            "nothing here goes over the internet.",
                    )
                }
                ExplainRow("address and token") {
                    Text(
                        "the two values `shep bridge pair` prints, if you would rather paste " +
                            "them.",
                        style = ShepType.bodySmall,
                    )
                    OutlinedTextField(
                        value = url,
                        onValueChange = { url = it },
                        label = { Text("computer address", style = ShepType.fieldLabel) },
                        textStyle = ShepType.field,
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(
                            keyboardType = KeyboardType.Uri,
                            autoCorrectEnabled = false,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    OutlinedTextField(
                        value = token,
                        onValueChange = { token = it },
                        label = { Text("pairing token", style = ShepType.fieldLabel) },
                        textStyle = ShepType.field,
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(
                            keyboardType = KeyboardType.Password,
                            autoCorrectEnabled = false,
                        ),
                        // A token on this screen is one the bridge refused, or
                        // the one saved from last time; either way nobody can
                        // read it, so the way to replace it is a clear button,
                        // not a caret in the middle of it.
                        trailingIcon = {
                            if (token.isNotEmpty()) {
                                IconButton(onClick = { token = "" }) {
                                    Icon(Icons.Filled.Close, contentDescription = "clear token")
                                }
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    )
                    ShepButton(
                        if (busy) "connecting…" else "connect",
                        onClick = { connect(url, token) },
                        enabled = !busy && url.isNotBlank() && token.isNotBlank(),
                        modifier = Modifier.fillMaxWidth().height(ShepSize.buttonHeight),
                    )
                }
                Text(
                    "run `shep bridge pair --host <tailnet-ip>` on the computer.",
                    style = ShepType.metaSmall,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth().padding(top = ShepSpace.small),
                )
            }
        }
    }
}
