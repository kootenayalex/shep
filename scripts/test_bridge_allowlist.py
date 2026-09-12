"""Maintenance test: the bridge relays only allowlisted API methods, and the
Android companion never calls anything the bridge would refuse.

`src/cli/bridge.rs::BRIDGE_ALLOWED_METHODS` is the contract. This test parses
that block, the bridge-local handlers (`"x.y" =>` match arms under
`src/cli/bridge*`), the API event names (`#[serde(rename = "...")]` in
`src/api/schema/events.rs`) and the real API method names (`src/api/server.rs`),
then checks every dotted `"a.b"` string literal in the companion's Kotlin
sources against them. A phone verb that reaches the relay without being
allowlisted would fail at runtime with "method not allowed over the bridge";
this catches it at `just check` instead.

It also holds `BRIDGE_LOCAL_METHODS` to the handlers it claims to list. That
const is what non-bridge callers (`shep mcp`) read to know what the bridge
answers itself, so a handler missing from it is a tool nobody can reach and a
name in it with no handler is a tool that errors at call time.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BRIDGE = ROOT / "src" / "cli" / "bridge.rs"
BRIDGE_DIR = ROOT / "src" / "cli" / "bridge"
EVENTS = ROOT / "src" / "api" / "schema" / "events.rs"
API_SERVER = ROOT / "src" / "api" / "server.rs"
ANDROID_SRC = ROOT / "android" / "app" / "src" / "main" / "java"

DOTTED = re.compile(r'"([a-z_]+\.[a-z_]+)"')


def method_const(name: str) -> list[str]:
    """The dotted names in one `&[&str]` const in bridge.rs, in source order."""
    text = BRIDGE.read_text(encoding="utf-8")
    match = re.search(rf"const {name}: &\[&str\] = &\[(.*?)\];", text, re.S)
    if match is None:
        raise AssertionError(f"{name} block not found in bridge.rs")
    return DOTTED.findall(match.group(1))


def allowlist() -> list[str]:
    return method_const("BRIDGE_ALLOWED_METHODS")


def bridge_local_const() -> list[str]:
    return method_const("BRIDGE_LOCAL_METHODS")


def bridge_local_methods() -> set[str]:
    found: set[str] = set()
    for path in [BRIDGE, *sorted(BRIDGE_DIR.glob("*.rs"))]:
        text = path.read_text(encoding="utf-8")
        found.update(re.findall(r'^\s*"([a-z_]+\.[a-z_]+)"\s*=>', text, re.M))
        found.update(re.findall(r'const METHOD: &str = "([a-z_]+\.[a-z_]+)"', text))
    return found


def event_names() -> set[str]:
    text = EVENTS.read_text(encoding="utf-8")
    return set(re.findall(r'#\[serde\(rename = "([a-z_]+\.[a-z_]+)"\)\]', text))


def api_method_names() -> set[str]:
    text = API_SERVER.read_text(encoding="utf-8")
    return set(re.findall(r'=> "([a-z_]+\.[a-z_]+)"', text))


def api_namespaces() -> set[str]:
    """The first segment of every method and event name (`pane`, `agent`, …).

    Only dotted strings in one of these namespaces are method calls; the
    companion also has dotted strings that are file names (`shep.secure`)
    and the like, which are nothing the bridge would ever see.
    """
    names = api_method_names() | bridge_local_methods() | event_names()
    return {name.split(".", 1)[0] for name in names}


def companion_dotted_strings() -> dict[str, list[str]]:
    namespaces = api_namespaces()
    seen: dict[str, list[str]] = {}
    for path in sorted(ANDROID_SRC.rglob("*.kt")):
        for name in DOTTED.findall(path.read_text(encoding="utf-8")):
            if name.split(".", 1)[0] not in namespaces:
                continue
            seen.setdefault(name, []).append(str(path.relative_to(ROOT)))
    return seen


class BridgeAllowlistTest(unittest.TestCase):
    def test_allowlist_is_sorted_and_unique(self) -> None:
        names = allowlist()
        self.assertTrue(names, "allowlist parsed empty")
        self.assertEqual(names, sorted(set(names)), "keep BRIDGE_ALLOWED_METHODS sorted and unique")

    def test_allowlist_names_real_api_methods(self) -> None:
        methods = api_method_names()
        self.assertTrue(methods, "no API method names parsed from src/api/server.rs")
        unknown = sorted(set(allowlist()) - methods)
        self.assertEqual(unknown, [], f"allowlisted names that are not API methods: {unknown}")

    def test_local_const_is_sorted_and_lists_every_handler(self) -> None:
        declared = bridge_local_const()
        self.assertTrue(declared, "BRIDGE_LOCAL_METHODS parsed empty")
        self.assertEqual(
            declared,
            sorted(set(declared)),
            "keep BRIDGE_LOCAL_METHODS sorted and unique",
        )
        handled = bridge_local_methods()
        self.assertEqual(
            sorted(handled - set(declared)),
            [],
            "bridge-local handlers missing from BRIDGE_LOCAL_METHODS",
        )
        self.assertEqual(
            sorted(set(declared) - handled),
            [],
            "BRIDGE_LOCAL_METHODS names with no bridge-local handler",
        )

    def test_allowlist_never_overlaps_bridge_locals(self) -> None:
        overlap = sorted(
            set(allowlist()) & (bridge_local_methods() | set(bridge_local_const()))
        )
        self.assertEqual(
            overlap, [], f"bridge-local methods must not also be relayed: {overlap}"
        )

    def test_companion_only_calls_permitted_methods(self) -> None:
        permitted = set(allowlist()) | bridge_local_methods() | event_names()
        offenders = {
            name: files
            for name, files in companion_dotted_strings().items()
            if name not in permitted
        }
        self.assertEqual(
            offenders,
            {},
            "the companion sends methods the bridge would refuse; add them to "
            "BRIDGE_ALLOWED_METHODS (or a bridge-local handler) in the same commit",
        )

    def test_companion_sources_present(self) -> None:
        self.assertTrue(ANDROID_SRC.is_dir(), f"missing {ANDROID_SRC}")
        self.assertIn("session.snapshot", companion_dotted_strings())


if __name__ == "__main__":
    unittest.main()
