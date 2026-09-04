# Shep Android Companion — Plan

Status: A0–A5 BUILT (A5 2026-07-17), A6 BUILT 2026-07-17 (app-side; physical-
device gates owed). A0–A2 shipped + tails closed (bottom-nav
shell, event-driven home, filter chips, ANSI pane render, QR pairing, version
gating). A3 notifications rebuilt on FCM 2026-09-03: one notification per
agent that a newer event bumps, withdrawn when the agent is looked at on any
surface; the lock-screen gate is AVD-verified, real-phone check owed. A4 tasks + memory tabs built and live-verified over the bridge. A5 review &
ship built (workspace.diff/ship JSON API methods + review screen). A6 polish
built (widget, launcher shortcut, voice add-task, tablet two-pane, Maestro
E2E). Feature parity with the desktop ADE, re-shaped for a
phone: small screen, thumb navigation, interrupted attention. Same NN/g
psychology contract as the desktop-feel pass (`.local/prd/desktop-feel-pass.md`).

## Positioning — the cockpit in your pocket

The companion is NOT a terminal replacement (Termux + mosh already covers
"I need a shell"). It is the ADE meta-layer, mobile-shaped: *which agent needs
me, approve/deny it, queue the next prompt, review and ship, dispatch a task —
from anywhere on the tailnet.* The single most valuable interaction is
answering a blocked agent's permission prompt from a lock-screen notification
without opening anything.

## Architecture — a third client, not a second system

Shep is a detached server with thin clients; the phone is simply the next
client. Nothing moves server-side that isn't already there (runtime/client
guardrail holds).

```
shep server (macmini)
 ├─ unix socket · client protocol (bincode)   ← TUI, unchanged
 ├─ unix socket · API (NDJSON + JSON Schema)  ← `shep api`, event hub
 │      └── shep bridge ── WebSocket/TLS, tailnet-only, bearer token
 │                            └── Android app (Kotlin/Compose)
 └─ notify exec-bridge ── `shep bridge notify-push` ── FCM (data-only) ── Android
```

- **`shep bridge`** (new, in-tree Rust subcommand): a thin authenticated relay
  — WebSocket ↔ API socket pass-through (requests, event-hub subscriptions,
  and `terminal session observe`/`control` NDJSON streams multiplexed as
  channels). No new methods, no protocol invention: the phone speaks the same
  JSON Schema `shep api schema` already publishes. Binds the tailscale
  interface (or 127.0.0.1 behind `tailscale serve`); token file under
  `~/.config/shep/bridge-token`; never exposed publicly.
- **Push** is FCM, data-only (the app builds its own notification, so the
  lock-screen Approve/Deny survives). It is the one Google dependency, taken
  because nothing else wakes an app out of Doze reliably. The original
  self-hosted ntfy/UnifiedPush path was removed 2026-09-03 once FCM was
  confirmed; stale ntfy rows in `<config>/push-endpoints.json` can be deleted.
  Foregrounded, the app holds its own WebSocket; background wake-ups come only
  from push — no persistent connection, no battery burn.
- **Version skew**: the bridge ships in the shep repo and reports
  `{server_version, api_schema_version}` at handshake; the app hard-gates on
  schema major, soft-warns on minor — the `shep status` compatibility story,
  phone-shaped.

## Repo layout (one repo since 2026-09-03)

- Bridge: Rust crate at the repo root (part of `just check`).
- App: `android/` in the same repo (Kotlin, Jetpack Compose, Material 3,
  minSdk 26). The former `shep-android` repo was merged in with its history
  (`git subtree`); `kootenayalex/shep-mobile` on GitHub is archived. One repo
  means an API change and the companion change that uses it land in the same
  commit, and `just check` runs the companion gate (`just android-check`).
- Dev loop: `just android-install` puts the debug APK on a device or AVD;
  `just android-maestro` runs `android/maestro/`.

## Surface mapping — desktop → phone

| Desktop surface | Phone adaptation |
|---|---|
| Sidebar (blocked-first) + board | **Home = the attention queue.** One list, blocked pinned on top; the board's columns become filter chips. On a phone the sidebar and board were always the same idea — merge them. |
| Titlebar attention slot | Status chip on Home + app icon badge + notification. |
| Hint bar / prefix chords | **Bottom navigation + contextual action chips.** No prefix key exists; every action is a visible, labeled control (recognition, not recall). |
| Pane (terminal + ring) | Full-screen pane view: live render via `terminal session observe`, state ring as a colored app-bar border, swipe left/right between panes of a workspace. |
| Tab-to-queue ("Queue prompt…") | Prompt composer docked above the keyboard, with an explicit "queued — fires on idle" state. Voice input via system STT (later: whisper stack over tailnet). |
| Review diff (pager pane) | Native diff screen from `git diff` output fetched through the bridge; file list → per-file hunks. |
| Request changes modal | Bottom sheet with text field → routes into the origin pane (existing API path). |
| Ship worktree | Ship button with loss-aversion-honest confirm ("merge task/41 → master · 12 commits · then remove worktree") → `✓ shipped` success moment. |
| Task queue CLI | Tasks tab: list with states, add-task sheet (repo picker, runtime, `--worktree` toggle), cancel, dispatch-now. |
| `shep memory` CLI | Memory tab: USER/repo files, `§`-entry list, add/edit/remove, search (FTS5), cap meter with the >80% nudge. |
| Toasts | Snackbars in-app; FCM notifications out-of-app. |
| Modals | Bottom sheets (thumb reach), destructive ones red-headed with an explicit noun ("Close workspace *api*?"). |

