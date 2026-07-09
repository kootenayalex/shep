# Shep — Vision & Roadmap (Flagship ADE on the herdr fork)

## Context

Alex's daily workflow is mosh → herdr → 4–6 concurrent claude-code/opencode sessions sharing the same agent files. He forked `ogulcancelik/herdr` → `kootenayalex/shep` ("herdr as an ADE") and wants it evolved into a flagship-class terminal ADE, absorbing the best TUI-appropriate ideas from `per-simmons/damon-ade`, codex CLI, cmux, conductor.build, the hermes-agent memory system, and anything better in the landscape.

**Decisions locked with Alex:** clone lives at `~/vault/dev/shep`; diverge freely from upstream (no rebase constraint); v1 covers ALL four domains — session command center, shared memory, review & merge, task queue — plus my own ideas.

## Ground truth (from research, verified 2026-07-08)

- Fork is **0 commits diverged** from upstream master (`552aa8c`, herdr 0.7.3). ~190k LOC Rust: ratatui 0.30 + crossterm, tokio, vendored portable-pty, bincode socket IPC. AGPL-3.0. Excellent quality: ~2,521 inline tests + 9 integration suites, documented guardrails in `AGENTS.md` (server state + JSON API for shared facts; pure render client-side; no god objects).
- herdr already has: detached server + thin client (mosh-perfect), agent-state detection (`working|blocked|idle|done`) via TOML screen manifests (`src/detect/manifests/{claude,opencode,...}.toml`) AND authoritative integration hooks it installs into claude/opencode configs (`src/integration/`), a full socket API with JSON schema (`src/api/`), event hub (`src/api/event_hub.rs`), worktree support (`src/worktree.rs`), plugins, session persistence, live-handoff.
- herdr has **no** memory, no review/merge flow, no task queue, no cross-session coordination. Those are the net-new build.
- damon-ade (Electron, Elastic-2.0, Superset fork) — its portable gems, all filesystem/CLI-based, no SDK/ACP: (1) Hermes memory scaffold bridged through each CLI's native context files + a claude Stop-hook forcing one reflection turn; (2) a universal `notify.sh` hook normalizing claude/codex/opencode lifecycle events → `Start/Stop/PermissionRequest`; (3) multi-model launch table incl. `ANTHROPIC_BASE_URL` override trick; (4) heredoc one-shot prompt injection; (5) fs-watchers → agent invoke.
- hermes memory core: capped markdown files (`MEMORY.md` ~2,200 chars, `USER.md` ~1,375), `§`-delimited entries, `add/replace/remove` by substring, **error-on-overflow (no auto-compact) forcing consolidation**, frozen-snapshot injection at session start, SQLite FTS5 history sidecar.
- Landscape steals: cmux notification rings + jump-to-unread; codex `HistoryCell`/incremental-render discipline + Tab-to-queue input; opencode width-adaptive diff (`auto|stacked`); claude-squad lifecycle verbs (launch/pause/review/ship); Crystal (MIT) diff-stats + auto worktree cleanup; Vibe Kanban board-as-overview; omnara notify-only-when-blocked.
- Reference clones (session-temporary, copy what's needed in M0): `/private/tmp/claude-501/-Users-alex/64090c62-81aa-457c-827a-fafb0620c0f0/scratchpad/{shep,damon-ade}`. Key damon-ade files: `apps/desktop/src/main/lib/agent-scaffold.ts` (memory templates + reflect hook), `agent-setup/templates/notify-hook.template.sh`, `notifications/map-event-type.ts`, `packages/shared/src/agent-command.ts` (launch table + heredoc), `scheduler/watcher.ts`, `docs/memory.md`.

## Architecture principles

1. **Build beside the VT core, not into it.** herdr's server/API/detection layers are the substrate. New ADE state lives in **server state + socket API** (per upstream `AGENTS.md` guardrail); the TUI renders it. Never touch `terminal/state.rs` / `pane/terminal.rs` unless forced.
2. **Files + CLI-native bridges over custom protocols.** Memory, tasks, hooks all ride plain files, the existing socket API, and each agent CLI's own config mechanisms (damon-ade's proven approach). Agents interact via the `shep` CLI (they already have Bash).
3. **Mosh discipline:** every new UI surface is glyph/color-cheap, width-adaptive, incremental-redraw.
4. **No paid third-party services** (standing rule): local model routing goes through odysseus-mlx (`127.0.0.1:7860/mlx/v1`), not OpenRouter; notifications bridge to user-configured local commands (voicebox/KDE Connect), no push SaaS.

