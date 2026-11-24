from __future__ import annotations

import datetime as dt
from collections.abc import Callable

import httpx
import pytest
from epochal.api import APIError, InvalidArgumentError
from epochal.api.time_portal import ExitReason, FileType, Network, SandboxState, Stream
from fake_api import Api, Fake, body, error

AS_OF = "2026-03-01T00:00:00Z"
ID = "sbx_7f3k9q2m4x8p1c6w"
EXEC = "exec_m3p8x2k7"
BOX = f"/v1/time-portal/sandbox/{ID}"
SANDBOX = {
    "id": ID,
    "state": "live",
    "as_of": AS_OF,
    "network": "time-portal",
    "vcpus": 2,
    "memory_gib": 4,
    "timeout": "1h",
    "idle_timeout": "10m",
    "created_at": "2026-11-02T18:04:11Z",
    "booted_at": "2026-11-02T18:04:14Z",
    "expires_at": "2026-11-02T19:04:14Z",
    "ended_at": None,
    "error": None,
    "metadata": {"job": "nightly"},
    "apps": [{"name": "relay", "version": "1.2.0"}],
}
FAILED = {
    **SANDBOX,
    "id": "sbx_failed",
    "state": "failed",
    "error": {
        "type": "spend_limit_reached",
        "message": "The organization's spend limit is reached.",
    },
    "apps": [],
}
OUTPUT = {
    "frames": [{"stream": "stdout", "offset": 0, "data_b64": "MjAyNi0wMy0wMQo="}],
    "exit": {"reason": "exited", "code": 0},
}
STAT = {
    "path": "/home/epochal/out/result.csv",
    "type": "file",
    "size": 18432,
    "mode": 420,
    "mtime": "2026-03-01T00:02:13Z",
}
FILE = b"0123456789"
MakeApi = Callable[..., Api]


def read(request: httpx.Request) -> httpx.Response:
    """``FILE`` as a box serves it: the asked range clamped to its end, or 416 past it."""
    first, _, last = request.headers["range"].removeprefix("bytes=").partition("-")
    start, size = int(first), len(FILE)
    if start >= size:
        return httpx.Response(416, headers={"content-range": f"bytes */{size}"})
    end = min(int(last), size - 1) if last else size - 1
    headers = {"content-range": f"bytes {start}-{end}/{size}"}
    return httpx.Response(206, headers=headers, content=FILE[start : end + 1])


def route(request: httpx.Request) -> httpx.Response:
    method, path = request.method, request.url.path
    rest = path.removeprefix(BOX) if path.startswith(BOX) else None
    if path == "/v1/time-portal/sandbox":
        if method == "POST":
            return httpx.Response(202, json=SANDBOX)
        return httpx.Response(200, json={"sandboxes": [SANDBOX], "next_cursor": "c2"})
    if path == "/v1/time-portal/sandbox/sbx_failed":
        return httpx.Response(200, json=FAILED)
    answers = {
        ("GET", ""): lambda: httpx.Response(200, json=SANDBOX),
        ("DELETE", ""): lambda: httpx.Response(204),
        ("POST", "/execs"): lambda: httpx.Response(200, json={"id": EXEC}),
        ("GET", f"/execs/{EXEC}/output"): lambda: httpx.Response(200, json=OUTPUT),
        ("POST", f"/execs/{EXEC}/input"): lambda: (
            httpx.Response(200, json={"next_seq": 4})
            if body(request)["seq"] == 3
            else error(409, "conflict", "Input 'seq' is out of order.", next_seq=3)
        ),
        ("GET", "/files/stat"): lambda: httpx.Response(200, json=STAT),
        ("GET", "/files/list"): lambda: httpx.Response(
            200, json={"entries": [STAT], "next_cursor": None}
        ),
        ("GET", "/files"): lambda: (
            {
                "/empty": httpx.Response(200),
                "/whole": httpx.Response(200, content=b"all of it"),
            }.get(request.url.params["path"])
            or read(request)
        ),
        ("PUT", "/files"): lambda: httpx.Response(200, json=STAT),
        ("DELETE", "/files"): lambda: httpx.Response(204),
    }
    answer = answers.get((method, rest or "")) if rest is not None else None
    return answer() if answer else httpx.Response(404)


def test_create_sends_the_instant_the_fields_set_and_the_key(make_api: MakeApi) -> None:
    fake = Fake(route)
    box = make_api(fake).create_sandbox(
        network=Network.TIME_PORTAL,
        timeout=dt.timedelta(minutes=90),
        metadata={"job": "nightly"},
        apps=["relay"],
        idempotency_key="job-42-box",
    )

    seen = fake.last
    assert (seen.method, seen.url.path) == ("POST", "/v1/time-portal/sandbox")
    assert seen.headers["accept-datetime"] == AS_OF
    assert seen.headers["idempotency-key"] == "job-42-box"
    assert body(seen) == {
        "network": "time-portal",
        "timeout": "90m",
        "metadata": {"job": "nightly"},
        "apps": ["relay"],
    }
    assert box.id == ID
    assert box.state is SandboxState.LIVE
    assert box.network is Network.TIME_PORTAL
    assert box.timeout == dt.timedelta(hours=1)
    assert box.idle_timeout == dt.timedelta(minutes=10)
    assert box.booted_at == dt.datetime(2026, 11, 2, 18, 4, 14, tzinfo=dt.UTC)
    assert box.metadata == {"job": "nightly"}
    assert box.apps[0].name == "relay"
    assert box.error is None


