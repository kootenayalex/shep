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
`BOARD.md` of at most 10 lines: a header (`OVERSEER · <at> · <source>`) and a
short read of the room in prose — who is blocked and waiting on you first,
then what the docket owes, then health. The TUI's overseer board (`ctrl+alt+b`)
draws the agent table, the docket and the health strip itself, so the board
carries the narrative rather than the lists. That is what you get with
nothing configured, and it is already useful.

## A brain, optionally

```toml
# config.toml
[plugins.overseer]
runtime = "claude"          # any runtime `shep runtime list` marks headless
session_argv = ["claude", "--resume", "{session_id}"]   # optional; see below
session_cwd = "~/vault/agents/shep-overseer"   # both faces run here; default: the state dir
```

or `SHEP_OVERSEER_RUNTIME=claude` in the server's environment. With a brain,
at most once every ten minutes the tick hands the situation to
`shep runtime ask <runtime>` together with the hard rules below and asks for
exactly one thing: a board of at most 30 lines, one `## <agent>` section per
agent (blocked first; an agent that shares its name with another is
`## name · group`) with a sentence or two on what the state word cannot say,
then `## room` for what cuts across them — the docket, health, where to look
first. The board replaces the deterministic one, which has the same shape.
The brain never writes to the docket: what is owed is said on the board, and
the person keeps their own list. Events inside the ten-minute window refresh
the situation and leave the brain's board standing. A brain that fails or
answers badly costs nothing: the deterministic board stands.

The prompt goes on stdin, so the runtime never sees it on argv. Point
`runtime` at a local model behind a `[runtimes.<name>] headless_argv` if you
would rather not send a session summary anywhere.

The same runtime answers the board's `chat` (`tab` on the overseer board):
the TUI hands it the hard rules below, `situation.md` and the question, and
shows the answer. The thread is `chat.jsonl` in the state dir, one
`{"at", "role", "text"}` line per turn (`role` is `you` or `overseer`),
appended by the TUI; the plugin does not write it. The chat obeys the same
rules as the narrative — an answer is words on the board, never keys into a
pane.

## One session, two faces

The board's chat and the session pane are the same claude conversation.
The first question (or the first opening of the pane) mints a v4 uuid into
`session-id` in the state dir; every headless question runs
`claude -p … --session-id <id>` until something has begun the conversation
and `--resume <id>` after (`session-started` is the marker, written by the
first answer that succeeds or by opening the pane), and the pane runs
`claude --resume <id>` (or `--session-id <id>` when nothing has). Whatever
you said in the pane, the chat knows; whatever the chat answered, the pane
remembers. Because claude keys conversations by working directory, both
faces run in one place: `session_cwd` when it exists, else the state dir
(shep hands the pane `SHEP_OVERSEER_SESSION_CWD`). The chat's prompt
carries no replay of earlier turns — the session is the memory — and
re-sends the situation each time as current. A resume the runtime refuses
(its transcript gone) drops the marker and starts the same id over, once.

This holds for any runtime whose headless recipe declares
`session_new_args` and `session_resume_args` (`claude` does; see the
manifest's `[headless]`). A runtime without them gets today's stateless
chat instead: a fresh process per question with the last twelve turns
replayed in the prompt, and an unrelated session pane. A
`[runtimes.<name>] headless_argv` override shares nothing unless it also
sets `headless_session_new_args` / `headless_session_resume_args`.

Ticks are stateless either way: `shep runtime ask` never touches the
session. If the pane is open while you send a chat line, both append to the
same transcript; do not type in both at the same instant.

## The session

The board's `▸ open the overseer's session` button opens the `session` pane:
`overseer-session` execs `session_argv` — or `SHEP_OVERSEER_SESSION_ARGV`
(shell words) from the server's environment, with `{session_id}` in any
word replaced by the shared id — or, by default, `claude --resume <id>` /
`claude --session-id <id>` (plain `claude` when shep passed no id). It runs
in `SHEP_OVERSEER_SESSION_CWD` when shep set it (the resolution above),
else `session_cwd` (`~` expands; must exist) or the plugin's state dir,
with `SHEP_OVERSEER_SESSION_ID`, `SHEP_OVERSEER_SESSION_RESUME` (`1`/`0`),
`SHEP_OVERSEER_STATE_DIR`, `SHEP_OVERSEER_SITUATION` (`situation.md`),
`SHEP_OVERSEER_BOARD` (`BOARD.md`) and `SHEP_OVERSEER_CHAT` (`chat.jsonl`)
exported, so the agent can read what the tick wrote and what the chat said.
shep keeps the pane in a system workspace that no list ever shows; the
titlebar reads `✦ overseer › session` while it is up. The launcher makes no
shep calls, so the forbidden-verb test covers it too.

## What it never does

- Never answers for an agent: no keys, no text, nothing to any pane's input.
- Never touches the server: no stop, no handoff, no reload, no signals.
- Never nudges or queues prompts.
- Never writes to the docket — no captured items, no promotion, no dates.
  What is owed is said on the board; the docket is yours.

`tests/plugin_overseer.rs` greps the script for the forbidden verbs, so the
rules hold by construction, not by discipline.

## Files

- `shep-plugin.toml` — manifest: two event hooks, the `tick` action, the
  `board` and `session` panes.
- `overseer-tick` — the tick (Python 3, stdlib only).
- `overseer-board` — the pane: redraws `BOARD.md` every five seconds.
- `overseer-session` — the session launcher (Python 3, stdlib only): execs
  the configured agent with the state files in its environment.
- state, under `SHEP_PLUGIN_STATE_DIR`: `situation.md`, `situation.json`,
  `BOARD.md`, `BOARD.md.source`, `last-brain`, `journal.log`, the board's
  `chat.jsonl` (written by the TUI, read by anyone), and the shared
  session's `session-id` and `session-started` (written by the TUI).

Alex's vault skill `/shep-overseer` is the richer, claude-only variant of the
same charter (paging, nudging, dispatch); this plugin is the part
of it that is safe to hand to a stranger.
