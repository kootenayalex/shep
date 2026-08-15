# Design language

Shep has two surfaces — the terminal UI and the Android companion — and they are
meant to read as one product. That only works if a colour means the same thing in
both. This file is the contract.

It is a **document, not an API**. Per the runtime/client guardrail in
`CLAUDE.md`, colours and glyphs are client presentation and must not enter the
JSON API or the wire protocol. Both surfaces implement this table independently
and each pins it with a test, so drift shows up as a failing test rather than as
a phone that disagrees with the desktop.

- Desktop: `src/ui/status.rs` (`state_appearance`), `Palette` in `src/app/state.rs`.
- Companion: `ui/theme/ShepSemantic.kt`, `ShepPalette` in `ui/theme/Color.kt`.

## Tiers — what a colour means

Every colour has one job. If you need a new distinction, find the tier it belongs
to rather than reaching for an unused token.

| token | tier | used for |
|---|---|---|
| `red` | **stop** | blocked agents, destructive actions. Nothing else, ever. |
| `peach` | **warning** | behind upstream, memory pressure, changes requested. |
| `yellow` | **working** | an agent is running. |
| `blue` | **done, unseen** | finished, and you have not looked yet. |
| `green` | **settled** | idle, approved, ahead of upstream. |
| `teal` | **queued** | input waiting for an agent to go idle. |
| `mauve` | **review** | review requested; also branch identity. |
| `accent` (copper) | **focus** | selection, focused pane, the active tab. Never a state. |
| `overlay0` | **absent** | unknown state, dim metadata, disabled affordances. |

Red carries the Von Restorff load: exactly one thing on a screen should be red,
and it should be the thing that needs you. That is why "git behind" and "changes
requested" are peach and not red, and why `accent` never doubles as a state —
a selected row and a working agent must not share ink.

## Agent states

Five states, and `seen` splits idle in two: an agent that finished while you were
away is not the same as one you have already looked at.

| state | label | glyph | colour |
|---|---|---|---|
| blocked | `blocked` | `◉` | red |
| working | `working` | braille spinner `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` | yellow |
| idle, unseen | `done` | `●` | blue |
| idle, seen | `idle` | `○` | green |
| unknown | `idle` | `·` | overlay0 |

**One glyph per state, everywhere.** Sidebar rows, board cards, the navigator,
the phone's board — all of them. Colour is never the only channel that
distinguishes two states, because a colour-blind reader and a monochrome themed
icon both have to work. The glyph shapes carry the same story as the colours:
filled-with-a-ring is stopped, moving is working, filled is finished, hollow is
settled, a dot is nothing known.

### The one deliberate divergence: the working spinner

The desktop spins braille `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`; the companion spins half-filled circles
`◐◓◑◒`. This is not drift, and it is not negotiable in either direction.

A terminal cell must be exactly one column wide. `◐` and `◑` are
East-Asian-Ambiguous while `◓` and `◒` are Neutral, so on a terminal configured
for wide ambiguous glyphs that set would change width *every frame* and shift the
whole row. Braille is uniformly Neutral, so it is the only correct choice there.

A phone has no column grid, and braille loses badly at phone sizes: at 13sp those
dots render as a scatter of specks next to a solid `●`, which made the one mark
that says "this is alive" the faintest thing on the card. A half-filled circle
carries the same optical weight as `●` and `○`, so the five states read as one
family — ring, filling, full, empty, speck.

What is shared is the meaning and the cadence: working animates, in yellow, at
roughly two-thirds of a second per turn. How it animates belongs to the medium.

## Task-queue states

A task is a different thing from an agent, so it gets its own row of the table
— but the *shapes* are the same, because they mean the same things.

| state | label | glyph | colour |
|---|---|---|---|
| blocked | `blocked` | `◉` | red |
| running | `running` | spinner | yellow |
| done | `done` | `●` | green |
| todo | `todo` | `○` | overlay1 |
| cancelled | `cancelled` | `·` | overlay0 |

Only "done" takes a different tier from the agent table: settled green rather
than done-unseen blue, because a task has no notion of your having looked at it.
Todo is `overlay1` — dimmer than running, brighter than cancelled — because the
queue is a backlog and the eye should land on what is moving.

The labels are the wire format (`TaskState::as_str`), pinned by a test on the
desktop side, so a typo cannot rename a state in the UI while the server keeps
calling it something else.

- Desktop: `task_appearance` in `src/ui/status.rs`.
- Companion: `ShepSemantic.task` in `ui/theme/ShepSemantic.kt`.

## Badges

Badges sit beside a name and answer a different question from state.

| badge | glyph | colour | meaning |
|---|---|---|---|
| needs review | `◆` | mauve | changes are waiting for you to look |
| changes requested | `↺` | peach | you sent it back |
| approved | `✓` | green | you said yes |
| queued input | `⇥N` | teal | N prompts waiting for idle |
| git ahead | `↑N` | green | commits to push |
| git behind | `↓N` | peach | commits to pull |
| memory pressure | `mem NN%` | peach | at or over 80% of the cap |
| worktree | `⑂` (phone) / `· worktree` (desktop) | accent | a linked worktree, not the main checkout |

`✓` means approved and nothing else. It used to be idle's glyph too, which is why
idle is now `○`.

The worktree badge is the one entry that reads differently on the two surfaces,
and deliberately: the phone puts it in a card header beside an id, where a
one-column glyph is right, and the desktop puts it in a prose meta line —
`workmayt · claude · worktree · dispatched · 4m` — where a glyph among words
reads as a typo. Same fact, different sentence.

Every other non-ASCII mark the desktop draws lives in `src/ui/glyphs.rs`, with
a test pinning each one to a single column. A mark that measures two shifts
everything after it on the row.

## Rules that have bitten us

1. **A glyph gets one meaning.** `✓` was idle *and* approved; `⇥` is queued input
   only; `◉` is blocked only.
2. **A colour gets one tier.** `yellow` was working *and* needs-review, which is
   why needs-review moved to mauve. `teal` was queued *and* done, which is why
   done moved to blue.
3. **Never distinguish by colour alone.** Three board-card states once shared a
   filled `●` and differed only in hue, and the task queue did it for all five
   of its states on both surfaces at once.
4. **The palette doc-comments in `src/app/state.rs` are the tiebreaker.** When
   the two surfaces disagreed about `working` and `done`, those comments already
   said yellow and blue; both implementations had drifted from them.
