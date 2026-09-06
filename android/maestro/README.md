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
3. Debug APK installed (`just android-build` at the repo root, then
   `adb -s <serial> install -r android/app/build/outputs/apk/debug/app-debug.apk`).
   The release build needs a real keystore (`SHEP_ANDROID_*` or
   `shep.*` in `local.properties`) and fails without one; for the AVD the
   debug build is the one to use.

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

# Groups, output/input modes, live and queued input, the key bar,
# notification clearing, move-to-group, manual state:
maestro --device <serial> test maestro/06-groups.yaml
maestro --device <serial> test maestro/07-pane-output-modes.yaml
maestro --device <serial> test maestro/08-live-input.yaml     # …13
```

Flows 08–13 type into a plain shell agent named `shell` (`-e AGENT=` to
change it) and only see the screen. `input-checks.py` owns the other half:
it starts that agent over the JSON socket, holds it in a manual "working"
state around the queue flow, posts the notification the clear flow
dismisses, and reads the pty, the agent's state and group, and the
notification shade back afterwards. Run it against a throwaway server, not
the one that owns your terminals:

```bash
env -u HERDR_SOCKET_PATH SHEP_SOCKET_PATH=/tmp/shep-dev/api.sock \
  target/debug/shep server &
target/debug/shep bridge --bind 127.0.0.1:7432 --socket /tmp/shep-dev/api.sock &
SHEP_SOCKET_PATH=/tmp/shep-dev/api.sock MAESTRO_DEVICE=emulator-5554 \
  android/maestro/input-checks.py            # --only 08,09 to narrow
```

Each flow's junit report lands in `/tmp/shep-dev/maestro/` (`--junit-dir`).
The whole directory runs on a phone with the tablet flow left out:
`maestro --device <serial> test --exclude-tags tablet -e SHEP_TOKEN=… \
  -e SHEP_BRIDGE_URL=… --format junit --output /tmp/shep-dev/maestro.xml android/maestro/`.
Notification checks need the debug config dir (`~/.config/shep-dev/`) to
hold an `fcm-service-account.json` and the AVD to be a Google APIs image;
the emulator registers its own FCM token with the bridge when it pairs.

```bash
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
  the header's "live · shep 0.7.3" needs `"live.*"`; "+ new" needs `"\\+ new"`.
  That header is also why the agents list must never render a bare `live`:
  flow 07 taps the first element whose *whole* text is `live` (the pane's out
  toggle), and the tablet layout has both on screen at once.
- **Landscape IME covers the form** — `hideKeyboard` between fields and before
  tapping buttons on tablets.
- The first run on a fresh AVD can hit a transient dadb `tcp:7001: closed`;
  just re-run.
- Physical phones: wireless-adb ports rotate, so a stale `IP:5555` gets
  connection-refused. Re-pair per the `android-adb-repair` skill (needs the
  on-screen pairing code — a manual step), then the same flows work with
  `--device <phone-serial>`.
