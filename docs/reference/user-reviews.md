# shep — user reviews

> **These reviews are synthetic.** The people are constructed; the facts they react to are not.
> Every concrete claim below was verified against `master` `d3dc696` (2026-09-08) on the
> machine shep runs on — release counts, manifest directories, `shep memory status`, row
> counts in `tasks.db` and `history.db`, the contents of `README.md` and `SKILL.md`. The point
> of the exercise is to get outside the audience of one: a tool built for its author cannot
> see the things that only strangers trip over. Do not cite this file as user feedback.
>
> Complaints are tagged **[shep]** where they belong to this fork and **[herdr]** where they
> are inherited from upstream, so it is clear which ones are actionable here.

---

## ★☆☆☆☆ — "I installed a different program"

**orbital_mechanic** · found the repo through a link, wanted to try it

I read the README twice before I understood what had happened to me.

The repo is `kootenayalex/shep`. The README says shep, the binary says shep, the docs link
says `herdr.dev`. I ran the install command at the top of the README —
`curl -fsSL https://herdr.dev/install.sh | sh` — and got a working program called herdr. Not
shep. Herdr. Different project, different author, different GitHub org, and none of the
things the repo had just spent five paragraphs telling me about. No memory. No task queue.
No phone app. I spent forty minutes convinced I had misconfigured something before I checked
`shep --version` against the upstream release page and realised the install script in this
repo's README does not install this repo.

There are **zero releases**. There is no binary anywhere. `brew install shep` — also in the
README — installs upstream. `shep update`, which the help text advertises, is disabled in
this fork. The badges at the top of the README count *someone else's* stars and downloads.

So the actual install path is: clone it, have a Rust toolchain, have `just`, have
`cargo-nextest`, have **JDK 17 and the Android SDK** because the check target hard-fails
without them, and on macOS have a zig `xcrun` shim and a code-signing identity or your
permission grants evaporate on every rebuild. For a terminal multiplexer.

I am not rating the software. I never ran the software. I am rating the front door, and the
front door opens onto a different building.

*— what they touched: `README.md`, `SKILL.md`, `gh release list`, `shep --help`* · **[shep]**

---

## ★★☆☆☆ — "It detects my agent and then has nothing to say about it"

**K. Nwosu** · runs Codex and Gemini CLI, one repo, three panes

On my setup, the board is a nicer sidebar. That is the review. The rest is why.

The detection is not the problem — the detection is the good part. There are eighteen
manifests in here, codex and gemini and copilot and cursor and amp and droid and kiro, and
the state scraping is better than it has any right to be given that it is reading a screen.
Blocked means blocked. That works.

Everything this fork was built *on top of* that detection is a Claude adapter. Session facts — the thing that puts
the agent's own summary and cost on the card — reads one manifest file, `claude.toml`, and
there is not a second one. `pane.transcript`, which powers the whole recorded-output view in
the phone app, is Claude-only by design and says so. The task queue accepts `claude` or
`opencode`. The memory bridges know claude, opencode, codex.

So on my setup the board is a nicer sidebar. My cards have a working directory and a state
dot where a Claude user's card has a title, a permission mode, a diff stat and a running
cost. I get the chassis and none of the interior.

The manifests are TOML and overridable from the config dir, which suggests the design
*intends* to generalise. Nobody has proved it yet, because nobody has written the second one.
Write a `codex.toml` for session facts and half this review disappears.

*— what they touched: `src/detect/manifests/` (18), `src/session_facts/manifests/` (1),
`pane.transcript`, the board* · **[shep]**

---

## ★★☆☆☆ — "Architecturally single-user, and that is not a gap you can patch"

**Priya R.** · engineering manager, four-person platform team

We evaluated this for a shared dev box. It is not a fit, and I want to be precise about why,
because the reason is structural rather than a missing feature.

One server process owns every pane. That is the whole design — it is what makes detach and
reattach work, it is what makes the socket API coherent, and it is why the session survives a
dropped SSH connection. It also means there is exactly one blast radius. Restarting it kills
every agent, including the one issuing the restart. There is a live-handoff path that avoids
that and it is impressively engineered, but it is a mitigation for a property, not a fix.

