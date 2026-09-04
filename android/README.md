# shep companion (Android)

The cockpit in your pocket: Android client for the shep terminal ADE.
Connects to `shep bridge` (WebSocket relay over the shep JSON API) on the
tailnet. Lives in the shep repo under `android/`; see `docs/ANDROID-COMPANION.md`
for the plan. From the repo root: `just android-check` (tests + debug build),
`just android-install`, `just android-maestro`.

Pairing: run `shep bridge pair --host <tailnet-ip>` on the server, paste the
URL + token into the app.

v0 scope (A0–A2 core): pairing, live agents home (blocked-first, state
colors, review badges), pane view (live text, quick keys y/n/enter/esc/↑/↓,
prompt composer with send / queue-on-busy). A3 push (UnifiedPush, lock-screen
approve/deny), A4 tasks + memory tabs, A5 review & ship, A6 polish (home-screen
widget, "New task" launcher shortcut, voice add-task, tablet two-pane) — see
`maestro/README.md` for the E2E flows.

## Screens

- **agents** — the session board grouped by spaces, with the same state and
  agent naming vocabulary as the desktop. Spaces still expose open, close,
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
`app/google-services.json` from the Firebase project. The UnifiedPush path is
still present and still works; it will be removed once FCM is confirmed on real
hardware.

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
