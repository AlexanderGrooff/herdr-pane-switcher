"""Shared mock Herdr Unix socket server for tests and benchmarks."""

import json
import select
import socket
import threading
from pathlib import Path


class MockHerdrServer:
    """Single-threaded Unix socket server that replies like Herdr."""

    def __init__(self, socket_path, panes, current_pane_id=None):
        self.socket_path = Path(socket_path)
        self.panes = panes
        self.current_pane_id = current_pane_id
        self.focus_calls = []
        self.requests = []
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
                response = self._dispatch(
                    request.get("method"), request.get("params", {})
                )
            except Exception as exc:  # noqa: BLE001
                response = {"error": str(exc)}
            conn.sendall((json.dumps(response) + "\n").encode())

    def _dispatch(self, method, params):
        self.requests.append((method, params))
        if method == "pane.list":
            return {"result": {"panes": self.panes}}
        if method == "pane.focus":
            pane_id = params.get("pane_id")
            if pane_id:
                self.current_pane_id = pane_id
                self.focus_calls.append(pane_id)
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