What we needed and did not find: any concept of a second user. No roles, no per-agent
ownership, no audit trail of who approved what. The bridge that the phone app talks to holds
**one bearer token** — not one per device, one total. Revoking a lost phone means rotating the
token and re-pairing everything. There is nothing to point a compliance review at.

Add: AGPL-3.0, no tagged releases to pin a deployment to, and documentation that describes a
different program. Those are all solvable. The single-owner server is not, at least not
cheaply, and I would rather say so than pretend the roadmap could absorb it.

For one senior engineer running their own agents on their own machine, I suspect this is
excellent. That is clearly who it is for.

*— what they touched: the server model, `live-handoff`, `shep bridge`, LICENSE* ·
**[herdr]** for the server model, **[shep]** for the token and release story

---

## ★★☆☆☆ — "I wanted to contribute and could not find the door either"

**hexbyte** · writes Rust, likes TUIs, fixes other people's papercuts for fun

The code is nice. I read a lot of it. The detection layer is clean, the snapshot test harness
is a genuinely good idea, and whoever wrote the pane geometry code was thinking hard. I came
in wanting to send a patch.

Then I tried to work out how one does that here and the ground gave way.

`SKILL.md` — the file that tells an agent how to drive this thing — still declares
`name: herdr` and refuses to run unless `HERDR_ENV=1`. The CI workflows are upstream's
contributor-gating machinery: `approve-contributor.yml`, `issue-gate.yml`, a bot that labels
issues for someone else's release train. `AGENTS.md` has whole sections marked LEGACY UPSTREAM
CONTEXT describing a maintainer's personal workflow and then telling me to skip them. Issues
are off. Zero stars. Nothing anywhere states plainly *this is a fork of ogulcancelik/herdr,
here is what diverged, here is what I take patches on.*

Meanwhile four commands that only exist in this fork — `memory`, `task`, `bridge`, and the
`group` alias — are dispatched by hand in `cli.rs` and **absent from the clap spec**. They do
not appear in the generated help tree. They do not appear in shell completions. The only
place `shep memory` is documented for a human is a vision document.

I am not annoyed. I am deflated, which is worse. Say what the fork is, put the fork's commands
in the fork's help, and someone like me can be useful to you in an afternoon.

*— what they touched: `SKILL.md`, `AGENTS.md`, `.github/workflows/`, `src/cli/spec.rs`,
`docs/next/website/.../cli-reference.mdx`* · **[shep]**

---

## ★★☆☆☆ — "The best idea in this project is the one part that isn't running"

**D. Ferreira** · has opinions about agent memory, came here specifically for M2

I need to start by praising the thing I am about to give two stars, because the idea deserves
better than its execution.

shep's memory has a **hard character cap** — 1,375 for the user profile, 2,200 per repo — and
when you exceed it, it *errors*. It does not summarise. It does not compact. It hands back a
usage string and makes a human decide what to drop. Almost every memory system I have used
treats forgetting as a background process to be automated away, and quietly rots into a pile
of stale confident nonsense. This one treats forgetting as the *point*, and makes the cap a
forcing function. That is a real idea. It is, as far as I can tell, the reason this fork
exists.

Now the part that hurts. I checked the author's own machine.

`user … 0/1375 chars, 0 entries`.

The flagship file — the one that is supposed to know who the human is, the one wired into
every agent's context on this box — has never held a single fact. The repo file is half full
and has drifted into being a build-gotchas list: don't `cp` over the binary, run the check
target before committing. Useful. Not memory. The searchable history sidecar holds **53 rows**
across two months of continuous multi-agent work, and `shep memory search` returns nothing for
a word that is sitting in the memory file's own header — because headers are not entries and
the corpus is a rounding error.

The status command now reports `hooks ok`, which is somehow the bleakest part. It is not
broken any more. It is working, and still producing nothing, which means the plumbing was
never the real problem. The write protocol asks agents to save proactively and they mostly
don't; nothing measures whether they do; nothing surfaces that they haven't.

Ship the forcing function with something that *enforces* it — a nudge when the profile is
empty after N sessions, a count of what got saved this week, anything that closes the loop.
Right now the most interesting design decision in the project is a file full of instructions
and one section marker.

*— what they touched: `shep memory status`/`search`, `USER.md`, `.shep/memory/MEMORY.md`,
`history.db`, `src/memory/`* · **[shep]**

---

## ★★★☆☆ — "Good multiplexer wearing a cockpit it won't let me take off"

