#!/usr/bin/env python3
"""Golden tests comparing Python and Rust plugin behavior.

Each test runs the same action/event against both implementations connected
to the same mock Herdr server and isolated state directories, then compares
the emitted pane.focus requests and exit codes. The implementations must
agree on observable behavior even though their internal state formats differ.
"""

import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

sys.path.insert(0, str(REPO_ROOT))
from helpers.mock_herdr_server import MockHerdrServer  # noqa: E402
from helpers.plugin_harness import generate_panes, run_impl  # noqa: E402

FIXTURE_SIZE = 5


class TestGolden(unittest.TestCase):
    """Compare observable behavior of the Python and Rust plugins."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.socket_path = Path(self.tmp.name) / "herdr.sock"
        self.panes = generate_panes(FIXTURE_SIZE)
        self.server = MockHerdrServer(
            self.socket_path, self.panes, current_pane_id=self.panes[0]["pane_id"]
        )
        self.addCleanup(self.server.stop)

    def _env(self, state_dir, action=None, event=None, pane_id=None, event_json=None):
        env = os.environ.copy()
        env["HERDR_SOCKET_PATH"] = str(self.socket_path)
        env["HERDR_PLUGIN_STATE_DIR"] = str(state_dir)
        if action is not None:
            env["HERDR_PLUGIN_ACTION_ID"] = action
            if pane_id is not None:
                env["HERDR_PANE_ID"] = pane_id
            env.pop("HERDR_PLUGIN_EVENT", None)
            env.pop("HERDR_PLUGIN_EVENT_JSON", None)
        elif event is not None:
            env["HERDR_PLUGIN_EVENT"] = event
            env["HERDR_PLUGIN_EVENT_JSON"] = json.dumps(event_json or {})
            env.pop("HERDR_PLUGIN_ACTION_ID", None)
            env.pop("HERDR_PANE_ID", None)
        return env

    def _run(self, impl, state_dir, **kwargs):
        result = run_impl(impl, self._env(state_dir, **kwargs))
        return result.returncode, result.stderr, result.stdout

    def _assert_impls_match(self, **kwargs):
        """Run both implementations and compare emitted focus calls."""
        results = {}
        label = kwargs.get("action") or kwargs.get("event") or "unknown"
        for impl in ("python", "rust"):
            state_dir = Path(self.tmp.name) / f"state-{impl}-{label}"
            state_dir.mkdir(parents=True, exist_ok=True)
            rc, stderr, _stdout = self._run(impl, state_dir, **kwargs)
            self.assertEqual(rc, 0, f"{impl} exited non-zero: {stderr}")
            results[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(
            results["python"],
            results["rust"],
            f"focus calls differ for {label}: {results}",
        )

    def test_cycle_focuses_next_mru_pane(self):
        self._assert_impls_match(
            action="cycle", pane_id=self.panes[0]["pane_id"]
        )

    def test_focus_attention_jumps_to_first_blocked(self):
        # pane-0003 is working, so attention priority picks the first blocked pane.
        self._assert_impls_match(
            action="focus-attention", pane_id=self.panes[3]["pane_id"]
        )

    def test_cycle_attention_jumps_to_next_attention_pane(self):
        # pane-0000 is blocked; the next attention pane is pane-0004 (also blocked).
        self._assert_impls_match(
            action="cycle-attention", pane_id=self.panes[0]["pane_id"]
        )

    def test_pane_focused_event_updates_history_no_focus(self):
        self._assert_impls_match(
            event="pane.focused", event_json={"pane_id": self.panes[2]["pane_id"]}
        )

    def test_pane_closed_event_removes_pane_no_focus(self):
        self._assert_impls_match(
            event="pane.closed", event_json={"pane_id": self.panes[1]["pane_id"]}
        )

    def test_cycle_repeated_within_timeout_continues(self):
        state_dirs = {
            "python": Path(self.tmp.name) / "state-py-repeat",
            "rust": Path(self.tmp.name) / "state-rs-repeat",
        }
        for d in state_dirs.values():
            d.mkdir(parents=True, exist_ok=True)

        current = self.panes[0]["pane_id"]
        for _ in range(3):
            calls = {}
            for impl in ("python", "rust"):
                rc, stderr, _ = self._run(
                    impl, state_dirs[impl], action="cycle", pane_id=current
                )
                self.assertEqual(rc, 0, f"{impl} failed: {stderr}")
                calls[impl] = list(self.server.focus_calls)
                self.server.focus_calls.clear()
                self.server.requests.clear()
            self.assertEqual(calls["python"], calls["rust"], calls)
            current = calls["python"][-1]

    def test_cycle_resets_after_timeout(self):
        state_dirs = {
            "python": Path(self.tmp.name) / "state-py-timeout",
            "rust": Path(self.tmp.name) / "state-rs-timeout",
        }
        for d in state_dirs.values():
            d.mkdir(parents=True, exist_ok=True)

        # First cycle establishes a cycle and focuses pane-0001.
        calls1 = {}
        for impl in ("python", "rust"):
            rc, stderr, _ = self._run(
                impl, state_dirs[impl], action="cycle", pane_id=self.panes[0]["pane_id"]
            )
            self.assertEqual(rc, 0, f"{impl} failed: {stderr}")
            calls1[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(calls1["python"], calls1["rust"], calls1)

        time.sleep(1.2)

        # After the timeout, the next cycle should restart from the current pane.
        current = calls1["python"][-1]
        calls2 = {}
        for impl in ("python", "rust"):
            rc, stderr, _ = self._run(
                impl, state_dirs[impl], action="cycle", pane_id=current
            )
            self.assertEqual(rc, 0, f"{impl} failed: {stderr}")
            calls2[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(calls2["python"], calls2["rust"], calls2)

    def test_cycle_after_closed_pane_excludes_it(self):
        state_dirs = {
            "python": Path(self.tmp.name) / "state-py-closed",
            "rust": Path(self.tmp.name) / "state-rs-closed",
        }
        for d in state_dirs.values():
            d.mkdir(parents=True, exist_ok=True)

        # Close pane-0001, then cycle from pane-0000. The next pane should
        # skip pane-0001 and focus pane-0002.
        calls = {}
        for impl in ("python", "rust"):
            rc, stderr, _ = self._run(
                impl,
                state_dirs[impl],
                event="pane.closed",
                event_json={"pane_id": self.panes[1]["pane_id"]},
            )
            self.assertEqual(rc, 0, f"{impl} pane.closed failed: {stderr}")
            rc, stderr, _ = self._run(
                impl, state_dirs[impl], action="cycle", pane_id=self.panes[0]["pane_id"]
            )
            self.assertEqual(rc, 0, f"{impl} cycle failed: {stderr}")
            calls[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(calls["python"], calls["rust"], calls)

    def test_cycle_restarts_when_pane_disappears_without_event(self):
        state_dirs = {
            "python": Path(self.tmp.name) / "state-py-disappear",
            "rust": Path(self.tmp.name) / "state-rs-disappear",
        }
        for d in state_dirs.values():
            d.mkdir(parents=True, exist_ok=True)

        # First cycle establishes order [0,1,2,3,4] and focuses pane-0001.
        calls1 = {}
        for impl in ("python", "rust"):
            rc, stderr, _ = self._run(
                impl, state_dirs[impl], action="cycle", pane_id=self.panes[0]["pane_id"]
            )
            self.assertEqual(rc, 0, f"{impl} failed: {stderr}")
            calls1[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(calls1["python"], calls1["rust"], calls1)

        # Simulate a pane disappearing from pane.list without a pane.closed event.
        self.server.panes = self.panes[:4]

        current = calls1["python"][-1]
        calls2 = {}
        for impl in ("python", "rust"):
            rc, stderr, _ = self._run(
                impl, state_dirs[impl], action="cycle", pane_id=current
            )
            self.assertEqual(rc, 0, f"{impl} failed: {stderr}")
            calls2[impl] = list(self.server.focus_calls)
            self.server.focus_calls.clear()
            self.server.requests.clear()
        self.assertEqual(calls2["python"], calls2["rust"], calls2)
        self.assertEqual(calls2["python"], [self.panes[0]["pane_id"]])


if __name__ == "__main__":
    unittest.main()
