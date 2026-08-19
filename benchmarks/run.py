#!/usr/bin/env python3
"""Benchmark harness for herdr-pane-switcher.

Starts a temporary Unix socket server that replies with deterministic
pane.list / pane.focus / pane.current responses, then runs the plugin's
actions and events against isolated temp state directories.
"""

import argparse
import csv
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PANE_SWITCHER_SCRIPT = REPO_ROOT / "pane_switcher.py"
OUTPUT_DIR = REPO_ROOT / "benchmarks" / "output"

sys.path.insert(0, str(REPO_ROOT))
from helpers.mock_herdr_server import MockHerdrServer  # noqa: E402

FIXTURE_SIZES = [5, 50, 500]
ACTIONS = ["cycle", "focus-attention", "cycle-attention", "pane.focused", "pane.closed"]
DEFAULT_ITERATIONS = 100
DEFAULT_WARMUP = 5

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


def run_plugin(env):
    """Execute pane_switcher.py and return the subprocess result."""
    return subprocess.run(
        [sys.executable, str(PANE_SWITCHER_SCRIPT)],
        env=env,
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
    )


def setup_event(state_dir, pane_id, socket_path, event="pane.focused"):
    """Run an untimed setup event (defaults to pane.focused)."""
    env = os.environ.copy()
    env["HERDR_SOCKET_PATH"] = str(socket_path)
    env["HERDR_PLUGIN_STATE_DIR"] = str(state_dir)
    env["HERDR_PLUGIN_EVENT"] = event
    env["HERDR_PLUGIN_EVENT_JSON"] = json.dumps({"pane_id": pane_id})
    result = run_plugin(env)
    if result.returncode != 0:
        raise RuntimeError(f"setup {event} for {pane_id} failed: {result.stderr}")


def _cycle_setup(size):
    return {"event": "pane.focused", "focus_indices": list(range(size))}


def _cycle_runtime(size, panes):
    return {"current_pane": panes[size - 1]["pane_id"], "event_json": None}


def _attention_runtime(_size, panes):
    return {"current_pane": panes[0]["pane_id"], "event_json": None}


def _pane_focused_setup(size):
    return {"event": "pane.focused", "focus_indices": [1 if size > 1 else 0]}


def _pane_focused_runtime(_size, panes):
    pane_id = panes[0]["pane_id"]
    return {"current_pane": pane_id, "event_json": {"pane_id": pane_id}}


def _pane_closed_setup(size):
    return {"event": "pane.focused", "focus_indices": list(range(min(3, size)))}


def _pane_closed_runtime(size, panes):
    idx = min(2, size - 1)
    pane_id = panes[idx]["pane_id"]
    return {"current_pane": pane_id, "event_json": {"pane_id": pane_id}}


# Centralized per-action metadata: setup parameters and runtime context.
ACTION_CONFIG = {
    "cycle": {"setup": _cycle_setup, "runtime": _cycle_runtime},
    "focus-attention": {"setup": None, "runtime": _attention_runtime},
    "cycle-attention": {"setup": None, "runtime": _attention_runtime},
    "pane.focused": {"setup": _pane_focused_setup, "runtime": _pane_focused_runtime},
    "pane.closed": {"setup": _pane_closed_setup, "runtime": _pane_closed_runtime},
}


def build_golden_state(state_dir, action, panes, size, socket_path):
    """Populate an isolated state directory for a given action/fixture."""
    state_dir.mkdir(parents=True, exist_ok=True)
    config = ACTION_CONFIG[action]
    if config["setup"] is None:
        return
    setup = config["setup"](size)
    for idx in setup["focus_indices"]:
        setup_event(
            state_dir,
            panes[idx]["pane_id"],
            socket_path,
            event=setup["event"],
        )


def action_context(action, panes, size):
    """Return the current pane and event payload needed for an action."""
    config = ACTION_CONFIG.get(action)
    if config is None:
        raise ValueError(f"unknown action: {action}")
    ctx = config["runtime"](size, panes)
    return ctx["current_pane"], ctx["event_json"]


def percentile(sorted_values, p):
    """Linear-interpolation percentile for 0 <= p <= 1."""
    if not sorted_values:
        return 0.0
    k = (len(sorted_values) - 1) * p
    f = math.floor(k)
    c = math.ceil(k)
    if f == c:
        return sorted_values[int(k)]
    lower = sorted_values[int(f)]
    upper = sorted_values[int(c)]
    return lower * (c - k) + upper * (k - f)


