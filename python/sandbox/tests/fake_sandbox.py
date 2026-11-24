"""An in-process sandbox API on a real local port. A box is a temp directory: a box path ``/x`` is
``<root>/x`` in file calls and in every argument and ``cwd`` of a command, which runs on the host.
A command named :data:`RELAY` runs :class:`FakeRelay` instead. Output comes back in frames of at
most :data:`FRAME` bytes, each poll repeating the byte before the asked offset, as a server
answering a retried poll may. Creates and starts replay their answer to a repeated
``Idempotency-Key``."""

from __future__ import annotations

import asyncio
import base64
import json
import os
import shutil
import signal as signals
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import IO, Any
from urllib.parse import parse_qs, unquote, urlsplit

import h2.config
import h2.connection
import h2.events

KEY = "good-key"
RELAY = "/opt/epochal/bin/epochal-relay"
NO_BOOT = "2001-01-01T00:00:00Z"
"""An as-of whose box never boots."""
FRAME = 7
RANGE_CAP = 4096
"""The most bytes one ranged read returns, so a file comes back over several."""
_PREFIX = "/v1/time-portal/sandbox"

Reply = tuple[int, dict[str, str], bytes]


def _json(status: int, value: Any) -> Reply:
    return status, {"content-type": "application/json"}, json.dumps(value).encode()


def _error(status: int, kind: str, message: str, **extra: Any) -> Reply:
    return _json(status, {"error": {"type": kind, "message": message, **extra}})


_NOT_FOUND = _error(404, "not_found", "No such object.")


@dataclass
class Seen:
    method: str
    path: str
    headers: dict[str, str]
    body: bytes


@dataclass
class Exec:
    argv: list[str]
    frame: int = FRAME
    stdout: bytearray = field(default_factory=bytearray)
    stderr: bytearray = field(default_factory=bytearray)
    exit: dict[str, Any] | None = None
    changed: threading.Condition = field(default_factory=threading.Condition)
    next_seq: int = 0
    stdin: Callable[[bytes | None], None] | None = None
    """Writes to stdin, or closes it on ``None``; unset once closed."""
    signal: Callable[[int], None] = lambda _: None

    def append(self, stream: str, data: bytes) -> None:
        with self.changed:
            (self.stdout if stream == "stdout" else self.stderr).extend(data)
            self.changed.notify_all()

    def end(self, exit_: dict[str, Any]) -> None:
        with self.changed:
            self.exit = self.exit or exit_
            self.changed.notify_all()


@dataclass
class Box:
    root: Path
    info: dict[str, Any]
    execs: dict[str, Exec] = field(default_factory=dict)