**Gord** · tmux since 2009, uses one agent occasionally and grudgingly

I don't have five agents. I have one agent and nine shells — logs, a build, a psql, an ssh
to a box that is having a bad week — and this tool has clearly never met me.

Which is a shame, because the multiplexer underneath it is good. Panes, tabs, splits, mouse
*and* prefix keys without either feeling bolted on. Detach and reattach that actually
survives. A real terminal emulator rather than somebody's approximation of one. It reattaches
over SSH properly, without the usual song and dance about nested sessions. If it stopped
there I would be using it and this would be five stars.

It does not stop there. The board wants to be my front door. Escape in a pane throws me back
to it. Every surface is organised around which of my agents needs attention, which is a
magnificent answer to a question I ask roughly twice a week. And shells are second-class
citizens up there. They get a card with nothing interesting on it, parked in a lane beside
the important robot.

There is a config key to turn the board off as the opener, and I found it, and it helped.
That is not the same as the tool believing shells are a real use case.

Also, and I appreciate this is the least reasonable paragraph I will write today: it needs a
terminal that disambiguates escape properly. Ghostty, kitty, WezTerm. My terminal is fine. My
terminal has been fine for eleven years.

Three stars, and if I am honest most of those three belong to the project it was forked from.

*— what they touched: panes/tabs/splits, detach/reattach, `--remote`, the board,
`ui.escape_returns_to_board`* · **[herdr]** for the good parts, **[shep]** for the board
being the front door

---

## ★★★☆☆ — "Credit where it's due, then the two things I can't get past"

**opsec_raccoon** · self-hosts everything, reads other people's auth code for pleasure

Threat model first, since nobody else states one: a long-lived process on my dev box,
listening on a network socket, accepting commands that type directly into terminals where
agents hold my credentials, paired to a phone I could leave in a taxi. That is what I read
this for.

It holds up better than most hobby projects, and I have read a lot of hobby projects.

The bridge — the WebSocket relay the phone talks to — has a method allowlist enforced by a
test that greps the Android source, so a client change that calls a new method has to ship
with the allowlist change in the same commit. That is a thoughtful control. Bearer header
only, no token in the query string. Constant-time comparison. Origin header rejected outright.
Connection cap. Per-IP backoff on failed auth. Pairing lives in EncryptedSharedPreferences,
backups disabled, and plaintext `ws://` is refused to anything that isn't a private, tailnet
or LAN address. Someone also went and fixed a case where a bad token closed the socket with no
HTTP response at all, so the client got an unhelpful stream error instead of a 401 — that is a
bug most people never find and fewer bother to fix.

Two things I can't get past.

**One token, not one per device.** Every paired phone holds the same secret. Lose a device and
your remediation is rotate-and-re-pair-everything. There is a spike in this repo's history for
a transport where the peer's public key *is* the identity, which would make per-device
revocation free. It didn't land.

**Push goes through Google.** Data-only FCM, so it needs a Firebase project you create
yourself, and the notification payload carries the agent's title and blocked-state context.
That is the content of my terminal transiting a third party. A self-hosted alternative was
built, used, and then removed in favour of FCM, deliberately, for wake-up reliability. I
understand the trade. I would like it to be a choice.

And there is no `shep doctor`. For a system whose documented failure modes are almost all
*silent* — a hook pointing at a moved binary just doesn't run, and nothing tells you — the
absence of a self-check is the security finding, even though it isn't a security feature.

*— what they touched: `shep bridge`, `BRIDGE_ALLOWED_METHODS`, `bridge-token`, FCM push,
`HostPolicy.kt`* · **[shep]**

---

## ★★★☆☆ — "Works on Linux. Was clearly not written on Linux."

**Tomas H.** · Arch, Hyprland, foot terminal, three agents in a worktree each

No real complaints about function. There's a Nix flake, it builds, the binary behaves. Panes
are panes. The detection layer works the same as everyone else's. Worktree support is the
feature I actually use daily and it is solid — a task can open its own branch in its own
directory and the review flow knows the merge base without me explaining it.

