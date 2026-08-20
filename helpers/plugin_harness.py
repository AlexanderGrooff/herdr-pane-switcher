"""Shared plugin process and fixture helpers for tests and benchmarks."""

import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BASELINE_SCRIPT = REPO_ROOT / "benchmarks" / "baseline.py"
RUST_BINARY = REPO_ROOT / "bin" / "herdr-mru-cycle"
PLUGIN_TIMEOUT_SECONDS = 15.0
STATUS_ROTATION = ["blocked", "done", "idle", "working"]


def generate_panes(size):
    """Return a fixture with `size` panes and a mix of agent_status values."""
    return [
        {
            "pane_id": f"pane-{i:04d}",
            "agent_status": STATUS_ROTATION[i % len(STATUS_ROTATION)],
            "title": f"Pane {i}",
        }
        for i in range(size)
    ]


def run_impl(impl, env):
    """Execute one implementation and return the subprocess result."""
    if impl == "python":
        cmd = [sys.executable, str(BASELINE_SCRIPT)]
    elif impl == "rust":
        cmd = [str(RUST_BINARY)]
    else:
        raise ValueError(f"unknown impl: {impl}")
    try:
        return subprocess.run(
            cmd,
            env=env,
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            timeout=PLUGIN_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(
            f"{impl} plugin timed out after {PLUGIN_TIMEOUT_SECONDS:.1f}s"
        ) from exc
