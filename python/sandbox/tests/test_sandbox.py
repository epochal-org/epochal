from __future__ import annotations

import asyncio
import contextlib
import datetime as dt
import http.server
import json
import os
import runpy
import signal
import socket
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any

import pytest
from epochal.api import APIError, DecodeError, InvalidArgumentError
from epochal.api.time_portal import ExitReason, FileType, Network, Stream
from epochal.sandbox import CommandError, SandboxNotLiveError, TunnelError, _tunnel
from epochal.sandbox import _common as c
from fake_sandbox import FRAME, KEY, NO_BOOT, RANGE_CAP, FakeSandboxApi
from sandbox_harness import Blocking

Create = Callable[..., Blocking]


def test_a_box_is_built_as_asked_reported_live_and_terminated(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create(
        as_of=dt.datetime(2025, 3, 1, 9, tzinfo=dt.timezone(dt.timedelta(hours=9))),
        network=Network.TIME_PORTAL,
        vcpus=2,
        memory_gib=4,
        timeout=dt.timedelta(hours=2),
        idle_timeout=dt.timedelta(minutes=5),
        env=("TOKEN", "secret"),
        label=("job", "nightly"),
        app="claude",
        relay=True,
    )

    assert fake.log[0].headers["accept-datetime"] == "2025-03-01T00:00:00Z"
    assert json.loads(fake.log[0].body) == {
        "network": "time-portal",
        "vcpus": 2,
        "memory_gib": 4,
        "timeout": "2h",
        "idle_timeout": "5m",
        "env": {"TOKEN": "secret"},
        "metadata": {"job": "nightly"},
        "apps": ["claude", "relay"],
    }
    assert box.id == "sbx_1"
    assert box.as_of == dt.datetime(2025, 3, 1, tzinfo=dt.UTC)
    assert box.info.network is Network.TIME_PORTAL
    assert fake.boxes["sbx_1"].info["state"] == "live"

    box.terminate()
    assert fake.boxes["sbx_1"].info["state"] == "ended"
    box.terminate()


def test_a_bare_day_is_held_to_its_last_second(create: Create, fake: FakeSandboxApi) -> None:
    create(as_of="2025-03-01")

    assert fake.log[0].headers["accept-datetime"] == "2025-03-01T23:59:59Z"


def test_a_command_runs_as_its_calls_set_it_up(create: Create) -> None:
    box = create()
    pwd = box.command(["sh", "-c", 'pwd; echo "$X"; cat']).with_cwd("/home/epochal")

    out = pwd.with_env("X", "1").with_stdin(b"piped in").with_timeout(60).output()
    assert out.text.endswith("/home/epochal\n1\npiped in")
    assert pwd.output().text.endswith("/home/epochal\n\n"), "the first command kept as it was"

    with pytest.raises(CommandError, match="nope"):
        box.command(["sh", "-c", "echo nope >&2; exit 1"]).output(check=True)
    with pytest.raises(InvalidArgumentError, match="argv"):
        box.command("ls -l").output()

    execution = box.command(["cat"]).with_open_stdin().start()
    for part in (b"one ", b"two ", b"three"):
        execution.write(part)
    execution.close_stdin()
    assert execution.output().stdout == b"one two three"


def test_a_box_with_a_client_of_its_own_ends_once_however_often_it_is_terminated(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create(own_client=True, app="relay", relay=True)

    assert json.loads(fake.log[0].body)["apps"] == ["relay"], "the relay asked for twice is one"
    box.terminate()
    box.terminate()
    assert [s.method for s in fake.log].count("DELETE") == 1


def test_lost_replies_are_retried_without_a_second_box_or_a_second_run(
    create: Create, fake: FakeSandboxApi
) -> None:
    fake.lose_replies(1)
    box = create()
    fake.lose_replies(1)
    out = box.command(["sh", "-c", 'echo ran >> "$1"', "sh", "/home/epochal/log"]).output()

    assert out.success
    assert list(fake.boxes) == ["sbx_1"]
    assert (fake.root("sbx_1") / "home/epochal/log").read_text() == "ran\n"


def test_a_box_that_never_boots_is_an_error_naming_why_and_is_terminated(
    create: Create, fake: FakeSandboxApi
) -> None:
    with pytest.raises(SandboxNotLiveError, match="The host went away"):
        create(as_of=NO_BOOT)

    assert fake.log[-1].method == "DELETE"
    assert fake.boxes["sbx_1"].info["state"] == "ended"


def test_a_command_streams_both_outputs_in_order_and_reports_its_exit_code(
    create: Create,
) -> None:
    box = create()
    script = "printf 'hello from stdout, a line longer than a frame'; printf oops >&2; exit 3"
    execution = box.command(["sh", "-c", script]).start()

    chunks = list(execution)
    out = b"".join(c.data for c in chunks if c.stream is Stream.STDOUT)
    err = b"".join(c.data for c in chunks if c.stream is Stream.STDERR)
    assert out == b"hello from stdout, a line longer than a frame"
    assert err == b"oops"
    assert len(chunks) > len(out) // FRAME, "output arrives in pieces"
    offsets = [c.offset for c in chunks if c.stream is Stream.STDOUT]
    assert offsets == sorted(offsets)
    assert offsets[0] == 0
    assert execution.exit.reason is ExitReason.EXITED
    assert execution.exit.code == 3


def test_a_write_larger_than_one_input_goes_in_pieces(create: Create, fake: FakeSandboxApi) -> None:
    box = create()
    execution = box.command(["wc", "-c"]).with_open_stdin().start()

    execution.write(b"x" * (2 * 1024 * 1024 + 5))
    execution.close_stdin()

    assert execution.output().text.strip() == str(2 * 1024 * 1024 + 5)
    inputs = [s for s in fake.log if s.path.endswith("/input")]
    assert len(inputs) == 4, "three pieces of at most 1 MiB, then the close"


def test_a_write_that_fails_partway_sends_the_rest_before_the_next_input(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create(max_retries=0)
    execution = box.command(["wc", "-c"]).with_open_stdin().start()
    size = 3 * 1024 * 1024

    fake.lose_replies(1)
    with pytest.raises(APIError):
        execution.write(b"x" * size)
    execution.close_stdin()

    assert execution.output().text.strip() == str(size), "every byte, once, then the close"


def test_input_whose_reply_was_lost_is_applied_once_before_the_next(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create(max_retries=0)
    execution = box.command(["cat"]).with_open_stdin().start()

    fake.lose_replies(1)
    with pytest.raises(APIError) as caught:
        execution.write(b"one ")
    assert caught.value.retryable
    execution.write(b"two")
    execution.close_stdin()

    assert execution.output().stdout == b"one two"


def test_input_whose_reply_was_unreadable_is_applied_once_before_the_next(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create()
    execution = box.command(["cat"]).with_open_stdin().start()

    fake.garble_replies(1)
    with pytest.raises(DecodeError):
        execution.write(b"one ")
    execution.write(b"two")
    execution.close_stdin()

    assert execution.output().stdout == b"one two"


def test_a_signal_ends_a_command_and_kill_waits_for_it(create: Create) -> None:
    box = create()
    execution = box.command(["sleep", "30"]).start()
    execution.signal(signal.SIGTERM)
    assert execution.output().exit.signal == signal.SIGTERM

    other = box.command(["sleep", "30"]).start()
    other.kill()
    assert other.exit.reason is ExitReason.SIGNALED
    other.kill()


def test_a_command_on_an_ended_box_is_the_apis_conflict(create: Create) -> None:
    box = create()
    box.terminate()

    with pytest.raises(APIError) as caught:
        box.command(["true"]).output()

    assert (caught.value.status, caught.value.kind) == (409, "conflict")


def payload(size: int) -> bytes:
    return bytes((i * 7 + i // 251) % 256 for i in range(size))


def test_a_file_goes_up_and_comes_back_byte_for_byte(
    create: Create, fake: FakeSandboxApi, tmp_path: Path
) -> None:
    box = create()
    data = payload(3 * RANGE_CAP + 17)

    box.files.write("/home/epochal/new/dir/data.bin", data)
    assert (fake.root("sbx_1") / "home/epochal/new/dir/data.bin").read_bytes() == data
    assert box.files.read("/home/epochal/new/dir/data.bin") == data
    stat = box.files.stat("/home/epochal/new/dir/data.bin")
    assert (stat.type, stat.size) == (FileType.FILE, len(data))

    local = tmp_path / "up.bin"
    local.write_bytes(data[::-1])
    box.files.upload_file(local, "/home/epochal/up.bin")
    box.files.download_file("/home/epochal/up.bin", tmp_path / "down.bin")
    assert (tmp_path / "down.bin").read_bytes() == data[::-1]

    box.files.write("/home/epochal/empty", b"")
    assert box.files.read("/home/epochal/empty") == b""

    fake.unknown_sizes = True
    assert box.files.read("/home/epochal/new/dir/data.bin") == data, (
        "read on to the end, size unknown"
    )

    box.files.remove("/home/epochal/new", recursive=True)
    assert not (fake.root("sbx_1") / "home/epochal/new").exists()


def test_a_large_file_overlaps_its_chunks_and_lands_in_order(
    create: Create, fake: FakeSandboxApi, monkeypatch: pytest.MonkeyPatch
) -> None:
    box = create()
    monkeypatch.setattr(c, "WRITE_CHUNK", RANGE_CAP)
    data = payload(13 * RANGE_CAP + 5)
    fake.file_delay = 0.2

    started = time.monotonic()
    box.files.write("/home/epochal/big.bin", data)
    assert box.files.read("/home/epochal/big.bin") == data
    took = time.monotonic() - started

    assert fake.peak_file_calls == c.IN_FLIGHT
    serial = 2 * 14 * fake.file_delay
    assert took < serial / 2, f"{took:.2f}s, serially {serial:.1f}s"
    assert (fake.root("sbx_1") / "home/epochal/big.bin").read_bytes() == data


def test_a_slow_upload_chunk_does_not_hold_back_the_chunks_after_it(
    create: Create, fake: FakeSandboxApi, monkeypatch: pytest.MonkeyPatch
) -> None:
    box = create()
    monkeypatch.setattr(c, "WRITE_CHUNK", RANGE_CAP)
    data = payload(6 * RANGE_CAP)
    fake.hold_write = (RANGE_CAP, 5 * RANGE_CAP)

    box.files.write("/home/epochal/big.bin", data)

    assert fake.passed_held_write, "the last chunk started while the second was held"
    assert (fake.root("sbx_1") / "home/epochal/big.bin").read_bytes() == data


def test_short_ranges_that_name_a_larger_file_are_read_to_its_end(
    create: Create, fake: FakeSandboxApi
) -> None:
    box = create()
    data = payload(5 * RANGE_CAP + 3)
    (fake.root("sbx_1") / "home/epochal/f").write_bytes(data)
    fake.short_ranges = 1000

    assert box.files.read("/home/epochal/f") == data


def test_a_failed_download_leaves_no_partial_file_and_never_an_unopened_one(
    create: Create, fake: FakeSandboxApi, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    box = create(max_retries=0)
    (fake.root("sbx_1") / "home/epochal/a").write_bytes(b"new")
    kept = tmp_path / "kept"
    kept.write_bytes(b"mine")
    kept.chmod(0o444)

    with pytest.raises(PermissionError):
        box.files.download_file("/home/epochal/a", kept)
    assert kept.read_bytes() == b"mine"

    monkeypatch.setattr(fake, "_read_file", lambda *_: (500, {}, b""))
    with pytest.raises(APIError):
        box.files.download_file("/home/epochal/a", tmp_path / "partial")
    assert not (tmp_path / "partial").exists()


def test_a_missing_path_is_not_found_and_a_relative_one_is_refused(create: Create) -> None:
    box = create()

    with pytest.raises(APIError) as caught:
        box.files.read("/home/epochal/missing")
    assert caught.value.status == 404

    with pytest.raises(InvalidArgumentError):
        box.files.read("work/relative")


def test_a_directory_goes_up_and_comes_back_whole(
    create: Create, fake: FakeSandboxApi, tmp_path: Path
) -> None:
    box = create()
    src = tmp_path / "src"
    (src / "pkg/deep").mkdir(parents=True)
    (src / "pkg/deep/data.bin").write_bytes(payload(RANGE_CAP * 2))
    (src / "top.txt").write_text("top")
    (src / "run.sh").write_text("#!/bin/sh\n")
    (src / "run.sh").chmod(0o755)
    (src / "link").symlink_to("top.txt")
    os.utime(src / "top.txt", (1_000_000_000, 1_000_000_000))

    box.files.upload_dir(src, "/home/epochal/proj")
    root = fake.root("sbx_1")
    assert (root / "home/epochal/proj/pkg/deep/data.bin").read_bytes() == payload(RANGE_CAP * 2)
    assert os.readlink(root / "home/epochal/proj/link") == "top.txt"
    assert (root / "home/epochal/proj/top.txt").stat().st_mtime > 1_000_000_000, (
        "the host's mtime stays"
    )
    assert list((root / "tmp").iterdir()) == [], "the upload's tar is removed"

    (root / "home/epochal/proj/escape").symlink_to("/etc/passwd")
    box.files.download_dir("/home/epochal/proj", tmp_path / "back")
    back = tmp_path / "back"
    assert (back / "pkg/deep/data.bin").read_bytes() == payload(RANGE_CAP * 2)
    assert (back / "top.txt").read_text() == "top"
    assert os.access(back / "run.sh", os.X_OK)
    assert os.readlink(back / "link") == "top.txt"
    assert not (back / "escape").is_symlink(), "a link out of the target is skipped"
    assert list((root / "tmp").iterdir()) == [], "the download's tar is removed"


def test_downloading_a_directory_into_itself_leaves_its_tar_out(
    create: Create, fake: FakeSandboxApi, tmp_path: Path
) -> None:
    box = create()
    (fake.root("sbx_1") / "tmp/a.txt").write_text("a")

    box.files.download_dir("/tmp", tmp_path / "out")

    assert sorted(p.name for p in (tmp_path / "out").iterdir()) == ["a.txt"]


def test_downloading_a_missing_directory_says_what_failed(create: Create, tmp_path: Path) -> None:
    box = create()

    with pytest.raises(CommandError, match="home/epochal/missing"):
        box.files.download_dir("/home/epochal/missing", tmp_path / "out")


@pytest.fixture
def echo_server() -> Iterator[tuple[str, int]]:
    with serving(0) as address:
        yield address


@contextlib.contextmanager
def serving(port: int) -> Iterator[tuple[str, int]]:
    """A local TCP server that answers each connection with ``echo:`` and what it read."""
    server = socket.create_server(("127.0.0.1", port))

    def answer(conn: socket.socket) -> None:
        with conn:
            data = b""
            while chunk := conn.recv(65536):
                data += chunk
            conn.sendall(b"echo:" + data)

    def accept() -> None:
        while True:
            try:
                conn, _ = server.accept()
            except OSError:
                return
            threading.Thread(target=answer, args=(conn,), daemon=True).start()

    threading.Thread(target=accept, daemon=True).start()
    yield server.getsockname()
    server.close()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port: int = s.getsockname()[1]
        return port


def through(port: int, data: bytes) -> bytes:
    """Sends ``data`` to the box's port as a box process would, half-closes, reads the answer."""
    with socket.create_connection(("127.0.0.1", port), timeout=10) as conn:
        conn.sendall(data)
        conn.shutdown(socket.SHUT_WR)
        answer = b""
        while chunk := conn.recv(65536):
            answer += chunk
        return answer


def test_forward_joins_the_boxs_connections_to_a_local_server_until_closed(
    create: Create, echo_server: tuple[str, int]
) -> None:
    box = create(relay=True)
    port = free_port()
    tunnel = box.forward(port, to=echo_server)
    try:
        assert tunnel.running
        assert through(port, b"hello") == b"echo:hello"
        big = payload(600 << 10)
        assert through(port, big) == b"echo:" + big, "more than a flow-control window"

        answers: list[bytes] = []
        threads = [
            threading.Thread(target=lambda i=i: answers.append(through(port, b"%d" % i)))
            for i in range(8)
        ]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        assert sorted(answers) == sorted(b"echo:%d" % i for i in range(8))
    finally:
        tunnel.close()

    assert not tunnel.running
    with pytest.raises(ConnectionRefusedError):
        through(port, b"after")
    tunnel.close()


def test_connections_the_local_side_refuses_leave_the_tunnel_room_for_the_rest(
    create: Create, echo_server: tuple[str, int], monkeypatch: pytest.MonkeyPatch
) -> None:
    """What a refused connection sent before it was refused must go back to the window all
    connections share, or a few refusals starve every connection after."""
    monkeypatch.setattr(_tunnel, "_STREAM_WINDOW", 65_535)
    monkeypatch.setattr(_tunnel, "_CONNECTION_WINDOW", 2 * 65_535)
    connect = asyncio.open_connection
    refusing = True

    async def slow_to_refuse(host: str, port: int) -> Any:
        if refusing:
            await asyncio.sleep(0.2)
            raise ConnectionRefusedError
        return await connect(host, port)

    monkeypatch.setattr(asyncio, "open_connection", slow_to_refuse)
    box = create(relay=True)
    port = free_port()
    tunnel = box.forward(port, to=echo_server)
    try:
        for _ in range(4):
            assert through(port, payload(60_000)) == b""
        refusing = False
        assert through(port, payload(60_000)) == b"echo:" + payload(60_000)
    finally:
        tunnel.close()


def test_a_tunnel_cut_off_by_its_box_ending_says_so_when_closed(
    create: Create, echo_server: tuple[str, int]
) -> None:
    box = create(relay=True)
    tunnel = box.forward(free_port(), to=echo_server)

    box.terminate()
    deadline = time.monotonic() + 10
    while tunnel.running and time.monotonic() < deadline:
        time.sleep(0.01)
    # Whichever the tunnel sees first: the API refusing the ended box, or the relay killed with it.
    with pytest.raises(TunnelError, match=r"not live|the relay ended .*signal=9"):
        tunnel.close()
    assert not tunnel.running


def test_forward_needs_the_relay_app(create: Create, echo_server: tuple[str, int]) -> None:
    box = create()

    with pytest.raises(InvalidArgumentError, match="with_relay"):
        box.forward(free_port(), to=echo_server)


def test_the_forward_example_serves_the_box_from_here(
    fake: FakeSandboxApi, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("EPOCHAL_API_KEY", KEY)
    monkeypatch.setenv("EPOCHAL_BASE_URL", fake.url)
    example = runpy.run_path(str(Path(__file__).parents[1] / "examples/forward.py"))

    said = example["main"](free_port())

    assert said == "{'ticker': 'NVDA', 'close': 124.92}\n"
    assert all(box.info["state"] == "ended" for box in fake.boxes.values())


@pytest.mark.parametrize(
    ("base", "path", "url"),
    [
        ("https://api.anthropic.com", "/v1/messages", "https://api.anthropic.com/v1/messages"),
        ("https://gw.example/llm/v1", "/v1/messages", "https://gw.example/llm/v1/messages"),
        (
            "https://gw.example/anthropic",
            "/v1/messages",
            "https://gw.example/anthropic/v1/messages",
        ),
        ("https://api.openai.com", "/v1/responses", "https://api.openai.com/v1/responses"),
        ("https://openrouter.ai/api/v1", "/v1/responses", "https://openrouter.ai/api/v1/responses"),
        ("https://gw.example/openai", "/v1/responses", "https://gw.example/openai/responses"),
    ],
)
def test_the_agent_example_addresses_a_base_as_the_cli_does(base: str, path: str, url: str) -> None:
    example = runpy.run_path(str(Path(__file__).parents[1] / "examples/agent.py"))

    assert example["target"](base, path) == url


@pytest.mark.parametrize("base", ["api.anthropic.com", "ftp://h.example", "https://u:p@h.example"])
def test_the_agent_example_refuses_a_bad_base_before_making_a_box(
    fake: FakeSandboxApi, monkeypatch: pytest.MonkeyPatch, base: str
) -> None:
    monkeypatch.setenv("EPOCHAL_API_KEY", KEY)
    monkeypatch.setenv("EPOCHAL_BASE_URL", fake.url)
    monkeypatch.setenv("EPOCHAL_LLM_BASE_URL", base)
    monkeypatch.setenv("EPOCHAL_LLM_API_KEY", "k")
    example = runpy.run_path(str(Path(__file__).parents[1] / "examples/agent.py"))

    with pytest.raises(SystemExit, match="http or https URL"):
        example["run"]("claude", "claude-sonnet-5-5", "2025-03-01", "x")

    assert fake.log == []


def test_the_agent_examples_proxy_forwards_a_chunked_request_whole(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    seen: list[tuple[str, bytes]] = []

    class Model(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = self.rfile.read(int(self.headers["content-length"]))
            seen.append((self.headers["authorization"], body))
            self.send_response(200)
            self.send_header("content-length", "2")
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *_: Any) -> None:
            pass

    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model) as model:
        threading.Thread(target=model.serve_forever, daemon=True).start()
        monkeypatch.setenv("EPOCHAL_LLM_BASE_URL", f"http://127.0.0.1:{model.server_port}/v1")
        monkeypatch.setenv("EPOCHAL_LLM_API_KEY", "xai-real")
        example = runpy.run_path(str(Path(__file__).parents[1] / "examples/agent.py"))
        as_of = dt.datetime(2025, 3, 1, 23, 59, 59, tzinfo=dt.UTC)
        with example["serve"]("grok", as_of, 8787) as proxy:
            threading.Thread(target=proxy.serve_forever, daemon=True).start()
            with socket.create_connection(("127.0.0.1", proxy.server_port)) as conn:
                head = b"POST /v1/responses HTTP/1.1\r\nhost: 127.0.0.1:8787\r\n"
                conn.sendall(head + b"transfer-encoding: chunked\r\nconnection: close\r\n\r\n")
                conn.sendall(b'6\r\n{"a":1\r\n1\r\n}\r\n0\r\n\r\n')
                assert conn.makefile("rb").readline().startswith(b"HTTP/1.1 200")
            proxy.shutdown()
        model.shutdown()

    assert seen == [("Bearer xai-real", b'{"a":1}')]


def test_the_agent_example_serves_the_harness_its_model_and_the_as_of_web(
    fake: FakeSandboxApi,
    monkeypatch: pytest.MonkeyPatch,
    capfd: pytest.CaptureFixture[str],
) -> None:
    seen: list[dict[str, Any]] = []

    class Model(http.server.BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            body = self.rfile.read(int(self.headers["content-length"]))
            seen.append({"path": self.path, "key": self.headers["x-api-key"], "body": body})
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("x-request-id", "req_upstream")
            self.end_headers()
            # The epoch split across two writes, as a stream may send it.
            for part in (b'{"id":"msg_1","created_at":1767', b'225600,"content":"hi"}'):
                self.wfile.write(part)
                self.wfile.flush()
                time.sleep(0.05)

        def log_message(self, *_: Any) -> None:
            pass

    fake_bin = Path(__file__).parent / "fake_bin"
    monkeypatch.setenv("PATH", f"{fake_bin}{os.pathsep}{os.environ['PATH']}")
    # The stand-in for epochal-mcp, unless EPOCHAL_MCP names the real one.
    real = os.environ.get("EPOCHAL_MCP")
    monkeypatch.setenv("EPOCHAL_MCP", real or str(fake_bin / "epochal-mcp"))
    monkeypatch.setenv("EPOCHAL_API_KEY", KEY)
    monkeypatch.setenv("EPOCHAL_BASE_URL", fake.url)
    with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Model) as model:
        threading.Thread(target=model.serve_forever, daemon=True).start()
        monkeypatch.setenv("EPOCHAL_LLM_BASE_URL", f"http://127.0.0.1:{model.server_port}/v1")
        monkeypatch.setenv("EPOCHAL_LLM_API_KEY", "sk-ant-real")
        monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-ant-real")  # Often set too; never passed on.
        example = runpy.run_path(str(Path(__file__).parents[1] / "examples/agent.py"))
        code = example["run"](
            "claude",
            "claude-sonnet-5-5",
            "2025-03-01",
            "nvidia",
            port=free_port(),
            mcp_port=free_port(),
        )
        model.shutdown()

    assert code == 0
    events = [json.loads(line) for line in capfd.readouterr().out.splitlines()]
    refused, server, tools, search, fetch, turn = events
    if server["server"].get("name") == "stand-in":
        assert server["server"]["leaked"] == [], "epochal-mcp gets no model endpoint variable"
    assert tools == {"tools": ["web_fetch", "web_search"]}
    assert refused == {"refused": 404}, "only model calls go out on the key"
    assert search["search"].startswith("1. About nvidia\n   https://example.com/a\n   …")
    assert fetch["fetch"].endswith("read at 2025-03-01T23:59:59Z"), "a bare day held to its end"
    assert turn["turn"] == {"id": "msg_1", "created_at": 1740873599, "content": "hi"}, (
        "the model's epoch, split across two reads, is the as-of's"
    )
    assert turn["headers"] == ["content-type", "transfer-encoding"], "no Date, no upstream ids"
    [call] = seen
    assert (call["path"], call["key"]) == ("/v1/messages", "sk-ant-real"), "the key added here"
    assert json.loads(call["body"]) == {"model": "claude-sonnet-5-5"}
    [created] = (json.loads(s.body) for s in fake.log if s.path.endswith("/sandbox"))
    assert created["apps"] == ["claude-code", "relay"]
    assert "sk-ant-real" not in json.dumps([s.body.decode(errors="replace") for s in fake.log])
    assert all(box.info["state"] == "ended" for box in fake.boxes.values())
