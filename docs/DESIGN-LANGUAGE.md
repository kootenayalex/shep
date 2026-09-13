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
- Companion: none — the phone never had a task screen, and the queue is retired.

## Docket states

A docket item is a reminder, not a process, so its table is short and borrows
the task table's shapes: hollow for undecided, filled for live, green when
settled. The one new mark is `!`.

| state | label | glyph | colour |
|---|---|---|---|
| inbox | `inbox` | `○` | overlay1 |
| open, overdue | `overdue` | `!` | peach, bold |
| open, due today | `due today` | `●` | yellow |
| open, later or undated | `open` | `●` | overlay1 |
| done | `done` | `●` | green |
| discarded | `discarded` | `·` | overlay0 (never drawn on the board) |

**Overdue is a warning, not a stop.** Peach, because red is what a blocked
agent gets and an item three days late is a nag. And never colour alone: the
gutter glyph *is* the `!`, the card's date row says `overdue 3d`, and the due
lane's heading counts them (`due 2 !1`). Due today takes the working tier's
yellow — it is the thing happening now — and lights the date row the same way.

- Desktop: `docket_appearance` in `src/ui/status.rs`.
- Companion: `ShepSemantic.docket` in `ui/theme/ShepSemantic.kt`.

### The docket card

Four rows at most, and only the rows the item has something to put on:

| row | content | when |
|---|---|---|
| 1 | gutter glyph · `#id` · title (elided) | always |
| 2 | kind `·` date `·` repeat | always; date is `overdue 3d` / `due today` / `in 5d` / `—`, repeat only when there is one |
| 3 | source: `memory.md:12` for a file, `pane p3` for a session | when the item has a source |
| 4 | first line of the notes, italic | when the item has notes |

The id leads the title because the id is what the CLI verbs take. Stacked on
a narrow terminal every card is its first two rows, so the due lane still
lands on an 80×24 screen; the detail screen (`i`) has the rest, with the
source unabridged and the notes whole.

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
| memory pressure | `mem NN%` | peach | at or over 80% of the cap; the board's, not the sidebar's |
| context window | `███▍░░ NN%` | peach at or over 80%, overlay0 below | how full the agent's context is |
| plan mode | `plan` | mauve | the agent is planning, not editing |
| bypassing permissions | `bypass` | peach | the agent is not asking before acting |
| churn | `+N/-N` | green / red | lines the session has added and removed |
| worktree | `⑂` (phone) / `· worktree` (desktop) | accent | a linked worktree, not the main checkout |

The permission-mode badges are a warning, never a stop: peach, because red is
what a blocked agent gets. The ordinary modes say nothing worth a glance and so
draw nothing. Both, like the churn and the card's summary line, come from the
agent's own session file (`src/session_facts/`) — display hints, never
detection evidence.

`✓` means approved and nothing else. It used to be idle's glyph too, which is why
idle is now `○`.

The context gauge is a meter, not a state, so it takes the warning tier and
nothing hotter: it used to go yellow at 60 and red at 85, which spent the
working and stop tiers on a number. One gauge, drawn by `src/ui/gauge.rs`,
wherever a context percentage appears.

- Companion: `ShepSemantic.gauge` in `ui/theme/ShepSemantic.kt`, drawn by
  `ContextGauge`. It carried the 60/85 ladder until the board landed.

The overseer's mark is `✦`, mauve, wherever the overseer speaks. Not `◆`: that
is the needs-review badge, and one glyph carries one meaning. `≡` is the global
menu, `⚠` a health finding (peach: a warning), and `«`/`»` fold the sidebar
away and back — all pinned in `src/ui/glyphs.rs`.

- Companion: `ShepSemantic.overseer` and `ShepSemantic.health` in
  `ui/theme/ShepSemantic.kt`.

The worktree badge is the one entry that reads differently on the two surfaces,
and deliberately: the phone puts it in a card header beside an id, where a
one-column glyph is right, and the desktop puts it in a prose meta line —
`workmayt · claude · worktree · dispatched · 4m` — where a glyph among words
reads as a typo. Same fact, different sentence.

Every other non-ASCII mark the desktop draws lives in `src/ui/glyphs.rs`, with
a test pinning each one to a single column. A mark that measures two shifts
everything after it on the row.

## Layout

**A card's trailing facts pin to its right edge**, one column in. Location, age
and context gauge each end on the same column, so a lane of cards reads as three
columns rather than a ragged edge — and so does the phone's card, which lays the
same line out with a weighted spacer. Trailing them after whatever text came
before only *looked* aligned when that text happened to be long enough to
truncate. The gauge's number is right-aligned in its own three columns for the
same reason: an unpadded `100%` drags its bar a column left of every other.

**A strip drops whole facts, never part of one.** Terminal clipping is a
guillotine: the board's dashboard used to end `·  3` — the head of
`3 ws · 3 tabs · 5 panes`, reading as a count of something never named. Facts go
in the order a glance wants them and the line stops at the first one that does
not fit, so what remains is a prefix of a known order rather than a gap-toothed
subset of it.

