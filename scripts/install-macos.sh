#!/usr/bin/env bash
# Build, code-sign, and install shep on macOS.
#
# Why the signing step exists: macOS keeps TCC grants (Full Disk Access,
# Documents/Desktop/Downloads, Photos, "data from other apps") keyed to the
# binary's code signature. Cargo's output is ad-hoc linker-signed, which has no
# stable identity, so TCC falls back to the cdhash — a content hash that changes
# on every single rebuild. Every rebuild therefore looks like a brand-new
# program and every grant you clicked is silently discarded.
#
# Signing with a real certificate and a fixed signing identifier gives the
# binary a designated requirement that survives rebuilds, so the grants stick.
#
# Usage:
#   scripts/install-macos.sh              # build release, sign, install
#   scripts/install-macos.sh --no-build   # sign + install the existing binary
#
# Environment:
#   SHEP_CODESIGN_IDENTITY  SHA-1 or name of the identity to sign with.
#                           Defaults to the first Apple Development identity.
#   SHEP_SIGN_ID            Signing identifier. Default dev.shep.cli.
#                           Changing this invalidates existing TCC grants.
#   SHEP_INSTALL_DIR        Install target. Default ~/.local/bin.
#   SHEP_SIGNING_KEYCHAIN   Keychain holding the identity. Signing from a
#                           dedicated keychain avoids prompting for the login
#                           keychain, which a headless shell cannot answer.
#   SHEP_SIGNING_KEYCHAIN_PW_FILE
#                           File holding that keychain's password; when set the
#                           keychain is unlocked non-interactively before
#                           signing. Keep it mode 600.

set -euo pipefail

sign_id="${SHEP_SIGN_ID:-dev.shep.cli}"
install_dir="${SHEP_INSTALL_DIR:-$HOME/.local/bin}"
target="$install_dir/shep"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
built="$repo_root/target/release/shep"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "install-macos.sh: macOS only (this is $(uname -s))" >&2
  exit 1
fi

if [[ "${1:-}" != "--no-build" ]]; then
  echo "==> building release binary"
  (cd "$repo_root" && cargo build --release --locked)
fi

[[ -f "$built" ]] || { echo "missing $built — build first" >&2; exit 1; }

# Resolve the signing identity. An Apple Development certificate is preferred:
# its designated requirement matches on the certificate's common name, which is
# reissued unchanged, so grants also survive the yearly certificate renewal.
# A self-signed certificate's requirement pins that exact certificate instead.
identity="${SHEP_CODESIGN_IDENTITY:-}"
if [[ -z "$identity" ]]; then
  identity="$(security find-identity -v -p codesigning 2>/dev/null \
    | awk '/Apple Development|Developer ID Application/ { print $2; exit }')"
fi

if [[ -z "$identity" ]]; then
  cat >&2 <<'EOF'
!! No code-signing identity found, so shep can only be ad-hoc signed.
   macOS will keep re-prompting for file access after every rebuild.
   Create one (Keychain Access > Certificate Assistant > Create a Certificate,
   type "Code Signing"), or set SHEP_CODESIGN_IDENTITY, then re-run.
EOF
  exit 1
fi

# codesign resolves the identity from the FIRST keychain in the search list that
# holds it, which is not necessarily the login keychain — so the password it
# asks for is not necessarily the account password. Report which keychain is in
# play, and unlock it up front when a password file is configured.
keychain="${SHEP_SIGNING_KEYCHAIN:-}"
if [[ -z "$keychain" ]]; then
  while read -r kc; do
    kc="${kc//\"/}"; kc="${kc#"${kc%%[![:space:]]*}"}"
    if security find-identity -v -p codesigning "$kc" 2>/dev/null | grep -q "$identity"; then
      keychain="$kc"
      break
    fi
  done < <(security list-keychains)
fi

if [[ -n "$keychain" ]]; then
  echo "==> identity resolves from $keychain"
  pw_file="${SHEP_SIGNING_KEYCHAIN_PW_FILE:-}"
  if [[ -n "$pw_file" ]]; then
    [[ -r "$pw_file" ]] || { echo "cannot read password file $pw_file" >&2; exit 1; }
    security unlock-keychain -p "$(cat "$pw_file")" "$keychain"
    echo "==> unlocked non-interactively"
  else
    echo "    (no SHEP_SIGNING_KEYCHAIN_PW_FILE set — codesign may prompt for"
    echo "     THIS keychain's password, which may not be your login password)"
  fi
fi

# Sign a scratch copy: codesign rewrites the file in place, and rewriting a
# binary that a running shep server is executing takes the process down.
staged="$(mktemp -t shep-staged)"
trap 'rm -f "$staged"' EXIT
cp "$built" "$staged"

echo "==> signing as $sign_id with identity $identity"
codesign --force --sign "$identity" --identifier "$sign_id" --timestamp=none \
  ${keychain:+--keychain "$keychain"} "$staged"
codesign --verify --strict "$staged"

# Same reason: replace the target by unlinking first. Copying over the running
# binary SIGKILLs it, and the shep server owns every live agent pane.
mkdir -p "$install_dir"
rm -f "$target"
cp "$staged" "$target"
# Explicit mode: the staged copy came from mktemp (0600), cp preserves the
# destination's mode, and a bare `chmod +x` is masked by umask into 0711.
chmod 755 "$target"

echo "==> installed $target"
codesign -dv "$target" 2>&1 | grep -E '^(Identifier|TeamIdentifier|Authority)=' || true
echo
echo "designated requirement (this is what TCC pins — it must stay stable):"
codesign -d -r- "$target" 2>&1 | sed -n 's/^designated => /  /p'
