# shep companion (Android)

The cockpit in your pocket: Android client for the shep terminal ADE.
Connects to `shep bridge` (WebSocket relay over the shep JSON API) on the
tailnet. Lives in the shep repo under `android/`; see `docs/ANDROID-COMPANION.md`
for the plan. From the repo root: `just android-check` (tests + debug build),
`just android-install`, `just android-maestro`.

Pairing: run `shep bridge pair --host <tailnet-ip>` on the server. Scan the QR
in the app, or enter the computer's address and the 8-character claim code it
prints beneath. `pair` waits until the code is claimed and prints `paired`; the
code expires after five minutes, works once, and Ctrl-C takes it back.
`--no-wait` prints and returns. The URL + token fields are still there under
"address and token" for scripts.

v0 scope (A0–A2 core): pairing, live agents home (blocked-first, state
colors, review badges), pane view (live text, quick keys y/n/enter/esc/↑/↓,
prompt composer with send / queue-on-busy). A3 push (FCM, lock-screen
approve/deny, one notification per agent that clears when the agent is looked
at), A4 tasks + memory tabs, A5 review & ship, A6 polish (home-screen
widget, "New task" launcher shortcut, voice add-task, tablet two-pane) — see
`maestro/README.md` for the E2E flows.

## Screens

- **agents** — the session board grouped by groups (a group is what the API calls a workspace), with the same state and
  agent naming vocabulary as the desktop. Groups still expose open, close,
  rename, focus and split actions from their overflow menus.
- **tasks**, **memory**, **shep** (bridge, server, notification settings and
  push diagnostics).

## Push notifications

Delivery is FCM. It is the only transport that reliably wakes an Android app
out of Doze, which is the entire job of a notification — a self-hosted broker is
a background service the OS is free to kill, silently, exactly when you have
stopped watching the screen. Messages are data-only so the app builds its own
notification and keeps the lock-screen Approve/Deny.

This is the only Google dependency in a repo that is otherwise `org.json` +
OkHttp on purpose. It needs Play Services on the device and a
`app/google-services.json` from the Firebase project.

Each agent gets one notification: a newer event replaces it (the message's
`tag` is the pane id) rather than stacking beside it, and an `op = clear`
message — sent when the pane is looked at on the desktop, in this app, or on
another phone — takes it down. Opening an agent in the app calls
`pane.mark_seen`, which is what makes the other surfaces clear too.

Server side needs a Firebase service-account key at
`<shep config>/fcm-service-account.json`. `shep bridge notify-push` signs a JWT
with `openssl` to mint access tokens. If notifications are not arriving, the
**Send test notification** button in the shep tab reports per device what
actually happened — push failing is otherwise indistinguishable from nothing
having happened.

Which kinds to notify about (blocked / done / task / review) is chosen in the
app and stored on the server, so a muted kind costs no radio wake and the
setting survives a reinstall.

Build: `./gradlew assembleRelease` (JDK 17, compileSdk 35). Release is
debug-signed on purpose — personal sideload only.

## Security

- **Pairing at rest.** The bridge URL and token live in an
  `EncryptedSharedPreferences` file keyed by the Android keystore
  (`Pairing.kt`). A pairing saved by an older build is moved out of the plain
  preferences file the first time anything reads it. Backups and
  device-to-device transfers are off (`allowBackup="false"`,
  `data_extraction_rules.xml`).
- **Plaintext only to our own addresses.** The bridge speaks `ws://` on the
  tailnet or LAN, so cleartext stays allowed at the platform level, but
  `BridgeClient` refuses to send the token over `ws://` to anything that is not
  loopback, the emulator host, RFC 1918, the tailnet's `100.64.0.0/10`,
  link-local, IPv6 ULA/link-local, or a local-only name (`.local`, `.ts.net`,
  `.internal`, `.lan`, `.home.arpa`, bare hostnames). `wss://` is always fine.
  The rule is `net/HostPolicy.kt`, pinned by `HostPolicyTest`.
- **Release signing.** `assembleRelease` needs a real keystore from
  `SHEP_ANDROID_KEYSTORE` / `SHEP_ANDROID_KEYSTORE_PASSWORD` /
  `SHEP_ANDROID_KEY_ALIAS` / `SHEP_ANDROID_KEY_PASSWORD` (or `shep.keystore`,
  `shep.keystore_password`, `shep.key_alias`, `shep.key_password` in
  `local.properties`) and fails at packaging without one. Debug builds are
  unaffected.
- **Bridge allowlist.** The phone can only call the methods `shep bridge`
  relays (see `BRIDGE_ALLOWED_METHODS` in `src/cli/bridge.rs`); everything else
  is refused server-side.

## Live input

Keys from the key bar and the soft keyboard go through one `InputRouter`
(`net/InputRouter.kt`): down the open `pane.stream` channel when there is one,
otherwise as `pane.send_text` / `pane.send_keys` requests in order — so what
you type while "connecting to pane…" is still on screen lands. A refused
socket write is reported in the notice line rather than dropped. Gboard's
delete bursts are coalesced into one write carrying N backspaces
(`terminal/TerminalIme.kt`, pinned by `TerminalInputTest`), and the keyboard is
put away when input leaves stream mode or the pane screen goes.
