"""Flow-control accounting inside the tunnel, where a local failure must not cost the window
every connection shares."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest
from epochal.sandbox import _tunnel


class _Credit:
    """Stands in for the h2 connection, counting the window handed back per stream."""

    def __init__(self) -> None:
        self.acked: dict[int, int] = {}

    def acknowledge_received_data(self, size: int, sid: int) -> None:
        self.acked[sid] = self.acked.get(sid, 0) + size


class _BrokenWriter:
    def write(self, data: bytes) -> None:
        pass

    async def drain(self) -> None:
        raise ConnectionResetError


def test_a_frame_taken_for_a_local_write_that_fails_gives_its_window_back() -> None:
    server: Any = _tunnel._Server(relay=None, to=("127.0.0.1", 1))  # type: ignore[arg-type]
    server._conn = _Credit()
    stream = server._streams[1] = _tunnel._Stream()
    stream.inbox.put_nowait((b"x" * 100, 100))
    stream.inbox.put_nowait((b"y" * 50, 50))

    with pytest.raises(ConnectionResetError):
        asyncio.run(server._to_local(1, stream, _BrokenWriter()))
    server._drop(1)

    assert server._conn.acked == {1: 150}, "the failed frame's and the queued frame's window"
