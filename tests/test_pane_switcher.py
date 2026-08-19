#!/usr/bin/env python3
"""Functional correctness tests for the herdr.pane-switcher plugin.

The tests drive the real pane_switcher.py entry point against a local mock Herdr
Unix socket server and isolated state directories. They verify observable
behavior at the public seams: focused pane, state.json contents, MRU cycle
order, attention priority, timeout continuation, and error/edge paths.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

REPO_ROOT = Path(__file__).resolve().parent.parent
PANE_SWITCHER_SCRIPT = REPO_ROOT / "pane_switcher.py"
sys.path.insert(0, str(REPO_ROOT))

import pane_switcher  # noqa: E402
from helpers.mock_herdr_server import MockHerdrServer as BaseMockHerdrServer  # noqa: E402

FIXTURE = [
    {"pane_id": "pane-0000", "agent_status": "blocked", "title": "Pane 0"},
    {"pane_id": "pane-0001", "agent_status": "done", "title": "Pane 1"},
    {"pane_id": "pane-0002", "agent_status": "idle", "title": "Pane 2"},
    {"pane_id": "pane-0003", "agent_status": "working", "title": "Pane 3"},
    {"pane_id": "pane-0004", "agent_status": "blocked", "title": "Pane 4"},
]


class MockHerdrServer(BaseMockHerdrServer):
    """Mock Herdr server with optional injected failures for tests."""

    def __init__(self, socket_path, panes, current_pane_id=None):
        self.failures = set()
        super().__init__(socket_path, panes, current_pane_id)

    def fail(self, method):
        self.failures.add(method)

    def unfail(self, method):
        self.failures.discard(method)

    def _dispatch(self, method, params):
        if method in self.failures:
            return {"error": f"injected failure for {method}"}
        return super()._dispatch(method, params)


class TestManifestAndRename(unittest.TestCase):
    """Static checks that the manifest and script match the expected rename."""

    def test_plugin_id_is_herdr_pane_switcher(self):
        text = (REPO_ROOT / "herdr-plugin.toml").read_text()
        match = re.search(r'^id\s*=\s*"([^"]+)"', text, re.MULTILINE)
        self.assertIsNotNone(match, "plugin id not found in herdr-plugin.toml")
        self.assertEqual(match.group(1), "herdr.pane-switcher")

    def test_all_commands_point_to_pane_switcher(self):
        text = (REPO_ROOT / "herdr-plugin.toml").read_text()
        for line in text.splitlines():
            if line.strip().startswith("command"):
                self.assertIn('"./pane_switcher.py"', line, line)

    def test_request_id_prefix_is_pane_switcher(self):
        source = (REPO_ROOT / "pane_switcher.py").read_text()
        self.assertIn('f"plugin:pane-switcher:{method}:{time.time_ns()}"', source)


class TestPaneSwitcherFunctional(unittest.TestCase):
    """End-to-end functional tests against a mock Herdr server."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.socket_path = Path(self.tmp.name) / "herdr.sock"
        self.state_dir = Path(self.tmp.name) / "state"
        self.server = MockHerdrServer(
            self.socket_path, FIXTURE, current_pane_id=FIXTURE[0]["pane_id"]
        )

    def tearDown(self):
        self.server.stop()

    def _env(self, **kwargs):
        base = {
            "HERDR_SOCKET_PATH": str(self.socket_path),
            "HERDR_PLUGIN_STATE_DIR": str(self.state_dir),
        }
        base.update(kwargs)
        return base

    def _read_state(self):
        state_path = self.state_dir / "state.json"
        if not state_path.exists():
            return {}
        return json.loads(state_path.read_text())

    def _write_state(self, raw):
        self.state_dir.mkdir(parents=True, exist_ok=True)
        (self.state_dir / "state.json").write_text(raw)

    def _run_action(self, action, pane_id=None, event_json=None, env_extra=None):
        """Run pane_switcher.main with the supplied Herdr-style environment."""
        env = self._env()
        if action in ("pane.focused", "pane.closed"):
            env["HERDR_PLUGIN_EVENT"] = action
            env["HERDR_PLUGIN_EVENT_JSON"] = json.dumps(event_json or {})
        else:
            env["HERDR_PLUGIN_ACTION_ID"] = action
            if pane_id is not None:
                env["HERDR_PANE_ID"] = pane_id
        if env_extra:
            env.update(env_extra)
        with patch.dict("os.environ", env, clear=True):
            pane_switcher.main()
        return self.server.current_pane_id

    def test_pane_focused_updates_mru_history(self):
        self._run_action("pane.focused", event_json={"pane_id": "pane-0002"})
        state = self._read_state()
        self.assertEqual(state["history"], ["pane-0002"])

        self._run_action("pane.focused", event_json={"pane_id": "pane-0000"})
        state = self._read_state()
        self.assertEqual(state["history"], ["pane-0000", "pane-0002"])

    def test_pane_focused_resets_cycle_when_target_changes(self):
        # Establish a cycle whose target is pane-0001.
        self._run_action("cycle", pane_id="pane-0000")
        state = self._read_state()
        self.assertEqual(state["cycle"]["target"], "pane-0001")

        # A focused event for a different pane should invalidate the cycle.
        self._run_action("pane.focused", event_json={"pane_id": "pane-0000"})
        state = self._read_state()
        self.assertEqual(state["cycle"], {})

    def test_cycle_focuses_next_mru_pane(self):
        # Build an explicit MRU history so the next pane is predictable.
        self._run_action("pane.focused", event_json={"pane_id": "pane-0002"})
        self._run_action("pane.focused", event_json={"pane_id": "pane-0000"})

        focused = self._run_action("cycle", pane_id="pane-0000")
        state = self._read_state()

        self.assertEqual(focused, "pane-0002")
        self.assertEqual(
            state["history"],
            ["pane-0002", "pane-0000", "pane-0001", "pane-0003", "pane-0004"],
        )
        self.assertEqual(
            state["cycle"]["order"],
            ["pane-0000", "pane-0002", "pane-0001", "pane-0003", "pane-0004"],
        )
        self.assertEqual(state["cycle"]["index"], 1)
        self.assertEqual(state["cycle"]["target"], "pane-0002")

    def test_cycle_repeated_within_timeout_continues(self):
        # Drive three rapid cycles and verify the order is continued.
        with patch.object(pane_switcher.time, "monotonic", side_effect=[0.0, 0.2, 0.4]):
            focused = self._run_action("cycle", pane_id="pane-0000")
            self.assertEqual(focused, "pane-0001")

            # Simulate Herdr now reporting the newly-focused pane as current.
            focused = self._run_action("cycle", pane_id="pane-0001")
            self.assertEqual(focused, "pane-0002")

            focused = self._run_action("cycle", pane_id="pane-0002")
            self.assertEqual(focused, "pane-0003")

    def test_cycle_after_timeout_restarts(self):
        with patch.object(pane_switcher.time, "monotonic", side_effect=[0.0, 2.0]):
            self._run_action("cycle", pane_id="pane-0000")
            focused = self._run_action("cycle", pane_id="pane-0001")
            state = self._read_state()

        self.assertEqual(focused, "pane-0000")
        self.assertNotEqual(state["cycle"]["order"][0], state["cycle"]["target"])

    def test_cycle_with_empty_pane_list(self):
        self.server.panes = []
        self.server.current_pane_id = None
        focused = self._run_action("cycle", pane_id="pane-0001")
        self.assertIsNone(focused)

    def test_cycle_with_single_pane(self):
        self.server.panes = [FIXTURE[0]]
        self.server.current_pane_id = None
        focused = self._run_action("cycle", pane_id="pane-0000")
        # No other pane exists to switch to, so no focus request is issued.
        self.assertIsNone(focused)

    def test_focus_attention_priority(self):
        focused = self._run_action("focus-attention", pane_id="pane-0003")
        self.assertEqual(focused, "pane-0000")

    def test_focus_attention_ties_by_priority_then_fixture_order(self):
        # pane-0000 and pane-0004 are both blocked; stable sort keeps fixture order.
        focused = self._run_action("focus-attention", pane_id="pane-0003")
        self.assertEqual(focused, "pane-0000")

    def test_cycle_attention_moves_to_next_priority_pane(self):
        focused = self._run_action("cycle-attention", pane_id="pane-0000")
        self.assertEqual(focused, "pane-0004")

    def test_cycle_attention_wraps_to_first_priority_pane(self):
        # pane-0003 is the last entry in the attention-sorted list, so it wraps to first.
        focused = self._run_action("cycle-attention", pane_id="pane-0003")
        self.assertEqual(focused, "pane-0000")

    def test_cycle_attention_when_current_not_in_attention_list(self):
        # pane-0003 is working and is the last attention pane, but remove it.
        self.server.panes = FIXTURE[:3]
        focused = self._run_action("cycle-attention", pane_id="pane-0003")
        self.assertEqual(focused, "pane-0000")

    def test_no_attention_panes_does_nothing(self):
        self.server.panes = [{"pane_id": "pane-x", "agent_status": "unknown"}]
        self.server.current_pane_id = None
        focused = self._run_action("focus-attention")
        self.assertIsNone(focused)

    def test_pane_closed_removes_from_history_and_resets_cycle(self):
        # Pre-populate state as if the user had cycled through several panes.
        self._write_state(
            json.dumps(
                {
                    "history": ["pane-0003", "pane-0002", "pane-0001", "pane-0000"],
                    "cycle": {
                        "order": ["pane-0003", "pane-0002", "pane-0001", "pane-0000"],
                        "index": 1,
                        "target": "pane-0002",
                    },
                }
            )
        )

        self._run_action("pane.closed", event_json={"pane_id": "pane-0001"})
        state = self._read_state()

        self.assertNotIn("pane-0001", state["history"])
        self.assertEqual(state["cycle"], {})

    def test_malformed_state_json_recovered(self):
        self._write_state("not json")
        self._run_action("pane.focused", event_json={"pane_id": "pane-0000"})
        state = self._read_state()
        self.assertEqual(state["history"], ["pane-0000"])

    def test_pane_current_error_falls_back_gracefully(self):
        self.server.current_pane_id = None
        self.server.fail("pane.current")
        # No HERDR_PANE_ID or event payload; pane.current fails, but cycle should simply do nothing.
        focused = self._run_action("cycle")
        self.assertIsNone(focused)

    def test_pane_list_error_exits_with_error(self):
        self.server.fail("pane.list")
        with self.assertRaises(RuntimeError):
            self._run_action("cycle", pane_id="pane-0000")

    def test_pane_focus_error_exits_with_error(self):
        self.server.fail("pane.focus")
        with self.assertRaises(RuntimeError):
            self._run_action("focus-attention", pane_id="pane-0003")


