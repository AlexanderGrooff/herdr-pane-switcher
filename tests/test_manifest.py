#!/usr/bin/env python3
"""Static checks for the plugin manifest and repository layout."""

import re
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


class TestManifest(unittest.TestCase):
    def test_plugin_id_is_herdr_pane_switcher(self):
        text = (REPO_ROOT / "herdr-plugin.toml").read_text()
        match = re.search(r'^id\s*=\s*"([^"]+)"', text, re.MULTILINE)
        self.assertIsNotNone(match, "plugin id not found in herdr-plugin.toml")
        self.assertEqual(match.group(1), "herdr.pane-switcher")

    def test_plugin_commands_use_rust_binary(self):
        text = (REPO_ROOT / "herdr-plugin.toml").read_text()
        current_section = None
        for line in text.splitlines():
            header = re.match(r"^\[\[(\w+)\]\]", line)
            if header:
                current_section = header.group(1)
                continue
            if current_section in ("actions", "events") and line.strip().startswith("command"):
                self.assertNotIn("pane_switcher.py", line, line)
                self.assertIn("herdr-mru-cycle", line, line)

    def test_build_command_runs_install_script(self):
        text = (REPO_ROOT / "herdr-plugin.toml").read_text()
        self.assertIn('command = ["bash", "herdr/install.sh"]', text)

    def test_python_plugin_removed_from_root(self):
        self.assertFalse(
            (REPO_ROOT / "pane_switcher.py").exists(),
            "pane_switcher.py should be removed from the repo root; keep baseline in benchmarks/",
        )

    def test_request_id_prefix_in_rust_source(self):
        source = (REPO_ROOT / "src" / "main.rs").read_text()
        self.assertIn('"plugin:pane-switcher:', source)


if __name__ == "__main__":
    unittest.main()
