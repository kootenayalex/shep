#!/usr/bin/env python3
"""Drive flows 08–13 against a throwaway shep server and verify each one by
artifact: the pty read back over the JSON socket, the agent's state and
group over the same socket, and the notification shade via `dumsys`.

The Maestro flows on their own only see the screen. This wrapper owns the
half they cannot: it starts the shell agent they type into, holds it in a
manual "working" state while the queue flow runs, posts the notification the
clear flow dismisses, and reads the server afterwards.

    SHEP_SOCKET_PATH=/tmp/shep-dev/api.sock \
    SHEP_BIN=target/debug/shep MAESTRO_DEVICE=emulator-5554 \
        android/maestro/input-checks.py [--junit-dir DIR] [--only 08,09]

Exit status is non-zero when any flow or any check fails. Each flow's junit
report lands in --junit-dir (default /tmp/shep-dev/maestro).
"""
from __future__ import annotations

import argparse
import json
import os
import random
import socket
import string
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
SOCKET = os.environ.get("SHEP_SOCKET_PATH", "/tmp/shep-dev/api.sock")
SHEP_BIN = os.environ.get("SHEP_BIN", str(HERE.parents[1] / "target" / "debug" / "shep"))
DEVICE = os.environ.get("MAESTRO_DEVICE", "emulator-5554")
MAESTRO = os.environ.get("MAESTRO", "maestro")
ADB = os.environ.get("ADB", "adb")
AGENT = os.environ.get("AGENT", "shell")
APP = "dev.shep.companion"