class TestProcessInvocation(unittest.TestCase):
    """Tests that exercise pane_switcher.py as a subprocess."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.socket_path = Path(self.tmp.name) / "herdr.sock"
        self.state_dir = Path(self.tmp.name) / "state"
        self.server = MockHerdrServer(
            self.socket_path, FIXTURE, current_pane_id=FIXTURE[0]["pane_id"]
        )

    def tearDown(self):
        self.server.stop()

    def _run(self, env):
        if "HERDR_SOCKET_PATH" not in env:
            env["HERDR_SOCKET_PATH"] = str(self.socket_path)
        env["PATH"] = os.environ.get("PATH", "")
        return subprocess.run(
            [sys.executable, str(PANE_SWITCHER_SCRIPT)],
            cwd=str(REPO_ROOT),
            env=env,
            capture_output=True,
            text=True,
        )

    def test_missing_state_dir_exits_nonzero(self):
        env = {
            "HERDR_PLUGIN_ACTION_ID": "cycle",
            "HERDR_PANE_ID": "pane-0000",
        }
        result = self._run(env)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("HERDR_PLUGIN_STATE_DIR", result.stderr)

    def test_missing_socket_path_exits_nonzero(self):
        env = {
            "HERDR_PLUGIN_ACTION_ID": "cycle",
            "HERDR_PANE_ID": "pane-0000",
            "HERDR_PLUGIN_STATE_DIR": str(self.state_dir),
            "HERDR_SOCKET_PATH": "",
            "XDG_CONFIG_HOME": self.tmp.name,
        }
        result = self._run(env)
        self.assertNotEqual(result.returncode, 0)

    def test_malformed_event_json_exits_nonzero(self):
        env = {
            "HERDR_PLUGIN_EVENT": "pane.focused",
            "HERDR_PLUGIN_EVENT_JSON": "not json",
            "HERDR_PLUGIN_STATE_DIR": str(self.state_dir),
        }
        result = self._run(env)
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