**The titlebar's right slot is a ladder.** The state tally (`◉ 1  ⠹ 2  ● 1
○ 1`, one fact per state with the agent table's glyph and colour, zero counts
absent) and the `desktop | board` pill share the slot with `group › agent` in
the centre. Rungs, walked until everything fits: full, drop idle, drop done,
shorten the pill to `desk`, drop working, drop blocked, the pill alone. `update
ready` leads every rung but the last. `src/ui/chrome.rs::titlebar_layout` is
the one function that lays it out, and the mouse asks the same function, so
the pill is clickable exactly where it is drawn.

- Companion: `StateTally` in `ui/components/Chrome.kt`, over
  `ShepSemantic.TALLY_ORDER`. The phone has no ladder — the title is in its
  own header row — so the tally simply elides.

**The pill is the one place `accent` paints a background.** Its lit half —
the view you are on — is accent behind panel-bg bold text; the unlit half is
surface1 behind subtext. Focus tier, because the pill is a selection between
two views and not a state. The unlit `board` half carries ` N` in teal (the
queued tier: proposals waiting for you) when the overseer has captured any.

- Companion: `ViewPill` in `ui/components/Chrome.kt`, and the same ground a
  selected `ShepChip` already paints — the pill is two halves of one chip, not
  a second colour rule. It is also the phone's *only* way between the two
  views: `board` is a hint-bar tab and `agents` is not.

**The board is a screen, not a panel.** It takes the desktop's place under
the same titlebar and hint bar — no border, no title, no footer of its own —
because a sibling of the desktop is not something drawn over it. Its regions
are headings in bold text with a count beside them in overlay0 (`needs you 2`,
`agents 5`, `docket  due 2 !1  ·  inbox 5`, `health`), and the overseer's own
regions carry its mark: `✦ read of the room  ·  hh:mm` in mauve, then
`proposals`, whose heading is teal — the queued tier, because a proposal is
waiting for you and nothing has happened yet — and `chat`. Above 120 columns
the left column is 74 wide with a surface1 rule beside it; below, the right
column stacks under the left. Each region is a prefix of its rows: when the
height runs out a region shows its first N entries and the rest are simply
not there, never an ellipsis row. The selected row is `▌` in accent on
surface0 across the full width, the same selection every other list draws.

**The overseer's session is a workspace with no row.** While it is active
the titlebar centre reads `✦ overseer › session` — the mark and name in mauve
where a group's name would be in text — and the sidebar highlights nothing,
because the session is not a group: a system workspace is skipped by every
list (sidebar, board, navigator, picker, `session.overview`) and reached only
through the board's button.

**The overseer strip is one row and no buttons.** `✦ overseer · <first
sentence> · hh:mm` on surface0, the mark in mauve, the time pinned to the right
edge in overlay0. Narrowing drops the word `overseer` before it elides the
sentence — the mark still says who is speaking. It is the overseer's only
voice outside the board, and clicking anywhere on it opens the board.

**The sidebar says who and how; the pane title says where and how full.**
A group row is its glyph, name and badges, and under it the branch with
`↑N`/`↓N` — the branch truncates, the two badges come off whole. An agent row
is its glyph, name and state word. That is the whole fact set: the event age
and `mem NN%` were host facts on a group row and live on the board, and the
bare `72%` an agent row used to trail was a meter with no bar on the one
surface that could least afford the columns. At twenty content columns and
under the tree goes narrow — one row per group, glyph and name per agent —
and the glyph and its colour carry the state on their own, as they always
could. The header row is ` groups … grouped ≡ «`: the sort toggle from a
26-column sidebar up, then the global menu and the collapse toggle, both
mouse-only; the footer is one `+ new group` button.

The pane title is ` agent · branch ` at the left of the top border and
` cwd · <gauge> NN% ` pinned to its right, the branch in mauve. Its ladder,
walked until everything fits: the directory truncates from the front to a
floor, then comes off whole, then the gauge goes, then the branch shrinks to
a floor and goes, and the name is last to truncate. The directory yields to a
short branch — a place is not a line of work — but the gauge outranks the
branch, because the group row beside a narrow pane already names the branch
and the gauge is nowhere else. There is no state word: the border ring is
the state's colour and the sidebar row says the word, and the title saying
it a third time cost the columns the branch has now.

**A layout collapses on its own threshold, not the app's.** The board is four
columns where the rest of shep is one, so it stacks at four times the width —
on a standard 80×24 its lanes were 20 columns and every card had elided its
agent's name to nothing.

**Too small to draw is a sentence, not a blank screen.** Five modals guarded on
a hardcoded width and rendered nothing below it, which from the outside is
indistinguishable from a keybinding that does not exist.

## Affordances without a mouse

`mouse_capture = false` is a supported configuration, and in it shep receives no
mouse events at all. **Anything that can only be clicked is not drawn** when it
cannot be clicked: the tab-scroll arrows, the `+` new-tab button, the sidebar's
`+ new group` footer, and the expanded sidebar's `≡` menu and `«` collapse
glyphs on its header row.

Two carve-outs, both because the mark is doing a second job:

- The **collapsed** sidebar's `»` stays. It is the only remaining evidence that a
  sidebar exists, and it is where the attention badge lights up.
- A **hint** names the keys first and the wheel only when there is one. Three
  scrollable modals advertised `wheel ↑↓` — two of them advertised nothing else —
  so without a mouse their footers named the one control that does nothing and
  stayed silent about `jk/↑↓`, `pgup/pgdn`, `home` and `end`, all of which have
  always worked.

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
5. **A rule that reads only the literal form is not a rule.** The first version
   of the "every mark is named once" test scanned for `▌` and waved through the
   six sites that had written `"\u{258c}"`. It now reads both spellings — and
   writing the escape to get past it is not a workaround, it is the thing the
   test exists to catch.
