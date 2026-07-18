# Shep Android Companion — Plan

Status: A0–A4 BUILT (A4 2026-07-17). A0–A2 shipped + tails closed (bottom-nav
shell, event-driven home, filter chips, ANSI pane render, QR pairing, version
gating). A3 notifications implemented and server-verified end-to-end; the final
locked-screen gate needs the ntfy distributor app on the S22 (Alex's manual
step). A4 tasks + memory tabs built and live-verified over the bridge. A5 review &
ship built (workspace.diff/ship JSON API methods + review screen). A6 not
started. Feature parity with the desktop ADE, re-shaped for a
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
 └─ notify exec-bridge ── ntfy (self-hosted) ── UnifiedPush ── Android
```

- **`shep bridge`** (new, in-tree Rust subcommand): a thin authenticated relay
  — WebSocket ↔ API socket pass-through (requests, event-hub subscriptions,
  and `terminal session observe`/`control` NDJSON streams multiplexed as
  channels). No new methods, no protocol invention: the phone speaks the same
  JSON Schema `shep api schema` already publishes. Binds the tailscale
  interface (or 127.0.0.1 behind `tailscale serve`); token file under
  `~/.config/shep/bridge-token`; never exposed publicly.
- **Push** without Google dependency (standing no-paid/local-first rule):
  M1's notify exec-bridge fires on `notify_on = ["blocked"]` → self-hosted
  ntfy (unraid or OVH VPS, tailnet-only) → UnifiedPush → app. Foregrounded,
  the app holds its own WebSocket; background wake-ups come only from ntfy —
  no persistent connection, no battery burn.
- **Version skew**: the bridge ships in the shep repo and reports
  `{server_version, api_schema_version}` at handshake; the app hard-gates on
  schema major, soft-warns on minor — the `shep status` compatibility story,
  phone-shaped.

## Repos

- Bridge: `~/vault/dev/shep` (Rust, part of `just check`).
- App: new repo `~/vault/dev/shep-android` (Kotlin, Jetpack Compose, Material 3,
  minSdk 26, gitea+github dual-push per convention). Separate repo keeps
  gradle out of the Rust workspace; the schema JSON is vendored into the app
  at build time from a pinned shep commit.

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
| Toasts | Snackbars in-app; ntfy notifications out-of-app. |
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
- **A3 — notifications. BUILT 2026-07-14.** ntfy self-hosted (Docker + tailscale
  sidecar at `https://ntfy.tail58187b.ts.net`, deploy in
  `shep-android/deploy/ntfy/`) as the UnifiedPush distributor. Server: the phone
  registers its endpoint over the bridge (`push.register`, handled bridge-locally
  → `<config>/push-endpoints.json`, no protocol bump); `[notifications] exec =
  "shep bridge notify-push"` POSTs the transition context to each endpoint
  (`SHEP_NTFY_PUBLISH_BASE` keeps the publish on loopback since shep + ntfy are
  co-located). App: UnifiedPush `MessagingReceiver` renders an actionable
  notification whose Approve/Deny fire `pane.send_keys` (y/n) over a short-lived
  bridge connection, and whose tap deep-links `shep://pane?pane=…`. Verified
  live: register→persist→exec→notify-push→ntfy publish; app installs/launches,
  fires the POST_NOTIFICATIONS prompt, and the distributor-detection path runs on
  the AVD. Gate (lock-screen approve, app never opened) pending the ntfy app on a
  real device — Alex installs ntfy (F-Droid), points it at the ntfy server, then
  installs the APK + pairs.
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
- **A6 — polish.** Home-screen widget (blocked count + top blocked agent),
  app shortcuts ("New task"), voice add-task, tablet two-pane layout (S22 vs
  iPad-class widths), Maestro E2E on the AVD + S22 (text anchors, per
  device-testing conventions).

## Verification

Per milestone: bridge unit/integration tests inside `just check`; app-side
instrumented tests + one Maestro flow per gate on the local AVD, then the S22
over tailnet adb. Every gate above is a live end-to-end check against the real
macmini server, never a mock (mock-pg lesson generalized).

## Risks / open questions

- **VT rendering on Android** — RESOLVED (2026-07-12): shipped the read-only
  ANSI-to-AnnotatedString renderer (`AnsiRender.kt`), not Termux terminal-view.
  Observe-only v1, so control keys need no local echo; contained to one module.
- **Battery vs immediacy** — no persistent background socket; ntfy wake-ups
  only. If ntfy latency disappoints, revisit with a foreground-service toggle
  ("on shift" mode), never a silent always-on drain.
- **Bincode temptation** — never implement the TUI client protocol in Kotlin;
  the JSON API is the contract. If a needed capability exists only in the
  bincode plane, add it to the JSON API server-side first (guardrail-clean).
- **Two-repo drift** — schema vendored at pinned commits + handshake gating is
  the mitigation; CI check in shep-android compares vendored schema to the
  pinned shep rev.
- **AGPL** — fine for a personal companion; note again if it ever ships to a
  store beyond personal sideload (Play listing would need source availability).