def call(method: str, params: dict | None = None) -> dict:
    """One request over the unix socket; raises on an error reply."""
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
        s.connect(SOCKET)
        s.sendall((json.dumps({"id": "1", "method": method, "params": params or {}}) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    reply = json.loads(buf.decode())
    if "error" in reply:
        raise RuntimeError(f"{method}: {reply['error']}")
    return reply["result"]


def agent() -> dict:
    return call("agent.get", {"target": AGENT})["agent"]


def ensure_agent() -> dict:
    """The plain shell the flows type into; started once, reused after."""
    try:
        return agent()
    except RuntimeError:
        call("agent.start", {"name": AGENT, "argv": ["/bin/sh"], "new_workspace": True})
        time.sleep(1.0)
        return agent()


def pane_text(pane_id: str) -> str:
    return call("pane.read", {"pane_id": pane_id, "source": "visible"})["read"]["text"]


def queued(pane_id: str) -> int:
    for row in call("session.overview")["overview"]["agents"]:
        if row["pane_id"] == pane_id:
            return int(row.get("queued_input", 0))
    return 0


def mark() -> str:
    return "".join(random.choices(string.ascii_lowercase, k=6))


def adb(*args: str) -> str:
    return subprocess.run([ADB, "-s", DEVICE, *args], capture_output=True, text=True, check=False).stdout


def notification_id(tag: str) -> int:
    """The companion posts under `tag.hashCode()` (Java's String hash)."""
    h = 0
    for ch in tag:
        h = (31 * h + ord(ch)) & 0xFFFFFFFF
    return h - (1 << 32) if h >= (1 << 31) else h


def shep_notifications() -> list[int]:
    """Ids of the companion's posted notifications, from the shade itself."""
    out = adb("shell", "dumpsys", "notification", "--noredact")
    ids = []
    for line in out.splitlines():
        if "NotificationRecord(" in line and f"pkg={APP}" in line:
            ids.append(int(line.split(" id=", 1)[1].split(" ", 1)[0]))
    return ids


def notify_push(pane_id: str, kind: str, state: str, message: str) -> None:
    """Post a notification the way the server's exec bridge does."""
    env = dict(os.environ)
    env.update({
        "SHEP_NOTIFY_OP": "show", "SHEP_NOTIFY_KIND": kind, "SHEP_NOTIFY_STATE": state,
        "SHEP_NOTIFY_AGENT": AGENT, "SHEP_NOTIFY_WORKSPACE": "checks",
        "SHEP_NOTIFY_PANE_ID": pane_id, "SHEP_NOTIFY_MESSAGE": message,
        "SHEP_NOTIFY_TITLE": "", "SHEP_NOTIFY_TASK_ID": "",
    })
    subprocess.run([SHEP_BIN, "bridge", "notify-push"], env=env, check=True, capture_output=True)


def run_flow(name: str, junit_dir: Path, **env: str) -> bool:
    flow = HERE / name
    cmd = [MAESTRO, "--device", DEVICE, "test", "--format", "junit",
           "--output", str(junit_dir / f"{flow.stem}.xml")]
    for key, value in env.items():
        cmd += ["-e", f"{key}={value}"]
    cmd.append(str(flow))
    print(f"--- {name} {env}", flush=True)
    return subprocess.run(cmd, check=False).returncode == 0


def wait_for(predicate, timeout: float = 8.0, every: float = 0.25) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(every)
    return predicate()


class Checks:
    def __init__(self, junit_dir: Path) -> None:
        self.junit_dir = junit_dir
        self.failures: list[str] = []

    def expect(self, ok: bool, what: str) -> None:
        print(("  ok   " if ok else "  FAIL ") + what, flush=True)
        if not ok:
            self.failures.append(what)

    def flow(self, name: str, **env: str) -> None:
        self.expect(run_flow(name, self.junit_dir, **env), f"{name} passes")

    # 08: the keyboard's text reaches the pty and Enter runs it.
    def live_input(self) -> None:
        pane_id = ensure_agent()["pane_id"]
        m = mark()
        self.flow("08-live-input.yaml", AGENT=AGENT, MARK=m)
        text = pane_text(pane_id)
        self.expect(f"\nlive-{m}\n" in text, "pty shows the echoed line as output")
        self.expect(text.count(f"echo live-{m}") == 1, "the command was typed exactly once")

    # 09: prompts queued while working wait, then land one paste + Enter each.
    def queue_input(self) -> None:
        pane_id = ensure_agent()["pane_id"]
        call("agent.set_state", {"target": AGENT, "state": "working"})
        m = mark()
        try:
            self.flow("09-queue-input.yaml", AGENT=AGENT, MARK=m)
            self.expect(wait_for(lambda: queued(pane_id) == 2), "two prompts are queued while working")
            self.expect(f"queued-{m}" not in pane_text(pane_id), "nothing reached the pty while working")
            call("agent.set_state", {"target": AGENT, "state": "idle"})
            self.expect(wait_for(lambda: queued(pane_id) == 0), "marking idle flushes the queue")
            text = pane_text(pane_id)
            self.expect(f"\nqueued-{m}-a\n" in text and f"\nqueued-{m}-b\n" in text,
                        "both prompts ran, each as its own line")
            self.expect(text.find(f"queued-{m}-a") < text.find(f"queued-{m}-b"), "in the order they were queued")
        finally:
            call("agent.clear_state", {"target": AGENT})

    # 10: interrupt, armed modifier, locked modifier — asserted on the grid.
    def keybar(self) -> None:
        pane_id = ensure_agent()["pane_id"]
        self.flow("10-keybar-modifiers.yaml", AGENT=AGENT)
        self.expect("^C" in pane_text(pane_id), "pty shows the interrupt")

    # 11: one notification per agent, newest bumps, opening the agent clears.
    def notification_clear(self) -> None:
        info = ensure_agent()
        pane_id = info["pane_id"]
        # Home, not force-stop: a force-stopped app is in Android's "stopped"
        # state and FCM will not wake it, which is not the case under test.
        adb("shell", "input", "keyevent", "KEYCODE_HOME")
        notify_push(pane_id, "blocked", "blocked", "needs a decision")
        notify_push(pane_id, "done", "idle", "finished")
        self.expect(wait_for(lambda: shep_notifications() == [notification_id(pane_id)], timeout=15),
                    f"two events → one notification for {pane_id}")
        self.flow("11-notification-clear.yaml", AGENT=AGENT)
        self.expect(wait_for(lambda: shep_notifications() == []), "opening the agent clears the shade")

    # 12: the agent lands in a fresh group with its name intact.
    def move_to_group(self) -> None:
        before = ensure_agent()
        self.flow("12-move-to-group.yaml", AGENT=AGENT)
        after = agent()
        self.expect(after["workspace_id"] != before["workspace_id"], "agent is in a different group")
        self.expect(after["name"] == AGENT, "agent kept its name")
        ids = {w["workspace_id"] for w in call("workspace.list")["workspaces"]}
        self.expect(before["workspace_id"] not in ids, "the group it left, now empty, closed")

    # 13: set and clear from the phone round-trips through the server.
    def manual_state(self) -> None:
        ensure_agent()
        self.flow("13-manual-state.yaml", AGENT=AGENT)
        self.expect(agent().get("manual_state") is None, "no override remains after clearing")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--junit-dir", default="/tmp/shep-dev/maestro")
    ap.add_argument("--only", default="", help="comma-separated flow numbers, e.g. 08,09")
    args = ap.parse_args()
    junit_dir = Path(args.junit_dir)
    junit_dir.mkdir(parents=True, exist_ok=True)
    checks = Checks(junit_dir)
    steps = {
        "08": checks.live_input, "09": checks.queue_input, "10": checks.keybar,
        "11": checks.notification_clear, "12": checks.move_to_group, "13": checks.manual_state,
    }
    wanted = [s.strip() for s in args.only.split(",") if s.strip()] or list(steps)
    for key in wanted:
        steps[key]()
    if checks.failures:
        print(f"\n{len(checks.failures)} failed:\n  " + "\n  ".join(checks.failures))
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
