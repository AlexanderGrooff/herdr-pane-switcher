#!/usr/bin/env python3
"""Benchmark harness for herdr-mru-cycle.

Starts a temporary Unix socket server that replies with deterministic
pane.list / pane.focus / pane.current responses, then runs the plugin's
actions and events against isolated temp state directories.
"""

import argparse
import csv
import json
import math
import os
import select
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MRU_TABS = REPO_ROOT / "mru_tabs.py"
OUTPUT_DIR = REPO_ROOT / "benchmarks" / "output"

FIXTURE_SIZES = [5, 50, 500]
ACTIONS = ["cycle", "focus-attention", "cycle-attention", "pane.focused", "pane.closed"]
DEFAULT_ITERATIONS = 100
DEFAULT_WARMUP = 5
MAX_SETUP_EVENTS = 50

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


class MockHerdrServer:
    """Single-threaded Unix socket server that replies like Herdr."""

    def __init__(self, socket_path, panes, current_pane_id=None):
        self.socket_path = Path(socket_path)
        self.panes = panes
        self.current_pane_id = current_pane_id
        self._shutdown = threading.Event()
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        if self.socket_path.exists():
            self.socket_path.unlink()
        self.sock.bind(str(self.socket_path))
        self.sock.listen(8)
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _serve(self):
        while not self._shutdown.is_set():
            try:
                readable, _, _ = select.select([self.sock], [], [], 0.1)
                if not readable:
                    continue
                conn, _ = self.sock.accept()
                self._handle(conn)
            except OSError:
                break

    def _handle(self, conn):
        with conn, conn.makefile("r") as reader:
            line = reader.readline()
            if not line:
                return
            try:
                request = json.loads(line)
                response = self._dispatch(request.get("method"), request.get("params", {}))
            except Exception as exc:  # noqa: BLE001
                response = {"error": str(exc)}
            conn.sendall((json.dumps(response) + "\n").encode())

    def _dispatch(self, method, params):
        if method == "pane.list":
            return {"result": {"panes": self.panes}}
        if method == "pane.focus":
            pane_id = params.get("pane_id")
            if pane_id:
                self.current_pane_id = pane_id
            return {"result": "ok"}
        if method == "pane.current":
            return {"result": {"pane_id": self.current_pane_id}}
        return {"error": f"unknown method: {method}"}

    def stop(self):
        self._shutdown.set()
        self._thread.join(timeout=2)
        try:
            self.sock.close()
        except OSError:
            pass
        if self.socket_path.exists():
            self.socket_path.unlink()


def run_plugin(env):
    """Execute mru_tabs.py and return the subprocess result."""
    return subprocess.run(
        [sys.executable, str(MRU_TABS)],
        env=env,
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
    )


def setup_focus_event(state_dir, pane_id, socket_path):
    """Run an untimed pane.focused setup event."""
    env = os.environ.copy()
    env["HERDR_SOCKET_PATH"] = str(socket_path)
    env["HERDR_PLUGIN_STATE_DIR"] = str(state_dir)
    env["HERDR_PLUGIN_EVENT"] = "pane.focused"
    env["HERDR_PLUGIN_EVENT_JSON"] = json.dumps({"pane_id": pane_id})
    result = run_plugin(env)
    if result.returncode != 0:
        raise RuntimeError(f"setup focus for {pane_id} failed: {result.stderr}")


def build_golden_state(state_dir, action, panes, size, socket_path):
    """Populate an isolated state directory for a given action/fixture."""
    state_dir.mkdir(parents=True, exist_ok=True)
    if action == "cycle":
        focused = min(size, MAX_SETUP_EVENTS)
        for i in range(focused):
            setup_focus_event(state_dir, panes[i]["pane_id"], socket_path)
    elif action == "pane.focused":
        setup_focus_event(state_dir, panes[1]["pane_id"], socket_path)
    elif action == "pane.closed":
        for i in range(min(3, size)):
            setup_focus_event(state_dir, panes[i]["pane_id"], socket_path)


