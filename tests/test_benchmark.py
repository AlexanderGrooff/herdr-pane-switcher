#!/usr/bin/env python3
"""Tests for benchmark process safety and progress reporting."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO_ROOT))

from helpers import plugin_harness  # noqa: E402


class TestPluginHarness(unittest.TestCase):
    def test_run_impl_reports_timed_out_process(self):
        timeout_error = subprocess.TimeoutExpired(
            cmd=["plugin"], timeout=plugin_harness.PLUGIN_TIMEOUT_SECONDS
        )
        with mock.patch.object(
            plugin_harness.subprocess, "run", side_effect=timeout_error
        ):
            with self.assertRaisesRegex(RuntimeError, "timed out after"):
                plugin_harness.run_impl("rust", {})


class TestBenchmarkProgress(unittest.TestCase):
    def test_benchmark_reports_progress_before_working(self):
        with tempfile.TemporaryDirectory() as output_dir:
            result = subprocess.run(
                [
                    sys.executable,
                    str(REPO_ROOT / "benchmarks" / "run.py"),
                    "--sizes",
                    "5",
                    "--actions",
                    "cycle",
                    "--iterations",
                    "0",
                    "--warmup",
                    "0",
                    "--output-dir",
                    output_dir,
                ],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
                check=False,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("[benchmark]", result.stderr)
        self.assertIn("fixture=5", result.stderr)
        self.assertIn("action=cycle", result.stderr)


if __name__ == "__main__":
    unittest.main()
