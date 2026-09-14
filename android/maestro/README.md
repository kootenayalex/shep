# Maestro E2E — shep companion

Text anchors for content, per Alex's device-testing conventions; test ids for
the navigation chrome, whose labels the board reuses as region headings (see
"the board", below). Every flow runs against the real bridge, never a mock.

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

# Against the throwaway dev stack the token is in the debug config dir:
#   -e SHEP_TOKEN=$(cat ~/.config/shep-dev/bridge-token)

# Then, in any order:
maestro --device <serial> test maestro/03-memory.yaml

# Tablet two-pane (wide AVD only):
maestro --device <tablet-serial> test maestro/05-tablet-two-pane.yaml

# Groups, output/input modes, live and queued input, the key bar,
# notification clearing, move-to-group, manual state, todos:
maestro --device <serial> test maestro/06-groups.yaml
maestro --device <serial> test maestro/07-pane-output-modes.yaml
maestro --device <serial> test maestro/08-live-input.yaml     # …14

# Docket tab (adds one inbox item — throwaway server only):
maestro --device <serial> test maestro/15-docket.yaml

# The board — the landing screen: regions, the desktop|board pill, the read of
# the room, and a real question to the overseer. Needs the overseer plugin
# linked, a headless runtime and one tick already run; `just dev-stack up`
# sets all three up on a throwaway server, and 16's header has the recipe:
maestro --device <serial> test maestro/16-board.yaml
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

## Gotchas learned (2026-09-12, the board)

- **The board is now the landing screen**, and its region headings are words
  the hint bar also uses. `board` matches the pill's half *and* the `b board`
  tab — a plain `tapOn:` takes the first one, which is the wrong one. `agents`
  is no longer a tab at all: the `desktop | board` pill is the switch, and
  `agents` on the board is a region heading that does nothing when tapped.
- **The board dropped `needs you` and the docket** (2026-09-13). A blocked or
  finished agent is a row in the agents list with its state beside it, and the
  docket has a tab of its own (flow 15), so neither anchor exists on the board
  any more — flow 16 lost its `"  due "` assertion with them. The overseer's
  paragraph about an agent is now seated under that agent's row as body text
  (a tap on it opens the same pane the row does), and the `✦ read of the room`
  region holds only what is left: the `room` section and anything about no
  agent on screen. On a board where every paragraph was seated the region is
  not drawn at all, so anchor on the room text only against a tick that wrote
  a `## room` section — the deterministic template always does.
- **So the navigation chrome carries test ids, and the flows use them.** Text
  anchors are still the rule for *content*; a control whose label the board
  also uses as a heading is not content. `testTagsAsResourceId` is on, so a
  testTag is an `id:` selector:
  - `hint-<label>` — one per hint-bar entry: `hint-board`, `hint-docket`,
    `hint-memory`, `hint-shep`.
  - `pill-desktop` / `pill-board` — the two halves of the `desktop | board`
    pill. (This replaced flow 16's `rightOf: {text: "desktop"}` trick.)
  - `overseer-strip` — the overseer's one row on the agents screen.
  A flow that wants the agents list opens with `tapOn: {id: "pill-desktop"}`,
  because that is where the app now lands. Coming back to it **from another
  tab** is two taps, not one — the bar has no `agents` entry — so it is
  `hint-board` then `pill-desktop` (flow 03).
- **`shep · agents` is two text nodes**, so `assertVisible: "agents"` does not
  match the agents header; it matches the *board's* region heading instead.
  Assert `"· agents"` / `"· board"` for "which screen am I on". Flow 01 asserts
  neither: a fresh pairing ends on agents and a saved one on the board, so it
  pins the two things both carry — the pill and the connection line.
- **The board's header carries the connection line too** (`live · shep
  <version>`, or `reconnect`), shared with the agents header as
  `ConnectionLine` in `ChannelsScreen.kt`. It shipped missing for one commit,
  which is what made flow 01 fail on an already-paired device.
- **The agents list is as long as the session.** Anything that reaches a
  particular agent by name wants `scrollUntilVisible` first — flow 12 parks
  its agent in a group of its own at the *end* of the list, so 13 could not
  find it afterwards.
- **A region's heading is several text nodes, not one** — a title, a count,
  and whatever else it carries are separate elements, so a heading that reads
  as one line on screen is never one string to match. Anchor on the single
  node that is the label (the docket tab's lane headings are `due` and a
  separate `2`, not `due 2`).
- **A placeholder painted under its text field is invisible to Maestro.** The
  board's composer really does read `ask the overseer` on screen, but the
  `BasicTextField` is drawn over it and Maestro's bounds filter drops what is
  behind. Hence the one id in flow 16: the field's `overseer-composer`
  testTag (`testTagsAsResourceId` is on, so a testTag is an `id:` selector).
- **A `LazyColumn` only has its visible rows in the hierarchy.** The board is
  taller than the screen, so `health`, `✦ read of the room` and
  `chat` need `scrollUntilVisible` before any assertion about them — a plain
  `assertVisible` fails on a region that simply has not been composed yet.
