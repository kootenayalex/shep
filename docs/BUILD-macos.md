# Building shep on macOS (Alex's macmini)

The vendored `vendor/libghostty-vt` is built by `build.rs` via `zig build`
(zig **0.15.2** pinned — see `vendor/libghostty-vt/build.zig.zon`).

## The Xcode 26 SDK landmine

Xcode 26's `MacOSX26.x.sdk` ships `libSystem.tbd` files that omit the
`arm64-macos` target in a form zig 0.15.2's linker understands. Any zig
compile that links libc — including the **`zig build` build-runner itself** —
fails with:

```
error: undefined symbol: _waitpid
error: undefined symbol: _sysctlbyname
```

zig discovers the SDK by running `xcrun --show-sdk-path` from `PATH`, and it
ignores `SDKROOT`, `DEVELOPER_DIR`, and the `--sysroot` flag for the
build-runner compile (`--sysroot` only reaches graph artifacts). The CLT's
`MacOSX.sdk` symlink also points at the 26.x SDK, so switching developer dirs
does not help.

## The fix in place on this machine

`~/.local/bin/zig` is a wrapper that:

1. prepends `~/.local/share/zig-0.15.2/shimbin` to `PATH` — that dir holds an
   `xcrun` shim which answers `--show-sdk-path` with
   `/Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk` and delegates
   everything else to `/usr/bin/xcrun`;
2. execs the real `~/.local/share/zig-0.15.2/zig`.

The shim is visible **only to zig** (nothing else sees the PATH prepend), so
Xcode/RN/mobile builds keep the real xcrun.

Remove the shim when either the vendored libghostty-vt moves to a zig version
whose linker reads the new tbd format, or the macOS 15 SDK disappears from
`/Library/Developer/CommandLineTools/SDKs/`.

## Installing: why the binary must be code-signed

Install with `just install-macos` (`scripts/install-macos.sh`), not a bare
`cargo build --release` + `cp`.

macOS keys every privacy grant — Full Disk Access, Documents/Desktop/Downloads,
Photos, "access data from other apps" — to the binary's **code signature**.
Cargo emits an ad-hoc, linker-signed binary with no stable identity, so TCC
falls back to the cdhash, which is a hash of the file contents. Every rebuild
produces a different cdhash, macOS sees an unrelated program, and silently
drops the grants: you get re-prompted for everything, forever, and nothing you
click ever sticks.

The install script signs with a real certificate under the fixed signing
identifier `dev.shep.cli`, which produces a designated requirement that stays
constant across rebuilds. Grants then survive. An Apple Development certificate
is preferred over a self-signed one: its requirement matches on the Apple
anchor plus the certificate's common name, which is reissued unchanged, so
grants also survive the yearly certificate renewal. A self-signed certificate's
requirement pins that exact certificate, so replacing it costs you the grants.

Two consequences worth knowing:

- **The keychain prompt is not asking for your login password.** codesign takes
  the identity from the *first* keychain in the search list that holds it. On
  this box that is `workmayt-signing.keychain-db`, which sits ahead of
  `login.keychain-db` and has its own generated password — the account password
  will never open it. Point the script at that password file and it unlocks
  non-interactively, so nothing prompts at all, including from a headless or
  background shell:

  ```sh
  SHEP_SIGNING_KEYCHAIN_PW_FILE=~/.config/workmayt/signing-kc.pw just install-macos
  ```

  The script prints which keychain the identity resolved from, so if it ever
  prompts you can see whose password is being asked for. Signing from the login
  keychain instead works but needs a GUI session: a background/launchd shell
  cannot display that prompt and simply hangs on SecurityAgent.
- **`shep server` is the responsible process for everything it spawns.** Agents
  started by shep inherit its TCC identity, so a grant given to shep covers
  every agent pane. That is what stops the prompts, and it also means those
  agents get that access without prompting again — grant deliberately.

Grant Full Disk Access once, after the first signed install: System Settings >
Privacy & Security > Full Disk Access > `+` > ⌘⇧G > `~/.local/bin` > `shep`.
Then `launchctl kickstart -k gui/$(id -u)/dev.shep.server` so the running server
picks up the new identity — this kills every live agent pane, so do it when
nothing is mid-flight.

## Test-environment gotcha

This box runs shep/herdr sessions, so `HERDR_*`/`SHEP_*` vars leak into any
shell — including test runners. Integration tests must scrub **both**
prefixes (the env_compat fallback reads legacy `HERDR_*` names); see
`src/env_compat.rs` and the `env_remove` pairs in `tests/*.rs`.