## Milestones

### M0 — Foundation (repo, rename, baseline)
- Clone `kootenayalex/shep` → `~/vault/dev/shep`; set up dual-push (gitea+github per convention, `gitea-dual-push` skill).
- Copy needed damon-ade reference files into `docs/reference/damon-ade/` (scratchpad is session-temporary) + write `docs/VISION.md` from this plan.
- **Full rename herdr→shep** (diverging freely): crate/binary name, `HERDR_*` env → `SHEP_*` (read `HERDR_*` as fallback for one transition period — installed hooks in live agent configs still emit them), config `~/.config/shep`, state `~/.local/state/shep`, socket paths, integration assets `shep-agent-state.*`, disable upstream manifest auto-update URL. First-run import of existing `~/.config/herdr/config.toml`.
- Build (`cargo build --release`), run full test suite (`just test`), install to `~/.local/bin/shep` alongside existing herdr.
- Verify: attach, spawn claude + opencode panes, confirm state detection + detach/reattach still work.

### M1 — Session command center
The daily-driver win: "which of my 6 agents needs me right now."
- **Blocked-first sidebar** sort + per-workspace metadata line (branch, dirty-file count, seconds-since-last-event): extend `src/workspace/aggregate.rs`, `src/ui/sidebar.rs`.
- **Jump-to-next-blocked hotkey** (cmux): new action in `src/app/actions.rs` + keybind in `src/config/keybinds.rs`.
- **State border rings** on panes (color/glyph by agent state): `src/ui/panes.rs`.
- **Board overlay** (Vibe Kanban): columns idle/working/blocked/needs-review, one row per session, enter = focus that pane. New `src/ui/board.rs` + state/input following the sidebar pattern.
- **Notify-only-when-blocked** (omnara): notification filter in `src/server/notifications.rs` config (`notify_on = ["blocked"]`) + **exec-bridge**: user-configurable shell command run on notification (hookable to voicebox/KDE Connect/ntfy-self-hosted).
- **Context/cost meters**: extend detection-manifest schema (`src/detect/manifest.rs`) with value-extractor rules (regex capture from screen regions — claude prints context-remaining %); render as a sidebar gauge. Best-effort per agent.

### M2 — Shared memory (hermes-style, the differentiator)
All sessions share agent files → shared memory is the coordination bus.
- **Canonical files**: global `~/.config/shep/memory/USER.md` (cap ~1,375 chars) + per-repo `<repo>/.shep/memory/MEMORY.md` (cap ~2,200) with `§` entries and the write-back protocol text embedded (lift from damon-ade `agent-scaffold.ts`).
- **`shep memory` CLI**: `add|replace|remove` (substring match), `show`, `search`. Enforce caps: **overflow returns an error with usage stats — no auto-compaction** (the hermes forcing function). Agents call it via Bash.
- **Bridges** (per-CLI native injection, generated + `.git/info/exclude`d): claude-code → `@import` lines in repo `CLAUDE.md` (or `.claude/settings.json` auto-memory dir); opencode → `opencode.json` `instructions[]`; codex → concatenated `AGENTS.md` regenerated at pane spawn. Install via herdr's existing `src/integration/` machinery (it already edits agent configs safely).
- **Reflect-on-stop hook** for claude: port damon-ade's `reflect-on-stop.mjs` (stdin JSON → `{decision:"block", reason:<reflection prompt>}`, `stop_hook_active` guard, one turn only, with the do-NOT-capture list) into shep's integration assets.
- **FTS5 history sidecar**: `~/.local/state/shep/history.db` (rusqlite) fed by lifecycle hook events (prompt/stop payloads); `shep memory search` queries it.
- Server surfaces memory usage % per repo → sidebar indicator when >80% (nudge to consolidate).

### M3 — Review & merge flow
Lifecycle verbs (claude-squad): **launch / pause / review / ship**.
- **Review overlay** per workspace: launches a pager pane running `git diff`/`delta` in that worktree (herdr panes are real terminals — cheapest correct diff view), with diff-stats header (files/±lines, Crystal-style) from `git diff --numstat`.
- **Ship action**: merge worktree branch → base, auto-cleanup worktree (extend `src/worktree.rs`, `src/app/worktrees.rs`).
- **Request-changes routing**: from review, type feedback → injected into the *originating* session as a queued prompt (heredoc/paste via existing pane-input API).
- **Dispatch-review**: send the diff to an idle session (or spawn one) with a review prompt; verdict posts back as a notification + board state `needs-review → approved/changes-requested`.
- Task/session states extended server-side (`src/api/schema/`) so board + sidebar show review states.

