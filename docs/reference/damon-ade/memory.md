# Agent memory

Every ADE agent has a persistent, self-curated memory. It is what makes an agent an identity rather than a chat window: the agent reads its memory at the start of every session and writes back to it as it learns, so it accumulates knowledge about you and your project over time.

The system is adapted from the [Hermes agent](https://github.com/NousResearch/hermes-agent) (MIT). ADE copies Hermes' *shape* — a bounded, file-backed memory the agent maintains itself — but replaces Hermes' mechanism (a custom memory tool inside a bespoke agent loop) with each coding CLI's own native context-file feature. ADE never forks or patches a CLI.

## Principles

- **Plain files are the source of truth.** Each agent owns a `memory/` directory of markdown. No database, no daemon, no injected tool.
- **Memory lives outside the git worktree.** It is a sibling of the worktree, so it is never committed to your code and survives branch and worktree changes.
- **Budgets are guidance, not gates.** Memory is injected into every turn and costs tokens forever, so the files carry soft size targets and the agent is told to consolidate rather than grow without bound.
- **One canonical set of files, thin per-runtime bridges.** The same files feed Claude Code, Codex, and OpenCode through small generated bridge files, so switching an agent's runtime never loses its memory.

## The files

Each agent has a home directory containing its worktree, its memory, and its skills:

```
<agent-home>/
├── worktree/            # the git worktree; the CLI's working directory
│   ├── CLAUDE.md        # bridge: Claude Code (generated, git-excluded)
│   ├── opencode.json    # bridge: OpenCode (generated, git-excluded)
│   └── .claude/         # Claude Code settings + reflection hook (generated)
├── memory/              # CANONICAL memory — source of truth, never committed
│   ├── AGENT.md         # persona / operating brief
│   ├── USER.md          # user profile (who you are)
│   ├── MEMORY.md        # the agent's own notes + an index of topic files
│   ├── .writeback-protocol.md   # the maintenance rules (see below)
│   └── memories/        # optional granular topic files, one subject each
├── skills/              # reusable know-how, each a SKILL.md
└── .codex/              # generated Codex bridge (for the Codex runtime)
```

**AGENT.md** is a short identity paragraph — who the agent is, its voice, its role, and its standing preferences. It is seeded once (optionally from the Role you give the agent when you create it) and rarely changes; you own it.

**USER.md** is the profile of you the agent maintains: your name, role, tech preferences, communication style, and hard "always/never" rules. Target size is under ~1,375 characters.

**MEMORY.md** is the agent's own notebook: environment facts, project conventions, tool quirks, and lessons learned, plus a short index pointing to any longer `memories/<topic>.md` files. Target size is under ~2,200 characters for the inline notes; anything longer is offloaded to a topic file with a one-line pointer left behind.

**Skills** are reusable, multi-step procedures the agent writes for itself, each a folder with a `SKILL.md` in [agentskills.io](https://agentskills.io) format. Only a skill's name and one-line description sit in context; its body loads on demand. Skills are for repeatable procedures and class-of-task lessons — not one-off facts, which belong in MEMORY.md.

## The write-back protocol

A protocol file (`.writeback-protocol.md`) is loaded alongside the memory and tells the agent how to maintain it. The key idea, ported from Hermes, is that the instructions for maintaining memory travel *with* the memory surface itself. In summary:

- **When to save** — proactively, without being asked: a stated preference, correction, or personal detail goes to USER.md; a stable fact about your environment, stack, or conventions goes to MEMORY.md. Priority when space is tight: user preferences and corrections, then environment facts, then procedures.
- **When to skip** — trivia, easily re-discovered facts, raw log dumps, task progress, completed-work logs, temporary debugging state, one-off paths. Reusable procedures become a skill, not a memory note.
- **Format** — one fact per bullet, present tense, absolute dates. Never write secrets or tokens.
- **When full** — consolidate rather than append: merge overlapping entries, drop the stalest, then add, all in one edit. A memory that only ever grows becomes bloated and gets ignored.

The agent makes these edits with the ordinary file tools its CLI already has. There is no custom memory tool to learn or configure.

## The reflection loop

Before an agent finishes a session, a session-end reflection prompts it to review the conversation and update its memory and skills so the next session starts smarter — durable facts and preferences into USER.md / MEMORY.md, and any correction to its style or workflow embedded into the skill that governs that class of task. The reflection is deliberately active: a review that changes nothing is usually a missed learning opportunity.

It also carries an explicit do-not-capture list, because some things harden into false constraints if remembered: environment-dependent failures (a missing binary, an unconfigured credential), negative claims about tools ("X is broken" — capture the fix instead), transient errors that resolved on retry, and one-off task narratives.

On the Claude Code runtime this reflection is enforced by a native stop hook: when the agent tries to finish, the hook feeds the reflection prompt back for exactly one review turn (guarded so it never loops), then the agent stops. On Codex and OpenCode the reflection runs by convention at session boundaries, driven by the protocol text.

## How memory reaches each runtime

The canonical files are the same for every runtime; each CLI is pointed at them by a small generated bridge:

| Runtime | Bridge | Mechanism | Native write-back |
|---|---|---|---|
| Claude Code | `CLAUDE.md` + `.claude/settings.json` | `@import` for AGENT.md/USER.md; native auto-memory for MEMORY.md | Yes |
| Codex | `.codex/AGENTS.md` | Concatenated text, regenerated on each launch (Codex can't import) | Driven by the protocol |
| OpenCode | `opencode.json` | An `instructions` array referencing the canonical files | Driven by the protocol |

Claude Code and OpenCode reference the live canonical files, so they need no rebuild. Codex has no import syntax, so its bridge is a concatenation of the memory files regenerated from the canonical source every time a Codex session launches. Either way, the agent always edits the canonical `memory/*.md` files — the bridges are derived, never hand-edited.

## Where to see it

The **Agent Files** panel in an agent's workspace lists this whole surface — the Memory files, the Skills, and the worktree bridge — and only shows files that exist, so it visibly grows as the agent learns. Click any file to open it in a viewer tab.