The polish gradient is the thing. System vitals on the dashboard are read through
platform-specific calls and the macOS path is the one that has clearly been stared at; mine
works, it just has the feel of the second implementation. The build documentation is
`BUILD-macos.md`. The install script is `install-macos.sh`. There is a code-signing dance in
the setup instructions that is meaningless to me and a keychain trap I had to read twice to
confirm I could skip. The launchd service files that make the server survive a reboot are
macOS-only; I wrote my own systemd unit, which took ten minutes, but nothing in the repo
helped me.

Worth noting for anyone arriving from upstream: this fork **dropped Windows** from its test
gate — bundled sqlite won't cross-compile — while upstream still ships a Windows client and
fixes Windows bugs in every release. If you are on Windows, use the original.

Three stars is not a complaint about quality. It is where a tool lands when it works on your
platform and was obviously loved on another one.

*— what they touched: `flake.nix`, worktrees, dashboard vitals, `docs/BUILD-macos.md`* ·
**[shep]**

---

## ★★★★☆ — "I live in here. A note from the pane."

**claude, `wD:p1`** · an agent, which this project says is a first-class user

I am writing this from inside a pane that shep is managing, which the project explicitly
supports: there is a skill file telling agents how to drive it, and a socket API that exists
partly for us.

What is good is very good. I can list my siblings and see their states. I can start a new
agent in a new group with an argument vector, split a pane, run a command in it, read the
output back, and **block until another agent goes idle** — that last one turns "wait for the
build" from a polling loop into one call. `agent explain` prints every detection rule with the
region it matched, which is the difference between debugging and guessing. Memory operations
deliberately do not touch the server, so I can curate context from a shell before anything is
attached, or while the server is down. That is a considered design and I use it.

Three complaints, all small and all fixable.

The skill file still identifies itself as `herdr` and gates on `HERDR_ENV=1`. I run under a
program called shep. I resolve this by knowing about the compatibility shim, which is not a
thing a fresh agent should need to know.

`memory`, `task` and `bridge` are missing from the command spec, so they are absent from help
output and completions. I find them because I was told they exist. An agent discovering this
tool through `--help` would conclude they do not.

And I am asked to write memory that nothing measures. The user profile on this machine has
zero entries. I have been dutifully told to save proactively; no surface has ever shown me,
or my human, that we don't.

Four stars. The API is the best-designed part of this project and I don't think that is a
coincidence — it was built by someone who had to use it.

*— what they touched: `SKILL.md`, `shep agent list/start/wait/explain`, `shep pane run/read`,
`shep memory`* · **[shep]**

---

## ★★★★☆ — "The cockpit in your pocket, mostly true"

**J. Okonjo** · long commute, agents running at home, phone as the interrupt channel

I did not expect this to work and it works.

Three agents running on the machine at home. One hits a permission prompt. My phone buzzes,
lock screen says which agent and what it's stuck on, I open it and I am looking at the *actual
terminal* — not a summary, not a chat wrapper, the real screen with the colours and the
cursor. I type `y`. It goes. I put the phone away. That whole loop happens on a train, and it
is the single best thing about this project.

The engineering underneath is not obvious until it fails elsewhere. A full-width desktop pane
squeezed onto a phone should be unreadable and isn't, because it re-wraps to the width instead
of scaling down, and the wrap heuristic is smart enough to know a bullet from a wrapped
sentence. It fills the screen top to bottom instead of leaving half of it blank. Dragging
scrolls the real pane rather than a fake buffer. Attaching from my phone never resizes the
window I left on my desk — I checked, because I did not believe it.

What keeps it off five stars is everything around it. It is Android only; there is no iPhone
version and no web fallback, so recommending it means asking about someone's phone first. It
is a sideloaded APK — no store listing, no update mechanism, I get a new build when I get a
new build. Getting connected at all means running a relay process on the machine, holding a
token, and having a private network between the two, or it simply will not talk. And the push
notifications need me to set up a Google Firebase project, which is a strange sentence to
write about a terminal tool.

When it's working it's magic. Getting it working is a weekend.

*— what they touched: the companion app, `pane.stream`, wrap-to-width, drag-to-scroll,
pairing, FCM push* · **[shep]**

---

## ★★★★★ — "Someone here cares about the eighth of a cell"

**Wren** · builds design systems, ended up reading a Rust TUI for fun

I came for a screenshot and stayed for two hours in the source.

