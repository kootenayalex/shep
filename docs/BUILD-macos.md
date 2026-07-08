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

## Test-environment gotcha

This box runs shep/herdr sessions, so `HERDR_*`/`SHEP_*` vars leak into any
shell — including test runners. Integration tests must scrub **both**
prefixes (the env_compat fallback reads legacy `HERDR_*` names); see
`src/env_compat.rs` and the `env_remove` pairs in `tests/*.rs`.
