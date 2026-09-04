# Maestro E2E — shep companion

Text-anchor flows (no ids) per Alex's device-testing conventions. Every flow
runs against the real bridge, never a mock.

## Setup

1. AVD up (phone and/or tablet): `workmayt_pixel8_api35`,
   `workmayt_tablet_api35` (headless on the mini:
   `emulator -avd <name> -no-window -gpu swiftshader_indirect`).
2. Bridge running so the AVD can reach it on `10.0.2.2`:
   `env -u HERDR_SOCKET_PATH nohup ~/.local/bin/shep bridge \
      --bind 127.0.0.1:7432 --socket ~/.config/shep/shep.sock \
      > ~/.local/state/shep/bridge-7432.log 2>&1 &`
   (**--bind must come first** — serve-mode dispatches on argv[1].)
3. Release APK installed (`./gradlew assembleRelease`, then
   `adb -s <serial> install -r app/build/outputs/apk/release/app-release.apk`).

## Run

```bash
export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools
export PATH="$ANDROID_HOME/platform-tools:$PATH"

# Pair + home (any device; seeds the saved pairing the other flows rely on).
# SHEP_BRIDGE_URL is the bridge as the DEVICE sees it:
maestro --device <serial> test \
  -e SHEP_TOKEN=$(cat ~/.config/shep/bridge-token) \
  -e SHEP_BRIDGE_URL=ws://10.0.2.2:7432/ \
  maestro/01-pair-and-home.yaml

# Then, in any order:
maestro --device <serial> test maestro/02-tasks.yaml
maestro --device <serial> test maestro/03-memory.yaml
maestro --device <serial> test maestro/04-new-task-shortcut.yaml

# Tablet two-pane (wide AVD only):
maestro --device <tablet-serial> test maestro/05-tablet-two-pane.yaml

# Physical phone (tailnet bridge; dismiss the keyguard first — a locked
# phone shows only the splash to Maestro):
adb -s <phone-serial> shell wm dismiss-keyguard
adb -s <phone-serial> shell svc power stayon true
maestro --device <phone-serial> test \
  -e SHEP_TOKEN=$(cat ~/.config/shep/bridge-token) \
  -e SHEP_BRIDGE_URL=ws://100.83.179.75:7431/ \
  maestro/01-pair-and-home.yaml
```

## Gotchas learned (2026-07-17/18)

- **An `unauthorized` device in `adb devices` poisons maestro's dadb listing** —
  every device reports "not connected". `adb disconnect <serial>` the offender.
- **OxygenOS/ColorOS block `pm clear` for shell** (CLEAR_APP_USER_DATA
  SecurityException) — no `clearState: true` on physical phones. Flow 01 is
  idempotent instead: the pairing block runs only when the pairing screen is
  visible, so one flow covers fresh installs and paired devices.
- Maestro plain-string selectors are **full-text regexes**, not substrings:
  chip "attention 0" needs `"attention.*"`; "+ new" needs `"\\+ new"`.
- **Landscape IME covers the form** — `hideKeyboard` between fields and before
  tapping buttons on tablets.
- The first run on a fresh AVD can hit a transient dadb `tcp:7001: closed`;
  just re-run.
- Physical phones: wireless-adb ports rotate, so a stale `IP:5555` gets
  connection-refused. Re-pair per the `android-adb-repair` skill (needs the
  on-screen pairing code — a manual step), then the same flows work with
  `--device <phone-serial>`.