def action_context(action, panes, size):
    """Return the current pane and event payload needed for an action."""
    if action == "cycle":
        focused = min(size, MAX_SETUP_EVENTS)
        return panes[focused - 1]["pane_id"], None
    if action == "focus-attention":
        return panes[0]["pane_id"], None
    if action == "cycle-attention":
        return panes[0]["pane_id"], None
    if action == "pane.focused":
        return panes[0]["pane_id"], {"pane_id": panes[0]["pane_id"]}
    if action == "pane.closed":
        return panes[2]["pane_id"], {"pane_id": panes[2]["pane_id"]}
    raise ValueError(f"unknown action: {action}")


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


def benchmark_action(action, panes, size, server, socket_path, golden_state,
                     iterations, warmup, current_pane, event_json, tmp_root):
    """Run a timed benchmark for one action/fixture and return raw samples."""
    samples = []
    for i in range(-warmup, iterations):
        iter_dir = Path(tempfile.mkdtemp(dir=tmp_root, prefix=f"iter-{size}-{action}-"))
        iter_dir.mkdir(parents=True, exist_ok=True)
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
            samples.append({
                "fixture": size,
                "action": action,
                "iteration": i,
                "elapsed_ms": elapsed_ns / 1_000_000.0,
            })

    return samples


def write_outputs(results, all_samples, report_path, sample_json_path, sample_csv_path,
                  iterations, warmup):
    """Write Markdown report, JSON samples, and CSV samples."""
    sample_json_path.write_text(json.dumps(all_samples, indent=2))

    with sample_csv_path.open("w", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=["fixture", "action", "iteration", "elapsed_ms"])
        writer.writeheader()
        writer.writerows(all_samples)

    rows = []
    for size, action, samples in results:
        elapsed = [s["elapsed_ms"] for s in samples]
        elapsed.sort()
        rows.append({
            "fixture": size,
            "action": action,
            "p50_ms": percentile(elapsed, 0.5),
            "p95_ms": percentile(elapsed, 0.95),
            "p99_ms": percentile(elapsed, 0.99),
        })

    report_lines = [
        "# herdr-mru-cycle Benchmark Report",
        "",
        f"- Generated: {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",
        f"- Python: {sys.version.split()[0]}",
        f"- Iterations per action/fixture: {iterations}",
        f"- Warmup iterations: {warmup}",
        "",
        "## Results by fixture and action",
        "",
        "| Fixture | Action | p50 (ms) | p95 (ms) | p99 (ms) |",
        "|---|---:|---:|---:|---:|",
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
    parser = argparse.ArgumentParser(description="Benchmark herdr-mru-cycle plugin")
    parser.add_argument("--sizes", type=int, nargs="+", default=FIXTURE_SIZES,
                        help="fixture pane counts")
    parser.add_argument("--actions", nargs="+", default=ACTIONS,
                        help="actions/events to benchmark")
    parser.add_argument("--iterations", type=int, default=DEFAULT_ITERATIONS,
                        help="timed iterations per action/fixture")
    parser.add_argument("--warmup", type=int, default=DEFAULT_WARMUP,
                        help="discarded warmup iterations per action/fixture")
    parser.add_argument("--output-dir", type=Path, default=OUTPUT_DIR,
                        help="directory for report and sample files")
    args = parser.parse_args()

    args.output_dir.mkdir(parents=True, exist_ok=True)

    all_samples = []
    results = []

    with tempfile.TemporaryDirectory(dir="/tmp", prefix="mru-bench-") as tmp_base:
        tmp_base = Path(tmp_base)
        for size in args.sizes:
            panes = generate_panes(size)
            golden_base = tmp_base / f"fixture-{size}-golden"
            golden_base.mkdir()

            socket_path = tmp_base / f"herdr-{size}.sock"
            server = MockHerdrServer(socket_path, panes, current_pane_id=panes[0]["pane_id"])

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
        results, all_samples, report_path, sample_json_path, sample_csv_path,
        args.iterations, args.warmup,
    )
    print(report_text)


if __name__ == "__main__":
    main()