### M4 — Task queue & dispatch
- **Local queue**: `~/.local/state/shep/tasks.db` (rusqlite). `shep task add "<prompt>" [--repo <path>] [--runtime claude|opencode] [--worktree]`, `list`, `cancel`. States: todo/running/blocked/needs-review/done.
- **Dispatch**: spawn pane in target repo/worktree with the runtime's launch command + heredoc prompt injection (port damon-ade `agent-command.ts` table + `buildHeredoc`).
- **Auto-dispatch toggle**: on agent-state `done|idle` + non-empty queue → dispatch next (event-hub subscriber in server).
- **Watchers**: `~/.config/shep/watchers.toml` (`dir → prompt template`, `{file}` substitution) via `notify` crate → enqueue task (damon-ade `watcher.ts`).
- Board overlay (M1) gains task columns — becomes the full kanban.

### M5 — Flagship polish (as time/value allows, in order)
1. **Model/runtime bar**: relaunch same worktree under a different runtime/model, incl. local models via `ANTHROPIC_BASE_URL=http://127.0.0.1:7860/mlx/v1` → claude-code on odysseus-mlx.
2. **Tab-to-queue input** (codex): queue a prompt to a busy session, fires when it goes idle.
3. **Approach comparison**: run one task in 2 sessions/worktrees, side-by-side (width-adaptive, stacked when narrow) diff compare.
4. **Session recording**: asciinema-format pane recording + replay.
5. **Native width-adaptive diff widget** (replace pager-pane review with first-class ratatui diff, opencode `auto|stacked`).

## Execution model (fable-orchestration)

Architect+delegate: this plan is the architecture; execution runs as **Opus 4.8 subagents** (Alex's standing rule — Fable only in the main loop). Per milestone: dispatch implementation to Opus agent(s) — parallel only across independent modules, worktree-isolated when concurrent — then a fresh-eyes Opus verify pass against the milestone's verification list before moving on. Fable reviews diffs and makes judgment calls between milestones. Land + push each milestone (dual-push) — trunk convention.

## Risks / gotchas

- Rename is load-bearing: `HERDR_*` env is read by hook scripts already installed in live agent configs — keep fallback reads until shep reinstalls its own integrations.
- Upstream plugin/API bugs (#893 plugin registry lost on live-handoff, #1033 foreground takeover locks hook authority, #803 nix-wrapped claude undetected) sit near our seams — patch in-tree as encountered (we've diverged).
- 190k-LOC onboarding cost: every subagent gets pointed at upstream `AGENTS.md` + the module map in `docs/VISION.md`; stay out of the VT core.
- AGPL: fine for a personal/public tool; noted if it ever becomes a hosted product.
- claude-code auto-memory / `@import` behavior should be verified live in M2 before building on it (damon-ade docs claim it; confirm against current claude-code).

## Verification (each milestone gates on this)

1. `cargo build --release` + `just test` (nextest) green.
2. Live smoke on this box: `shep` server up → spawn real claude-code + opencode panes → detach/reattach (mosh-sim: kill client, reattach).
3. M1: force a blocked state (claude permission prompt) → sidebar reorders, ring shows, hotkey jumps, exec-bridge fires once.
4. M2: fresh claude session sees memory in context (ask it "what do you know about me"); `shep memory add` past cap → error; stop a session → reflection turn fires exactly once.
5. M3: dirty worktree → review overlay shows diff + stats; ship merges + cleans up; request-changes lands in the origin session's input.
6. M4: `shep task add` → auto-dispatch on idle → completion flips board state; watcher file-drop enqueues.

## Status — 2026-07-09

M0–M4 are landed and green (`just check`, ~2,650 tests). From M5, tab-to-queue
is landed (`shep agent send --queue`, pane "Queue prompt..." menu).

**Model routing (M5.1) is config, not code**: `[tasks]` launch commands are
free-form shell, so local-model dispatch is

```toml
[tasks]
claude_command = "ANTHROPIC_BASE_URL=http://127.0.0.1:7860/mlx/v1 claude"
```

(odysseus-mlx gateway). Per-task model choice = a second config profile or an
inline env prefix; a first-class model bar remains open.

**Deferred** (open, in rough value order):
- dispatch-review to a second agent (M3 stretch)
- board task/review columns (board currently shows agent states only)
- approach comparison (two worktrees side-by-side)
- session recording (asciinema)
- native width-adaptive diff widget (review currently uses a pager pane)
- queued-input indicator in sidebar/board