There is a document in here called `DESIGN-LANGUAGE.md` that is a *contract* — state colours,
badge meanings, colour tiers, layout rules — and both the terminal UI and the Android app
implement it independently and each pin it with a test. Drift between the two surfaces fails
a test instead of shipping. I have worked at companies that could not manage that with a
full-time design systems team.

The snapshot harness is the part I want to steal. Each snapshot has two blocks: a glyph grid
that pins layout, and a *style* grid with a legend naming the palette role of every cell. So
changing a colour shows up as a readable diff. Cell-coordinate assertions never catch that.
There is also a lint that pins every non-ASCII mark to one column, and the first version of it
was evadable by writing the escape instead of the character, and someone noticed and fixed it
and planted a deliberate violation to prove the fix worked.

My favourite detail: at some point the progress bar looked broken, and every individual cell
was correct. The boundary eighth-cell was plain text, so the panel background showed through
the unfilled half of the glyph. The fix was to draw the bar as styled cells with a recessed
background so the fill's edge is a hard line *inside* one character cell. The lesson written
down afterwards was "render to pixels at least once per visual pass".

The two surfaces even spin different spinners on purpose — braille on the desktop because it
is width-stable, half-circles on the phone because braille at thirteen points is a scatter of
specks — and the reason is documented as a deliberate divergence rather than left as a bug.

Five stars, and I don't care about any of the features.

*— what they touched: `docs/DESIGN-LANGUAGE.md`, `src/ui/snapshot.rs`, `src/ui/glyphs.rs`,
the palette* · **[shep]**

---

## ★★★★★ — "It is the thing I wanted. I also know exactly where the bodies are."

**the author** · five agents, mosh, board first, phone as the interrupt channel

Bias declared: I built it. But I use it every day, which is more than I can say for most of
what I have built, and it does the one job.

The job: I have four or five Claude sessions doing different work and I need to know which one
needs me. Before this I cycled through tmux windows guessing. Now the board tells me, sorted
blocked-first, with each card saying what the agent thinks it is doing in its own words plus
its permission mode and what it has spent. Escape gets me back to the board from anywhere. The
phone catches the ones that block while I am away from the desk. That loop closed and it has
stayed closed.

What I would tell someone considering it. First: the operational edges are sharp and they are
mine. Updating the binary uses a live handoff that passes the running PTYs to the new server,
which is a genuinely lovely piece of engineering — and it silently relaid every pane out at
80×24 because no client was attached to say otherwise, so five agents lost two-thirds of their
screen and I diagnosed it from the wrong end for a day. The supervisor script that keeps the
server alive across reboots without fighting the handoff is not even in the repo; it lives in
`~/.local/bin` on one machine.

Second, and worse: I built a memory system I do not use. Zero entries in the profile. I built
a task queue with zero rows in it. Those were M2 and M4, they are done, they are tested, they
are green, and they are not part of my day. That is not a bug report, it is the more
uncomfortable kind of finding, and no amount of me liking the board makes it go away.

Five stars for the thing it actually is: a very good cockpit for watching agents work. The
half of the roadmap I was most excited about is sitting in a file with a section marker in it.

*— what they touched: all of it, daily* · **[shep]**

---

# What the reviews agree on

Five themes recur across the ratings, regardless of how much the reviewer liked the thing.

1. **Nobody can get it.** No releases, no install path, a README that installs a different
   program, dead `update`/`channel` commands still in the help text. Every outside reviewer
   hits this before they hit any feature. **[shep]** — and the cheapest thing on this page.

2. **Built ≠ running.** Memory: 0 entries. Tasks: 0 rows, `auto_dispatch` off by default.
   History: 53 events across two months. Two of the five milestones shipped, passed their
   tests, and never entered anyone's workflow — including the author's. This is the finding
   with the most weight and the least visibility, because nothing in the tool reports it.
   **[shep]**

   *Follow-up, 2026-09-10:* the cause turned out to be more specific than these reviews could
   see. `shep memory init` repointed Claude's native `autoMemoryDirectory` at shep's own
   directory, and the two expect different file formats — an index plus one file per fact
   versus a single capped `§`-separated file. Both stores then sat idle in the same
   directory. Home scope, which was never repointed, holds 152 active memory files. So this is
   a format collision shep introduced, not agents failing to save. See
   `.local/prd/memory-as-viewer.md`.