class FakeSandboxApi:
    def __init__(self) -> None:
        self.boxes: dict[str, Box] = {}
        self.log: list[Seen] = []
        self._lock = threading.Lock()
        self._answered: dict[str, Reply] = {}
        self._lose = 0
        self._garble = 0
        self.unknown_sizes = False
        """Range reads answer ``bytes a-b/*``, as a box that doesn't know a file's size may."""
        self.file_delay = 0.0
        """Seconds each file read or write takes, so calls that overlap show in the peak."""
        self.short_ranges: int | None = None
        """The most bytes a range read past byte 0 returns, the whole size still named."""
        self.peak_file_calls = 0
        """The most file reads and writes in flight at once."""
        self._file_calls = 0
        self._started_writes: set[int] = set()
        self._write_started = threading.Condition(self._lock)
        self.hold_write: tuple[int, int] | None = None
        """``(offset, until)``: holds the write at ``offset`` until the one at ``until`` starts,
        or 10 s pass."""
        self.passed_held_write: bool | None = None
        """Whether the write at ``until`` started while the held one was held."""
        self._dirs: list[tempfile.TemporaryDirectory[str]] = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _serve(self) -> None:
                length = int(self.headers.get("content-length") or 0)
                body = self.rfile.read(length)
                status, headers, reply = fake.handle(self.command, self.path, self.headers, body)
                self.send_response(status)
                for name, value in headers.items():
                    self.send_header(name, value)
                self.send_header("content-length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)

            do_GET = do_POST = do_PUT = do_DELETE = _serve

            def log_message(self, *_: Any) -> None:
                pass

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self.url = f"http://127.0.0.1:{self._server.server_address[1]}"
        serve = threading.Thread(target=self._server.serve_forever, args=(0.01,), daemon=True)
        serve.start()

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        for box in self.boxes.values():
            for execution in box.execs.values():
                execution.signal(signals.SIGKILL)
        for d in self._dirs:
            d.cleanup()

    def lose_replies(self, calls: int) -> None:
        """Carries out each of the next ``calls`` calls but answers it with a 503."""
        self._lose = calls

    def garble_replies(self, calls: int) -> None:
        """Carries out each of the next ``calls`` calls but answers it with an unreadable 200."""
        self._garble = calls

    def root(self, box_id: str) -> Path:
        return self.boxes[box_id].root

    def handle(self, method: str, target: str, headers: Any, body: bytes) -> Reply:
        url = urlsplit(target)
        self.log.append(Seen(method, url.path, dict(headers.items()), body))
        if headers.get("x-api-key") != KEY:
            return _error(401, "invalid_api_key", "No key matches that token.")
        key = headers.get("idempotency-key")
        replay = f"{method} {url.path} {key}" if key else None
        with self._lock:
            if replay and replay in self._answered:
                return self._answered[replay]
        query = {k: v[0] for k, v in parse_qs(url.query).items()}
        reply = self._route(method, unquote(url.path), query, dict(headers.items()), body)
        with self._lock:
            if replay and 200 <= reply[0] < 300:
                self._answered[replay] = reply
            if self._lose:
                self._lose -= 1
                return 503, {}, b""
            if self._garble:
                self._garble -= 1
                return 200, {"content-type": "application/json"}, b"{"
        return reply

    def _route(
        self, method: str, path: str, q: dict[str, str], headers: dict[str, str], body: bytes
    ) -> Reply:
        if path in ("/v1/time-portal/search", "/v1/time-portal/fetch"):
            return _web(path, headers["accept-datetime"], json.loads(body))
        if path == _PREFIX and method == "POST":
            return self._create(headers["accept-datetime"], json.loads(body))
        parts = path.removeprefix(_PREFIX + "/").split("/")
        box = self.boxes.get(parts[0])
        if box is None:
            return _NOT_FOUND
        rest = "/".join(parts[1:])
        if rest == "":
            return self._read(box) if method == "GET" else self._terminate(box)
        if box.info["state"] != "live":
            return _error(409, "conflict", "The sandbox is not live.")
        if rest == "execs":
            return self._start(box, json.loads(body))
        if rest.startswith("execs/"):
            execution = box.execs.get(parts[2])
            if execution is None:
                return _NOT_FOUND
            if parts[3] == "output":
                return self._output(execution, q)
            return self._input(execution, json.loads(body))
        host = Path(str(box.root) + q["path"])
        if rest == "files/stat":
            return self._stat(q["path"], host)
        if method == "DELETE":
            return self._remove(host, q.get("recursive") == "true")
        if method == "PUT":
            self._hold(int(q.get("offset", 0)))
        with self._lock:
            self._file_calls += 1
            self.peak_file_calls = max(self.peak_file_calls, self._file_calls)
        try:
            time.sleep(self.file_delay)
            if method == "GET":
                return self._read_file(host, headers.get("range", "bytes=0-"))
            return self._write_file(q["path"], host, int(q.get("offset", 0)), body)
        finally:
            with self._lock:
                self._file_calls -= 1

    def _hold(self, offset: int) -> None:
        with self._write_started:
            self._started_writes.add(offset)
            self._write_started.notify_all()
            if self.hold_write is None or self.hold_write[0] != offset:
                return
            until = self.hold_write[1]
            self.passed_held_write = self._write_started.wait_for(
                lambda: until in self._started_writes, timeout=10
            )

    def _create(self, as_of: str, body: dict[str, Any]) -> Reply:
        temp = tempfile.TemporaryDirectory()
        self._dirs.append(temp)
        root = Path(temp.name)
        for d in ("tmp", "home/epochal"):
            (root / d).mkdir(parents=True)
        with self._lock:
            box_id = f"sbx_{len(self.boxes) + 1}"
            info = {
                "id": box_id,
                "state": "creating",
                "as_of": as_of,
                "network": body.get("network", "none"),
                "vcpus": body.get("vcpus", 1),
                "memory_gib": body.get("memory_gib", 1),
                "timeout": body.get("timeout", "1h"),
                "idle_timeout": body.get("idle_timeout", "10m"),
                "created_at": "2026-11-09T10:00:00Z",
                "apps": [{"name": a, "version": "1.4.2"} for a in body.get("apps", [])],
            }
            self.boxes[box_id] = Box(root, info)
        return _json(202, info)

    def _read(self, box: Box) -> Reply:
        """A box answers ``creating`` once, then boots, unless its as-of is :data:`NO_BOOT`."""
        if box.info["state"] == "creating":
            if box.info["as_of"] == NO_BOOT:
                box.info["state"] = "failed"
                box.info["error"] = {"type": "sandbox_lost", "message": "The host went away."}
            else:
                box.info["state"] = "live"
        return _json(200, box.info)

    def _terminate(self, box: Box) -> Reply:
        box.info["state"] = "ended"
        for execution in box.execs.values():
            execution.signal(signals.SIGKILL)
        return 204, {}, b""

    def _start(self, box: Box, body: dict[str, Any]) -> Reply:
        argv = body["argv"]
        execution = Exec(argv)
        if argv[0] == RELAY:
            execution.frame = 64 << 10
            FakeRelay(int(argv[1]), execution)
        else:
            _spawn(box.root, execution, body)
        with self._lock:
            exec_id = f"exec_{len(box.execs) + 1}"
            box.execs[exec_id] = execution
        return _json(200, {"id": exec_id})

    def _output(self, execution: Exec, q: dict[str, str]) -> Reply:
        offsets = int(q["stdout_offset"]), int(q["stderr_offset"])

        def ready() -> bool:
            return (
                len(execution.stdout) > offsets[0]
                or len(execution.stderr) > offsets[1]
                or execution.exit is not None
            )

        with execution.changed:
            execution.changed.wait_for(ready, timeout=2)
            frames = _frames("stdout", execution.stdout, offsets[0], execution.frame)
            frames += _frames("stderr", execution.stderr, offsets[1], execution.frame)
            return _json(200, {"frames": frames, "exit": execution.exit})

    def _input(self, execution: Exec, body: dict[str, Any]) -> Reply:
        seq = body["seq"]
        with self._lock:
            if seq == execution.next_seq - 1:
                return _json(200, {"next_seq": execution.next_seq})
            if seq != execution.next_seq:
                return _error(409, "conflict", "Out of order.", next_seq=execution.next_seq)
            if "data_b64" in body or body.get("eof"):
                if execution.stdin is None:
                    return _error(409, "conflict", "Stdin is closed.")
                if body.get("eof"):
                    execution.stdin(None)
                    execution.stdin = None
                else:
                    execution.stdin(base64.b64decode(body["data_b64"]))
            if "signal" in body:
                if execution.exit is not None:
                    return _error(409, "conflict", "The command has ended.")
                execution.signal(body["signal"])
            execution.next_seq += 1
            return _json(200, {"next_seq": execution.next_seq})

    def _stat(self, path: str, host: Path) -> Reply:
        try:
            st = host.lstat()
        except FileNotFoundError:
            return _NOT_FOUND
        kind = "symlink" if host.is_symlink() else "directory" if host.is_dir() else "file"
        return _json(
            200,
            {"path": path, "type": kind, "size": st.st_size, "mode": st.st_mode & 0o7777},
        )

    def _read_file(self, host: Path, range_: str) -> Reply:
        try:
            data = host.read_bytes()
        except (FileNotFoundError, IsADirectoryError):
            return _NOT_FOUND
        if not data:
            return 200, {}, b""
        first, _, last = range_.removeprefix("bytes=").partition("-")
        start, total = int(first), len(data)
        if start >= total:
            return 416, {"content-range": f"bytes */{total}"}, b""
        end = min(int(last) + 1 if last else total, total, start + RANGE_CAP)
        if start and self.short_ranges is not None:
            end = min(end, start + self.short_ranges)
        size = "*" if self.unknown_sizes else total
        return 206, {"content-range": f"bytes {start}-{end - 1}/{size}"}, data[start:end]

    def _write_file(self, path: str, host: Path, offset: int, data: bytes) -> Reply:
        host.parent.mkdir(parents=True, exist_ok=True)
        with open(host, "r+b" if offset and host.exists() else "wb") as f:
            f.seek(offset)
            f.write(data)
        return self._stat(path, host)

    def _remove(self, host: Path, recursive: bool) -> Reply:
        if not host.exists():
            return _NOT_FOUND
        if host.is_dir():
            if not recursive:
                return _error(409, "conflict", "Directory not empty.")
            shutil.rmtree(host)
        else:
            host.unlink()
        return 204, {}, b""


def _frames(stream: str, data: bytearray, offset: int, size: int) -> list[dict[str, Any]]:
    """Everything from ``offset`` on, starting one byte early when there is new data past it."""
    if len(data) <= offset:
        return []
    start = max(offset - 1, 0)
    return [
        {
            "stream": stream,
            "offset": at,
            "data_b64": base64.b64encode(data[at : at + size]).decode(),
        }
        for at in range(start, len(data), size)
    ]


def _web(path: str, as_of: str, body: dict[str, Any]) -> Reply:
    """One search hit naming the query, or the fetched page naming its URL and the as-of."""
    meta = {"as_of": as_of, "request_id": "req_1"}
    if path.endswith("/search"):
        hit = {"title": f"About {body['query']}", "url": "https://example.com/a", "snippet": "…"}
        return _json(200, {**meta, "results": [hit], "entities": []})
    page = {"status": "success", "url": body["url"], "final_url": body["url"], "title": "A"}
    read = {"content": f"read at {as_of}", "truncated": False, "entity": None}
    return _json(200, {**meta, **page, **read})


def _reroot(root: Path, arg: str) -> str:
    return f"{root}{arg}" if arg.startswith("/") else arg


def _spawn(root: Path, execution: Exec, body: dict[str, Any]) -> None:
    stdin_data = base64.b64decode(body.get("stdin_b64", ""))
    proc = subprocess.Popen(
        [_reroot(root, a) for a in body["argv"]],
        cwd=_reroot(root, body.get("cwd") or "/"),
        # Keeps macOS tar from adding AppleDouble `._*` entries to archives.
        env={**os.environ, "COPYFILE_DISABLE": "1", **body.get("env", {})},
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    assert proc.stdin
    assert proc.stdout
    assert proc.stderr
    pipe = proc.stdin

    def write(data: bytes | None) -> None:
        if data is None:
            pipe.close()
            return
        try:
            pipe.write(data)
            pipe.flush()
        except BrokenPipeError:
            pass

    if stdin_data:
        write(stdin_data)
    if body.get("stdin_open"):
        execution.stdin = write
    else:
        pipe.close()
    execution.signal = proc.send_signal

    def pump(stream: str, source: IO[bytes]) -> None:
        with source:
            while chunk := source.read1(4096):  # type: ignore[attr-defined]
                execution.append(stream, chunk)

    readers = [
        threading.Thread(target=pump, args=("stdout", proc.stdout), daemon=True),
        threading.Thread(target=pump, args=("stderr", proc.stderr), daemon=True),
    ]
    for r in readers:
        r.start()

    def wait() -> None:
        code = proc.wait()
        for r in readers:
            r.join()
        if not pipe.closed:
            pipe.close()
        if code < 0:
            execution.end({"reason": "signaled", "signal": -code})
        else:
            execution.end({"reason": "exited", "code": code})

    threading.Thread(target=wait, daemon=True).start()


class FakeRelay:
    """The box's end of the tunnel, as the ``relay`` app speaks it: listens on
    ``127.0.0.1:<port>`` and, as the HTTP/2 client of the owner on its stdin and stdout, opens one
    ``POST /`` stream per connection."""

    def __init__(self, port: int, execution: Exec) -> None:
        self._exec = execution
        self._loop = asyncio.new_event_loop()
        self._conn = h2.connection.H2Connection(h2.config.H2Configuration(client_side=True))
        self._writers: dict[int, asyncio.StreamWriter] = {}
        self._halves: dict[int, int] = {}
        self._window = asyncio.Event()
        listening = threading.Event()
        execution.stdin = lambda data: self._loop.call_soon_threadsafe(self._feed, data)
        execution.signal = self._signal

        async def start() -> None:
            self._server = await asyncio.start_server(self._accept, "127.0.0.1", port)
            self._conn.initiate_connection()
            self._flush()
            listening.set()

        def run() -> None:
            self._loop.run_forever()
            self._loop.run_until_complete(asyncio.sleep(0))
            self._loop.close()

        threading.Thread(target=run, daemon=True).start()
        asyncio.run_coroutine_threadsafe(start(), self._loop)
        listening.wait(5)

    def _flush(self) -> None:
        if data := self._conn.data_to_send():
            self._exec.append("stdout", data)

    def _signal(self, sig: int) -> None:
        if not self._loop.is_closed():
            self._loop.call_soon_threadsafe(self._stop, sig)

    def _stop(self, sig: int) -> None:
        """Ends like a killed process: its listener, connections and loop all close."""
        self._server.close()
        for writer in self._writers.values():
            writer.close()
        for task in asyncio.all_tasks(self._loop):
            task.cancel()
        self._loop.call_soon(self._loop.stop)
        self._exec.end({"reason": "signaled", "signal": sig})

    def _feed(self, data: bytes | None) -> None:
        for event in self._conn.receive_data(data or b""):
            writer = self._writers.get(getattr(event, "stream_id", 0))
            if isinstance(event, h2.events.DataReceived) and writer:
                writer.write(event.data)
                self._conn.acknowledge_received_data(event.flow_controlled_length, event.stream_id)
            elif isinstance(event, h2.events.StreamEnded) and writer:
                if writer.can_write_eof():
                    writer.write_eof()
                self._half_closed(event.stream_id)
            elif isinstance(event, h2.events.StreamReset) and writer:
                writer.close()
            elif isinstance(event, h2.events.WindowUpdated | h2.events.RemoteSettingsChanged):
                self._window.set()
        self._flush()

    def _half_closed(self, sid: int) -> None:
        """Closes connection ``sid`` once both of its directions have ended."""
        self._halves[sid] = self._halves.get(sid, 0) + 1
        if self._halves[sid] == 2:
            self._writers.pop(sid).close()

    async def _accept(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        sid = self._conn.get_next_available_stream_id()
        self._writers[sid] = writer
        headers = [(":method", "POST"), (":path", "/"), (":scheme", "http"), (":authority", "t")]
        self._conn.send_headers(sid, headers)
        self._flush()
        try:
            while data := await reader.read(64 << 10):
                while data:
                    window = self._conn.local_flow_control_window(sid)
                    if window <= 0:
                        self._window.clear()
                        await self._window.wait()
                        continue
                    n = min(window, self._conn.max_outbound_frame_size, len(data))
                    self._conn.send_data(sid, data[:n])
                    self._flush()
                    data = data[n:]
            self._conn.end_stream(sid)
            self._flush()
        except Exception:
            writer.close()
            return
        self._half_closed(sid)
