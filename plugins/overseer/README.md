# overseer

A shep plugin that narrates the session on a one-screen board and proposes
docket items. It advises; it never acts.

```bash
shep plugin link plugins/overseer          # from a shep checkout
shep plugin pane open --plugin overseer --entrypoint board
shep plugin action invoke tick --plugin overseer
```

Every `pane.agent_status_changed` and `pane.exited` event runs one tick:
`shep doctor --json`, `session.overview` over the socket, `shep docket list
--json`, written to `<plugin state dir>/situation.{md,json}`, then a
`BOARD.md` of at most 40 lines — agents with their state ages, blocked agents
first, health fails and warns, docket due/overdue and inbox. That is what you
get with nothing configured, and it is already useful.

## A brain, optionally

```toml
# config.toml
[plugins.overseer]
runtime = "claude"          # any runtime `shep runtime list` marks headless
```

or `SHEP_OVERSEER_RUNTIME=claude` in the server's environment. With a brain,
at most once every ten minutes the tick hands the situation to
`shep runtime ask <runtime>` together with the hard rules below and asks for
exactly two things: a board (≤ 40 lines) and a JSON list of proposed inbox
items (`title`, `source`, `notes`). The board replaces the deterministic one;
each proposal whose `source` is not already in the docket becomes
`shep docket add … --kind captured` — inbox only, no date, no repeat, at most
five per tick. Events inside the ten-minute window refresh the situation and
leave the brain's board standing. A brain that fails or answers badly costs
nothing: the deterministic board stands.

The prompt goes on stdin, so the runtime never sees it on argv. Point
`runtime` at a local model behind a `[runtimes.<name>] headless_argv` if you
would rather not send a session summary anywhere.

## What it never does

- Never answers for an agent: no keys, no text, nothing to any pane's input.
- Never touches the server: no stop, no handoff, no reload, no signals.
- Never nudges or queues prompts.
- Never promotes, dates or repeats a docket item; capture proposes, you
  dispose.

`tests/plugin_overseer.rs` greps the script for the forbidden verbs, so the
rules hold by construction, not by discipline.

## Files

- `shep-plugin.toml` — manifest: two event hooks, the `tick` action, the
  `board` pane.
- `overseer-tick` — the tick (Python 3, stdlib only).
- `overseer-board` — the pane: redraws `BOARD.md` every five seconds.
- state, under `SHEP_PLUGIN_STATE_DIR`: `situation.md`, `situation.json`,
  `BOARD.md`, `BOARD.md.source`, `last-brain`, `journal.log`.

Alex's vault skill `/shep-overseer` is the richer, claude-only variant of the
same charter (paging, nudging, memory-file capture); this plugin is the part
of it that is safe to hand to a stranger.