def test_a_create_without_a_key_gets_a_fresh_one_kept_across_its_retries(
    make_api: MakeApi,
) -> None:
    fake = Fake(route, queue=[httpx.Response(503)])
    make_api(fake).create_sandbox()
    make_api(fake).create_sandbox()

    keys = [r.headers["idempotency-key"] for r in fake.seen]
    assert keys[0] == keys[1], "a retry resends the key"
    assert keys[1] != keys[2], "a new call makes a new key"
    assert len(keys[0]) == 32
    assert body(fake.last) == {}


@pytest.mark.parametrize("bad", [dt.timedelta(milliseconds=1500), dt.timedelta(0), -5, 1.5])
def test_a_duration_the_api_cannot_express_is_refused_before_any_request(
    make_api: MakeApi, bad: object
) -> None:
    fake = Fake(route)

    with pytest.raises(InvalidArgumentError):
        make_api(fake).create_sandbox(idle_timeout=bad)

    assert fake.seen == []


def test_a_box_reads_compound_durations(make_api: MakeApi) -> None:
    fake = Fake(route)
    fake.queue.append(
        httpx.Response(200, json={**SANDBOX, "timeout": "1h30m", "idle_timeout": "2m30s"})
    )

    box = make_api(fake).get_sandbox(ID)

    assert (box.timeout, box.idle_timeout) == (dt.timedelta(minutes=90), dt.timedelta(seconds=150))


def test_a_network_the_sdk_does_not_know_is_refused(make_api: MakeApi) -> None:
    with pytest.raises(InvalidArgumentError):
        make_api(Fake(route)).create_sandbox(network="internet")


def test_a_failed_box_says_why_by_kind(make_api: MakeApi) -> None:
    failed = make_api(Fake(route)).get_sandbox("sbx_failed")

    assert failed.state is SandboxState.FAILED
    assert failed.error is not None
    assert failed.error.kind == "spend_limit_reached"


def test_get_waits_by_duration_and_terminate_ends_the_box(make_api: MakeApi) -> None:
    fake = Fake(route)
    api = make_api(fake)

    api.get_sandbox(ID, wait=55)
    assert (fake.last.url.path, fake.last.url.query) == (BOX, b"wait=55s")

    api.terminate_sandbox(ID)
    assert (fake.last.method, fake.last.url.path) == ("DELETE", BOX)


def test_list_filters_pages_and_returns_the_next_cursor(make_api: MakeApi) -> None:
    fake = Fake(route)
    page = make_api(fake).list_sandboxes(state="live", cursor="c1", limit=20)

    assert fake.last.url.query == b"state=live&cursor=c1&limit=20"
    assert page.sandboxes[0].id == ID
    assert page.next_cursor == "c2"


def test_start_exec_sends_the_command_stdin_and_key(make_api: MakeApi) -> None:
    fake = Fake(route)
    exec_id = make_api(fake).start_exec(
        ID,
        ["python3", "-c", "print(1)"],
        cwd="/home/epochal",
        env={"TOKEN": "secret"},
        stdin=b"hi",
        idempotency_key="job-42-step-1",
    )

    assert exec_id == EXEC
    assert fake.last.url.path == f"{BOX}/execs"
    assert fake.last.headers["idempotency-key"] == "job-42-step-1"
    assert body(fake.last) == {
        "argv": ["python3", "-c", "print(1)"],
        "env": {"TOKEN": "secret"},
        "cwd": "/home/epochal",
        "stdin_b64": "aGk=",
    }


@pytest.mark.parametrize("argv", [[], [""], "ls -l"])
def test_an_argv_naming_no_program_is_refused_before_any_request(
    make_api: MakeApi, argv: list[str] | str
) -> None:
    fake = Fake(route)

    with pytest.raises(InvalidArgumentError):
        make_api(fake).start_exec(ID, argv)

    assert fake.seen == []


def test_output_is_read_from_the_offsets_asked(make_api: MakeApi) -> None:
    fake = Fake(route)
    output = make_api(fake).read_exec_output(
        ID, EXEC, stdout_offset=4, wait=dt.timedelta(seconds=30), max_bytes=65_536
    )

    assert fake.last.url.query == b"stdout_offset=4&stderr_offset=0&wait=30s&max_bytes=65536"
    [frame] = output.frames
    assert (frame.stream, frame.offset, frame.data) == (Stream.STDOUT, 0, b"2026-03-01\n")
    assert output.exit is not None
    assert output.exit.reason is ExitReason.EXITED
    assert output.exit.success


