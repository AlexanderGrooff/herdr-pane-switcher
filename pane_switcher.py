#!/usr/bin/env python3

import fcntl
import json
import os
import socket
import time
from contextlib import contextmanager
from pathlib import Path

CYCLE_TIMEOUT_SECONDS = 1.0
STATUS_PRIORITY = {
    "blocked": 0,
    "done": 1,
    "idle": 2,
    "working": 3,
}


def nested_value(value, key):
    if isinstance(value, dict):
        if value.get(key):
            return value[key]
        for child in value.values():
            found = nested_value(child, key)
            if found:
                return found
    elif isinstance(value, list):
        for child in value:
            found = nested_value(child, key)
            if found:
                return found
    return None


def herdr_socket_path():
    if sock_path := os.environ.get("HERDR_SOCKET_PATH"):
        return sock_path
    config_dir = os.environ.get("XDG_CONFIG_HOME", str(Path.home() / ".config"))
    return os.path.join(config_dir, "herdr", "herdr.sock")


def herdr_request(method, params):
    request_id = f"plugin:pane-switcher:{method}:{time.time_ns()}"
    payload = {"id": request_id, "method": method, "params": params}
    sock_path = herdr_socket_path()

    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(10.0)
        sock.connect(sock_path)
        with sock.makefile("r") as reader:
            sock.sendall((json.dumps(payload) + "\n").encode())
            response = reader.readline()

    if not response:
        raise RuntimeError(f"no response from herdr for {method}")

    data = json.loads(response)
    if "error" in data:
        raise RuntimeError(f"herdr {method} failed: {data['error']}")
    return data


def all_panes():
    payload = herdr_request("pane.list", {})
    return nested_value(payload, "panes") or []


def all_pane_ids():
    return [pane["pane_id"] for pane in all_panes()]


def focus_pane(pane_id):
    herdr_request("pane.focus", {"pane_id": pane_id})


def current_pane_id():
    payload = json.loads(os.environ.get("HERDR_PLUGIN_EVENT_JSON", "{}"))
    event = os.environ.get("HERDR_PLUGIN_EVENT")

    if event:
        pane_id = nested_value(payload, "pane_id")
        if pane_id:
            return pane_id

    return (
        os.environ.get("HERDR_PANE_ID")
        or nested_value(payload, "focused_pane_id")
        or nested_value(payload, "pane_id")
    )


def active_pane_id():
    pane_id = current_pane_id()
    if pane_id:
        return pane_id

    try:
        return nested_value(herdr_request("pane.current", {}), "pane_id")
    except Exception:
        return None


@contextmanager
def locked_state():
    state_dir = Path(os.environ["HERDR_PLUGIN_STATE_DIR"])
    state_dir.mkdir(parents=True, exist_ok=True)
    lock_path = state_dir / "state.lock"
    state_path = state_dir / "state.json"
    with lock_path.open("a+") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            state = json.loads(state_path.read_text()) if state_path.exists() else {}
        except (json.JSONDecodeError, OSError):
            state = {}

        if not isinstance(state.get("history"), list):
            state["history"] = []
        if not isinstance(state.get("cycle"), dict):
            state["cycle"] = {}

        yield state

        temporary_path = state_path.with_suffix(".tmp")
        temporary_path.write_text(json.dumps(state, separators=(",", ":")))
        temporary_path.replace(state_path)


def promote(items, item):
    return [item, *(existing for existing in items if existing != item)]


def record_event(pane_id):
    with locked_state() as state:
        if os.environ.get("HERDR_PLUGIN_EVENT") == "pane.closed":
            state["history"] = [
                existing for existing in state["history"] if existing != pane_id
            ]
            state["cycle"] = {}
            return

        state["history"] = promote(state["history"], pane_id)
        cycle = state.get("cycle")
        if cycle and cycle.get("target") != pane_id:
            state["cycle"] = {}


def cycle_panes(current_pane_id):
    panes = all_pane_ids()
    if len(panes) < 2:
        return

    now = time.monotonic()
    with locked_state() as state:
        history = [pane_id for pane_id in state["history"] if pane_id in panes]
        history = promote(history, current_pane_id)

        seen = set(history)
        history.extend(pane_id for pane_id in panes if pane_id not in seen)

        cycle = state.get("cycle")
        elapsed = now - cycle["last_at"] if cycle else None
        continuing = (
            cycle
            and elapsed is not None
            and 0 <= elapsed <= CYCLE_TIMEOUT_SECONDS
            and cycle.get("target") == current_pane_id
            and all(pane_id in panes for pane_id in cycle["order"])
        )

        if continuing:
            order = cycle["order"]
            index = (cycle["index"] + 1) % len(order)
        else:
            order = history
            index = 1

        target = order[index]
        state["history"] = promote(history, target)
        state["cycle"] = {
            "order": order,
            "index": index,
            "target": target,
            "last_at": now,
        }

    focus_pane(target)


def attention_panes():
    panes = [
        pane for pane in all_panes() if pane.get("agent_status") in STATUS_PRIORITY
    ]
    panes.sort(key=lambda pane: STATUS_PRIORITY[pane["agent_status"]])
    return panes


def focus_attention(next_pane=False):
    attention = attention_panes()
    if not attention:
        return

    if next_pane:
        current = active_pane_id()
        if current:
            index = next(
                (i for i, pane in enumerate(attention) if pane["pane_id"] == current),
                None,
            )
            if index is not None and index + 1 < len(attention):
                target = attention[index + 1]
            else:
                target = attention[0]
        else:
            target = attention[0]
    else:
        target = attention[0]

    focus_pane(target["pane_id"])


def main():
    event = os.environ.get("HERDR_PLUGIN_EVENT")
    action = os.environ.get("HERDR_PLUGIN_ACTION_ID")

    if event in {"pane.focused", "pane.closed"}:
        pane_id = current_pane_id()
        if pane_id:
            record_event(pane_id)
    elif action == "focus-attention":
        focus_attention(next_pane=False)
    elif action == "cycle-attention":
        focus_attention(next_pane=True)
    else:
        pane_id = active_pane_id()
        if pane_id:
            cycle_panes(pane_id)


if __name__ == "__main__":
    main()