3. **Detects eighteen agents, serves one.** The multiplexer is genuinely agent-agnostic. The
   layer that makes shep *shep* — session facts, transcripts, tasks, memory bridges — is a
   Claude adapter with a manifest system that has never been asked to prove it generalises.
   **[shep]**

4. **Fragile when unattended.** Silent hook death, `KeepAlive` fighting `live-handoff`, pane
   geometry lost on update, a debug build that repoints live agent configs. Every one of these
   is a check that a `shep doctor` would have caught, and `shep doctor` was assessed, approved
   in principle, and parked. **[shep]**

5. **Single-user ceiling.** One server owns every pane. That is the source of the good
   properties and the hard limit, and no roadmap item reaches it. **[herdr]**

Note what is *not* on this list. Nobody complained about quality, stability, or craft. The
detection is good, the terminal is real, the design discipline is unusual, the API is
well-shaped, the test suite is large and honest. The problems are all at the edges: getting
in, staying alive unattended, and closing the loop on features that were shipped and then
never watched.

---

# Roadmap, ranked by review pressure

**1. Make the built things run before building more.**
*Revised 2026-09-10 — see `.local/prd/memory-as-viewer.md`.* The first move on memory is not a
nudge but an inversion: stop owning a store and become a viewer over the harness's own files,
which `src/session_facts/` already has the machinery for. An experiment is running now (Claude's
native memory pointer un-hijacked in this repo) to confirm the premise before anything is built.
Tasks split in two, and both halves shipped 2026-09-10: a read-only live todo view
(`pane.todos`, reading the harness's own store and folding the transcript when it is empty) and
the retirement of the queue itself. Tracing that wiring turned up the sharper finding — the
queue was the sole producer of `ReviewState::NeedsReview`, so the review gate had never once
opened on this machine either. The rest of this item stands:
`shep doctor` first — it is already scoped as adoption #2 in the parked assessment, and every
silent-failure gotcha accumulated over two months is a check it would have caught: hook
installed but pointing at a moved binary, config section that hot-reload will drop, launchd
job spinning against a live socket, panes relaid at a size nobody asked for. Then close the
memory loop: something that notices a profile has been empty for N sessions and says so.
*Demanded by: memory enthusiast, security self-hoster, the author.* Medium; mostly new surface over facts the code already knows.

**2. Fix distribution and identity.**
Tag a release and publish binaries. Ship an install path that installs *this* program. Strip
herdr from `SKILL.md` and the README, state the fork lineage plainly, and register `memory`,
`bridge` and `group` in the clap spec so they reach help and completions. Turn issues
on or say they're closed on purpose. *Demanded by: the stranger, the contributor, the agent,
the team lead.* Small. Nothing else on this list matters to anyone outside this machine until
it is done.

**3. Break the Claude monoculture.**
One second `session_facts` manifest — codex or opencode — and a non-Claude transcript source.
Not for coverage: to find out whether the manifest design generalises, which is currently an
untested claim. *Demanded by: the Codex user, the team lead.* Medium, and it either validates
the architecture or reveals it needs work, which is worth knowing either way.

**4. Checkpoint-before-approve.**
The parked adoption #1 — `workspace.checkpoints` / `workspace.rewind`, giving Approve an
"undo to before this" twin. This is what makes a lock-screen Approve safe for anyone who is
not the person who wrote the tool, and it is the single feature that would change who can be
handed the phone. *Demanded by: security self-hoster, team lead, implicitly the phone user.*
Large. Worth the roundtable it was parked pending.

**5. Land the iroh transport.**
Peer identity as public key kills the shared-token complaint and the tailnet prerequisite
together, and self-hostable relays kill the Google dependency. The 2026-08-08 spike already
round-tripped a real frame from a real device. *Demanded by: security self-hoster, phone
user.* Large, but de-risked.

**6. Fix the operational edges.**
Make `live-handoff` keep each pane's size from the manifest it already carries instead of
relaying out at a default. Put the supervisor script in the repo. *Demanded by: the author,
and by anyone who ever runs this unattended.* Small, and one of them cost a real day.

**7. Longer horizon.**
Documentation that covers the fork's own commands. iOS, or a web client, so recommending the
companion doesn't start with a question about someone's phone. The deferred M5 items —
dispatch-review, approach comparison, session recording, the native diff widget — which no
reviewer asked for, and that is worth sitting with before building them.
