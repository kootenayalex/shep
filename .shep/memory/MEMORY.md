# Project memory (shep shared memory)
<!-- Managed via `shep memory` and by your agents. Shared across all shep
sessions on this repo. Stable facts: environment, conventions,
build/test commands, tool quirks, lessons learned. -->

## Write-back protocol
Entries are separated by a line containing only the section sign. Edit with
`shep memory add/replace/remove` (substring match). One fact per entry,
present tense, absolute dates, no secrets.
- SAVE proactively: preferences & corrections, then stable environment/
convention facts. SKIP trivia, task progress, log dumps, one-off paths.
- WHEN FULL: shep errors instead of auto-compacting. Consolidate — merge
overlapping entries and drop the stalest in one edit, then add.


Cap: 2200 characters of ENTRY CONTENT (this header does not count).

<!-- BEGIN shep read protocol (managed) -->
## Read protocol
These entries are not everything that is known. `shep memory search "<terms>"`
also searches prior sessions' prompts and replies, which are not in your context.
- SEARCH FIRST when: about to re-derive an environment, build, or convention
fact; about to ask the human something they may have answered before;
picking up work in a part of this repo you have not seen this session.
- Returns matching entries plus history snippets with the match in [brackets].
Terms are ANDed and matched as words, not meaning — retry with fewer or
different words. A miss is cheap and expected; skipping the search is not.
<!-- END shep read protocol (managed) -->

§

Run 'just check' before committing; ~2,658 tests must stay green.

§

Adding a config section needs BOTH KNOWN_TOP_LEVEL_CONFIG_KEYS and a load_live_section call in config/io.rs, or hot-reload silently resets it.

§

New Mode variants need match arms in ui.rs render, input/mod.rs (2 sites), and app/mod.rs paste.

§

Adding global-menu/context-menu entries shifts click y-offsets and label-vector assertions in input/sidebar.rs and state.rs tests.

§

Schema changes: SHEP_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current.

§

Never cp over the installed ~/.local/bin/shep binary — macOS inode cache SIGKILLs launches (exit 137); rm first, then cp.

§

A GitHub 403 aborts remaining origin push legs — push gitea directly: git push http://alex:$(cat ~/.config/gitea/api-key)@10.0.0.10:333/code/shep.git master

§

AppState::test_new() keeps titlebar/hint-bar chrome OFF (minimal-baseline convention, like pane_gaps); app_for_mouse_test pins chrome off.

§

docs/* is gitignored upstream — plan docs need git add -f.

§

Build needs the zig xcrun shim (macmini zig/xcode26 SDK workaround).