def benchmark_action(
    action,
    panes,
    size,
    server,
    socket_path,
    golden_state,
    iterations,
    warmup,
    current_pane,
    event_json,
    tmp_root,
):
    """Run a timed benchmark for one action/fixture and return raw samples."""
    samples = []
    for i in range(-warmup, iterations):
        iter_dir = Path(tempfile.mkdtemp(dir=tmp_root, prefix=f"iter-{size}-{action}-"))
        golden_state_json = golden_state / "state.json"
        if golden_state_json.exists():
            shutil.copy(golden_state_json, iter_dir / "state.json")

        env = os.environ.copy()
        env["HERDR_SOCKET_PATH"] = str(socket_path)
        env["HERDR_PLUGIN_STATE_DIR"] = str(iter_dir)
        if event_json is not None:
            env["HERDR_PLUGIN_EVENT"] = action
            env["HERDR_PLUGIN_EVENT_JSON"] = json.dumps(event_json)
        else:
            env["HERDR_PLUGIN_ACTION_ID"] = action
            if current_pane:
                env["HERDR_PANE_ID"] = current_pane

        start = time.perf_counter_ns()
        result = run_plugin(env)
        elapsed_ns = time.perf_counter_ns() - start

        shutil.rmtree(iter_dir, ignore_errors=True)

        if result.returncode != 0:
            raise RuntimeError(f"{action} failed (fixture {size}): {result.stderr}")

        if i >= 0:
            samples.append(
                {
                    "fixture": size,
                    "action": action,
                    "iteration": i,
                    "elapsed_ms": elapsed_ns / 1_000_000.0,
                }
            )

    return samples


def write_outputs(
    results,
    all_samples,
    report_path,
    sample_json_path,
    sample_csv_path,
    iterations,
    warmup,
):
    """Write Markdown report, JSON samples, and CSV samples."""
    sample_json_path.write_text(json.dumps(all_samples, indent=2))

    with sample_csv_path.open("w", newline="") as fh:
        writer = csv.DictWriter(
            fh, fieldnames=["fixture", "action", "iteration", "elapsed_ms"]
        )
        writer.writeheader()
        writer.writerows(all_samples)

    rows = []
    for size, action, samples in results:
        elapsed = [s["elapsed_ms"] for s in samples]
        elapsed.sort()
        rows.append(
            {
                "fixture": size,
                "action": action,
                "p50_ms": percentile(elapsed, 0.5),
                "p95_ms": percentile(elapsed, 0.95),
                "p99_ms": percentile(elapsed, 0.99),
            }
        )

    report_lines = [
        "# herdr-pane-switcher Benchmark Report",
        "",
        f"- Generated: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
        f"- Python: {sys.version.split()[0]}",
        f"- Iterations per action/fixture: {iterations}",
        f"- Warmup iterations: {warmup}",
        "",
        "## Results by fixture and action",
        "",
        "| Fixture | Action | p50 (ms) | p95 (ms) | p99 (ms) |",
        "|---|---|---:|---:|---:|",
    ]
    for row in rows:
        report_lines.append(
            f"| {row['fixture']} | {row['action']} | "
            f"{row['p50_ms']:.3f} | {row['p95_ms']:.3f} | {row['p99_ms']:.3f} |"
        )
    report_lines.append("")

    report_text = "\n".join(report_lines)
    report_path.write_text(report_text)
    return report_text


def main():
    parser = argparse.ArgumentParser(description="Benchmark herdr-pane-switcher plugin")
    parser.add_argument(
        "--sizes",
        type=int,
        nargs="+",
        default=FIXTURE_SIZES,
        help="fixture pane counts",
    )
    parser.add_argument(
        "--actions", nargs="+", default=ACTIONS, help="actions/events to benchmark"
    )
    parser.add_argument(
        "--iterations",
        type=int,
        default=DEFAULT_ITERATIONS,
        help="timed iterations per action/fixture",
    )
    parser.add_argument(
        "--warmup",
        type=int,
        default=DEFAULT_WARMUP,
        help="discarded warmup iterations per action/fixture",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=OUTPUT_DIR,
        help="directory for report and sample files",
    )
    args = parser.parse_args()

    args.output_dir.mkdir(parents=True, exist_ok=True)

    all_samples = []
    results = []

    with tempfile.TemporaryDirectory(dir="/tmp", prefix="pane-switcher-bench-") as tmp_base:
        tmp_base = Path(tmp_base)
        for size in args.sizes:
            panes = generate_panes(size)
            golden_base = tmp_base / f"fixture-{size}-golden"
            golden_base.mkdir()

            socket_path = tmp_base / f"herdr-{size}.sock"
            server = MockHerdrServer(
                socket_path, panes, current_pane_id=panes[0]["pane_id"]
            )

            golden_states = {}
            for action in args.actions:
                golden_state = golden_base / action
                build_golden_state(golden_state, action, panes, size, socket_path)
                golden_states[action] = golden_state

            try:
                for action in args.actions:
                    current_pane, event_json = action_context(action, panes, size)
                    samples = benchmark_action(
                        action=action,
                        panes=panes,
                        size=size,
                        server=server,
                        socket_path=socket_path,
                        golden_state=golden_states[action],
                        iterations=args.iterations,
                        warmup=args.warmup,
                        current_pane=current_pane,
                        event_json=event_json,
                        tmp_root=tmp_base,
                    )
                    all_samples.extend(samples)
                    results.append((size, action, samples))
            finally:
                server.stop()

    report_path = args.output_dir / "report.md"
    sample_json_path = args.output_dir / "samples.json"
    sample_csv_path = args.output_dir / "samples.csv"

    report_text = write_outputs(
        results,
        all_samples,
        report_path,
        sample_json_path,
        sample_csv_path,
        args.iterations,
        args.warmup,
    )
    print(report_text)


if __name__ == "__main__":
    main()
