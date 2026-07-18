# shep companion (Android)

The cockpit in your pocket: Android client for the shep terminal ADE.
Connects to `shep bridge` (WebSocket relay over the shep JSON API) on the
tailnet. See `docs/ANDROID-COMPANION.md` in the shep repo for the plan.

Pairing: run `shep bridge pair --host <tailnet-ip>` on the server, paste the
URL + token into the app.

v0 scope (A0–A2 core): pairing, live agents home (blocked-first, state
colors, review badges), pane view (live text, quick keys y/n/enter/esc/↑/↓,
prompt composer with send / queue-on-busy). A3 push (UnifiedPush, lock-screen
approve/deny), A4 tasks + memory tabs, A5 review & ship, A6 polish (home-screen
widget, "New task" launcher shortcut, voice add-task, tablet two-pane) — see
`maestro/README.md` for the E2E flows.

Build: `./gradlew assembleRelease` (JDK 17, compileSdk 35). Release is
debug-signed on purpose — personal sideload only.