## Navigation

- **Bottom nav, 4 tabs** (Hick's law ceiling): **Agents · Tasks · Memory · Shep**
  (the last = server status, theme, notification rules, bridge pairing).
  Review is not a tab — review state lives on the workspace, so it's reached
  *through* Agents (list → workspace → Review/Ship actions), keeping one mental
  model with the TUI's context menu.
- Stack: Agents → workspace detail (panes, review state, branch/mem meta) →
  pane view. System back everywhere; deep links from notifications go straight
  to the pane view (or straight to an approve/deny action — see A3).
- One-handed rule: primary actions live in the bottom half; nothing
  load-bearing in the top corners except the status chip.

## Psychology contract (NN/g, same guide as desktop)

- **Attention / tunnel vision** — one attention queue, blocked always first;
  exactly one red on any screen (Von Restorff): blocked and destructive. Peach
  = warning tier (behind, changes-requested, mem%), copper = working — the
  same vocabulary as the TUI and the shep theme, so the two surfaces build one
  mental model (picture superiority + consistency).
- **Interaction cost** — the killer feature is *approve/deny from the
  notification itself*: zero taps to context. Every screen answers "what does
  this agent need" in the first glance.
- **Fitts's law** — bottom nav, ≥48dp targets, swipe gestures for
  pane-switching rather than tiny tab strips.
- **Hick's law / choice overload** — ≤4 tabs, ≤3 actions per screen visible;
  everything else behind the sheet ("More" progressive disclosure).
- **Recognition over recall** — no chords, no hidden gestures without a
  visible equivalent; chips are labeled with verbs ("Queue prompt", "Ship").
- **Defaults** — notify-only-when-blocked ON, home filter = attention, dark
  shep theme ON. People don't change defaults; ship the right ones.
- **Loss aversion / framing** — destructive confirms state the concrete loss,
  not "Are you sure?".
- **Peak-end** — ship success and task completion get explicit success
  moments; sessions tend to end right after an approval or a ship, so those
  states are the app's last impression.
- **Zeigarnik** — Tasks tab leads with in-progress/queued (open loops), done
  collapses.
- **Spatial memory** — fixed tab order, stable card layout; state changes
  reorder *within* the list but never relocate chrome.

## Milestones

- **A0 — bridge + skeleton.** `shep bridge` (WS relay, token auth, tailnet
  bind, handshake versioning) with Rust tests in `just check`; Android repo
  scaffold (Compose, Material 3 shep theme, bottom nav, pairing screen: server
  URL + token via QR from `shep bridge pair`). Gate: phone shows live
  `shep status` equivalent over tailnet.
- **A1 — command center.** Agents home via `session.snapshot` + event-hub
  subscription: state dots, review badges ◆↺✓, branch/±/age/mem% meta, filter
  chips. Gate: force-block an agent on the mini → phone reorders within 1s.
- **A2 — pane view + queue.** `terminal session observe` rendering (v1: an
  embedded VT view — evaluate Termux `terminal-view` (GPLv3, license-compatible
  enough for a personal AGPL companion; decide before code) vs a minimal
  read-only ANSI renderer of the observe stream), quick-keys row
  (y/n/enter/esc/arrows via `terminal session control`), prompt composer with
  queue-on-busy. Gate: answer a real claude permission prompt from the phone.
- **A3 — notifications. BUILT 2026-07-14, REBUILT 2026-09-03.** Server: the
  phone registers its FCM token over the bridge (`push.register`, handled
  bridge-locally → `<config>/push-endpoints.json`, no protocol bump);
  `[notifications] exec = "shep bridge notify-push"` sends the transition
  context, data-only, to each registered device. Every message carries a
  `tag` (the pane id) and an `op`: `show` raises or replaces the one
  notification that agent gets, `clear` withdraws it. The server fires the
  clear when the pane is looked at — on the active tab of a focused desktop
  client, through `pane.mark_seen` from a companion, or because the pane is
  gone — so a question answered at the desk stops paging the phone. App:
  `ShepMessagingService` builds the notification (Approve/Deny fire
  `pane.send_keys` y/n over a short-lived bridge connection; tap deep-links
  `shep://pane?pane=…`); opening an agent anywhere in the app cancels its
  notification locally and calls `pane.mark_seen`. The original self-hosted
  ntfy/UnifiedPush transport was removed in the rebuild.
- **A4 — tasks + memory. BUILT 2026-07-17.** Bridge-local `task.list/add/cancel`
  and `memory.show/add/replace/remove` (direct ops on `<state>/tasks.db` and the
  memory files — same as the CLIs, no new API method / protocol bump, mirroring
  the `push.register` precedent); `task.dispatch` still proxies to the server
  since only it can spawn a pane. App: **Tasks tab** (queue with state colors,
  add-task sheet with repo picker + runtime + worktree toggle, cancel,
  dispatch-now; polls so a dispatched task visibly flips todo→running→done) and
  **Memory tab** (USER profile entries, cap meter with the >80% consolidate
  nudge, add/edit/remove). Memory `search` (over the FTS history db) and repo-
  scoped memory browsing are deferred. Verified live over the real bridge:
  task.add/list/cancel + memory CRUD against real state, `just check` green, and
  the AVD renders both tabs + the add-task sheet from live bridge data. Gate
  (dispatch → board flip) uses the pre-existing `task.dispatch` server method.
- **A5 — review & ship. BUILT 2026-07-17.** Two new **JSON API** methods
  (server-owned git/worktree state → guardrail-correct as API methods, not
  bridge-local): `workspace.diff` (reuses the review-pager diff target;
  returns target ref + `--stat` + capped unified diff) and `workspace.ship`
  (reuses `ship_merge` — merge the worktree branch into its base, refusing on
  dirty/detached/conflict without losing work). Schema artifact regenerated;
  no protocol bump. Request-changes needs no new method: `agent.send` the
  feedback into the agent pane + the existing `workspace.set_review_state`.
  App: a **Review screen** off the pane view (Agents → pane → review) showing
  the colorized diff, a request-changes bottom sheet, and a Ship button (linked
  worktrees only) behind a loss-aversion-honest confirm → `workspace.ship` then
  `worktree.remove` cleanup. Verified: `workspace.diff` live end-to-end over a
  throwaway bridge (real diff text on the wire); `ship_merge` + handler wiring
  unit-tested; a client-triggerable server **panic** (out-of-range
  `parse_workspace_id` index) found via live testing, fixed, and regression-
  tested; `just check` 2684 green. Not smoke-rendered on the AVD (reaching the
  Review screen needs a live pane + a debug-server re-pair; blocked by AVD
  keyboard/SELinux automation friction, not a code issue).
- **A6 — polish. BUILT 2026-07-17.** All app-side (no server changes, no
  protocol surface): **home-screen widget** (`ShepWidgetProvider` +
  RemoteViews — blocked count + top blocked agent in the shep palette; tap →
  deep-link into the top blocked pane, ↻ → manual re-pull; updates are
  pull-on-demand per the battery guardrail: 30-min system floor + refresh on
  every incoming push + tap), **launcher shortcut** "New task"
  (`res/xml/shortcuts.xml` → `shep://tasks/new` → Tasks tab with the add sheet
  pre-opened, showAdd hoisted to NavShell), **voice add-task** (RecognizerIntent
  chip in the add sheet — the recognizer app holds the mic, so no RECORD_AUDIO;
  absent recognizer degrades to an inline note), **tablet two-pane** (BoxWithConstraints
  ≥720.dp: agents list + pane detail side-by-side, phone path unchanged), and
  **Maestro E2E** (`shep-android/maestro/`, 5 text-anchor flows + README:
  pair-and-home, tasks incl. the voice chip, memory, the `shep://tasks/new`
  deep link, tablet two-pane). Verified: all flows GREEN on the phone AVD
  (emulator-5554) and pairing + two-pane GREEN on the tablet AVD
  (workmayt_tablet_api35, 2560×1600); widget provider + shortcut published
  (dumpsys appwidget/shortcut), refresh broadcast crash-free. Owed (needs
  Alex's hands): Maestro on a physical phone — both phones' wireless-adb ports
  rotated (re-pair per android-adb-repair, a manual pairing-code step) and the
  S22 USB showed `unauthorized`; widget pinning on the S22 home screen; voice
  recognizer on real hardware (AVDs ship no STT app).

## Verification

Per milestone: bridge unit/integration tests inside `just check`; app-side
instrumented tests + one Maestro flow per gate on the local AVD, then the S22
over tailnet adb. Every gate above is a live end-to-end check against the real
macmini server, never a mock (mock-pg lesson generalized).

## Risks / open questions

- **VT rendering on Android** — RESOLVED (2026-07-12): shipped the read-only
  ANSI-to-AnnotatedString renderer (`AnsiRender.kt`), not Termux terminal-view.
  Observe-only v1, so control keys need no local echo; contained to one module.
- **Battery vs immediacy** — no persistent background socket; push wake-ups
  only. If push latency disappoints, revisit with a foreground-service toggle
  ("on shift" mode), never a silent always-on drain.
- **Bincode temptation** — never implement the TUI client protocol in Kotlin;
  the JSON API is the contract. If a needed capability exists only in the
  bincode plane, add it to the JSON API server-side first (guardrail-clean).
- **Two-repo drift** — RESOLVED 2026-09-03 by merging the app into this repo
  under `android/`; handshake gating stays as the runtime check.
- **AGPL** — fine for a personal companion; note again if it ever ships to a
  store beyond personal sideload (Play listing would need source availability).