def test_input_encodes_each_action_and_returns_the_next_seq(make_api: MakeApi) -> None:
    fake = Fake(route)
    api = make_api(fake)

    assert api.send_exec_input(ID, EXEC, 3, data=b"hi") == 4
    assert body(fake.last) == {"seq": 3, "data_b64": "aGk="}
    api.send_exec_input(ID, EXEC, 3, eof=True)
    assert body(fake.last) == {"seq": 3, "eof": True}
    api.send_exec_input(ID, EXEC, 3, signal=15)
    assert body(fake.last) == {"seq": 3, "signal": 15}

    with pytest.raises(InvalidArgumentError):
        api.send_exec_input(ID, EXEC, 3, data=b"x", eof=True)


def test_an_out_of_order_input_names_the_seq_expected(make_api: MakeApi) -> None:
    with pytest.raises(APIError) as caught:
        make_api(Fake(route)).send_exec_input(ID, EXEC, 9, eof=True)

    assert (caught.value.status, caught.value.kind, caught.value.next_seq) == (409, "conflict", 3)


def test_stat_and_list_read_file_metadata(make_api: MakeApi) -> None:
    fake = Fake(route)
    api = make_api(fake)

    stat = api.stat_file(ID, "/home/epochal/out/result.csv")
    assert fake.last.url.params["path"] == "/home/epochal/out/result.csv"
    assert (stat.type, stat.size, stat.mode) == (FileType.FILE, 18432, 0o644)

    listing = api.list_files(ID, "/home/epochal", recursive=True, limit=10)
    assert fake.last.url.query == b"path=%2Fhome%2Fepochal&recursive=true&limit=10"
    assert listing.entries == [stat]
    assert listing.next_cursor is None


def test_read_file_says_where_the_bytes_start_and_the_size(make_api: MakeApi) -> None:
    fake = Fake(route)
    api = make_api(fake)

    got = api.read_file(ID, "/f", offset=2, length=3)
    assert fake.last.headers["range"] == "bytes=2-4"
    assert (got.start, got.data, got.total) == (2, b"234", 10)

    rest = api.read_file(ID, "/f", offset=8)
    assert fake.last.headers["range"] == "bytes=8-"
    assert (rest.start, rest.data) == (8, b"89")

    past = api.read_file(ID, "/f", offset=10)
    assert (past.start, past.data, past.total) == (10, b"", 10)

    empty = api.read_file(ID, "/empty")
    assert (empty.start, empty.data) == (0, b"")

    whole = api.read_file(ID, "/whole")
    assert (whole.start, whole.data, whole.total) == (0, b"all of it", 9), "a 200 is the file"

    with pytest.raises(InvalidArgumentError):
        api.read_file(ID, "/f", length=0)


def test_write_returns_the_file_after_and_remove_addresses_the_path(make_api: MakeApi) -> None:
    fake = Fake(route)
    api = make_api(fake)

    stat = api.write_file(ID, "/home/epochal/a b.txt", b"data", offset=4)
    assert fake.last.method == "PUT"
    assert fake.last.url.params["path"] == "/home/epochal/a b.txt"
    assert fake.last.url.params["offset"] == "4"
    assert fake.last.content == b"data"
    assert stat.path == "/home/epochal/out/result.csv"

    api.remove_file(ID, "/home/epochal", recursive=True)
    assert (fake.last.method, fake.last.url.query) == (
        "DELETE",
        b"path=%2Fhome%2Fepochal&recursive=true",
    )


def test_a_remove_whose_reply_was_lost_finds_the_path_gone_and_succeeds(
    make_api: MakeApi,
) -> None:
    fake = Fake(lambda _: error(404, "not_found", "No such file."), queue=[httpx.Response(503)])
    api = make_api(fake)

    api.remove_file(ID, "/home/epochal/a", recursive=False)
    assert len(fake.seen) == 2

    with pytest.raises(APIError) as caught:
        api.remove_file(ID, "/home/epochal/a", recursive=False)
    assert caught.value.status == 404, "a first attempt's 404 is the API's answer"


def test_an_id_cannot_change_which_endpoint_is_called(make_api: MakeApi) -> None:
    fake = Fake(route)

    with pytest.raises(APIError):
        make_api(fake).get_sandbox("x/../../search?q=1")

    assert fake.last.url.raw_path == b"/v1/time-portal/sandbox/x%2F..%2F..%2Fsearch%3Fq%3D1"


@pytest.mark.parametrize("bad", ["", ".", ".."])
def test_an_id_a_url_would_drop_is_refused_before_any_request(make_api: MakeApi, bad: str) -> None:
    fake = Fake(route)

    with pytest.raises(InvalidArgumentError):
        make_api(fake).terminate_sandbox(bad)

    assert fake.seen == []


def test_values_from_a_newer_server_read_as_unknown(make_api: MakeApi) -> None:
    newer = {**SANDBOX, "state": "hibernating", "network": "internet", "future_field": 1}
    box = make_api(Fake(lambda _: httpx.Response(200, json=newer))).get_sandbox(ID)

    assert box.state is SandboxState.UNKNOWN
    assert box.network is Network.UNKNOWN
