"""Reverse tunnels: a port in the box whose connections are served from here. The box's ``relay``
app listens on the port and is the HTTP/2 client of one connection carried over its own stdin and
stdout; this end is the server. Each connection to the port is one ``POST /`` stream: its request
body carries what the box sent and its response body what goes back, and either side's end of
stream is that direction's half-close."""

from __future__ import annotations

import asyncio
import contextlib
import logging
import threading
from dataclasses import dataclass, field
from types import TracebackType
from typing import TYPE_CHECKING, Union

import h2.config
import h2.connection
import h2.events
import h2.exceptions
import h2.settings
from epochal.api.time_portal import ExecExit, Stream
from epochal.sandbox._common import PIPE, Chunk, TunnelError

if TYPE_CHECKING:
    from epochal.sandbox._async import AsyncExec
    from epochal.sandbox._sync import Exec

_log = logging.getLogger("epochal.sandbox")

_READ = 64 << 10
_MAX_CONNECTIONS = 128
_STREAM_WINDOW = 256 << 10
_CONNECTION_WINDOW = _MAX_CONNECTIONS * _STREAM_WINDOW
_MAX_HEADERS = 16 << 10
_PING_EVERY = 10.0
"""The relay exits after 45 s without a byte from here; pings keep an idle tunnel alive."""
_DEFAULT_WINDOW = 65_535


Relay = Union["AsyncExec", "_ThreadedRelay"]
"""The relay command, as the tunnel drives it."""


class AsyncTunnel:
    """A served port, from :meth:`AsyncSandbox.forward`. :meth:`close` stops it and waits until
    the relay is gone, so the port is free."""

    def __init__(self, relay: Relay, task: asyncio.Task[None]) -> None:
        self._relay = relay
        self._task = task

    @property
    def running(self) -> bool:
        """Whether the tunnel still serves; it stops if the relay ends or the API fails."""
        return not self._task.done()

    async def close(self) -> None:
        """Stops the tunnel; raises what stopped it, if it failed before. Closing twice is fine."""
        self._task.cancel()
        await asyncio.wait([self._task])
        await self._relay.kill()
        if not self._task.cancelled() and (error := self._task.exception()):
            raise TunnelError(f"the tunnel failed: {error}") from error

    async def __aenter__(self) -> AsyncTunnel:
        return self

    async def __aexit__(
        self, t: type[BaseException] | None, e: BaseException | None, tb: TracebackType | None
    ) -> None:
        await self.close()


async def serve(relay: Relay, to: tuple[str, int]) -> AsyncTunnel:
    """Serves the relay's connections by joining each to a new TCP connection to ``to``; returns
    once the relay listens."""
    try:
        first = await _listening(relay)
    except BaseException:
        with contextlib.suppress(Exception):
            await asyncio.shield(relay.kill())
        raise
    return AsyncTunnel(relay, asyncio.create_task(_Server(relay, to).run(first)))


async def _listening(relay: Relay) -> Chunk:
    """The relay's first stdout bytes, which it writes once it listens."""
    stderr = bytearray()
    while (chunk := await relay.next()) is not None:
        if chunk.stream is not Stream.STDOUT:
            stderr += chunk.data
        elif chunk.offset == 0:
            return chunk
        else:
            raise TunnelError("the relay's first output was lost")
    said = stderr.decode(errors="replace").strip()
    raise TunnelError(f"the relay ended before it listened ({relay.exit}): {said}")


@dataclass(eq=False)
class _Stream:
    """One tunnelled connection: what the box sent and the flow-controlled size of each piece,
    ``None`` at its end."""

    inbox: asyncio.Queue[tuple[bytes, int] | None] = field(default_factory=asyncio.Queue)
    task: asyncio.Task[None] | None = None


class _Server:
    def __init__(self, relay: Relay, to: tuple[str, int]) -> None:
        self._relay = relay
        self._to = to
        config = h2.config.H2Configuration(client_side=False, header_encoding="utf-8")
        self._conn = h2.connection.H2Connection(config)
        self._conn.local_settings = h2.settings.Settings(
            client=False,
            initial_values={
                h2.settings.SettingCodes.INITIAL_WINDOW_SIZE: _STREAM_WINDOW,
                h2.settings.SettingCodes.MAX_CONCURRENT_STREAMS: _MAX_CONNECTIONS,
                h2.settings.SettingCodes.MAX_HEADER_LIST_SIZE: _MAX_HEADERS,
            },
        )
        self._streams: dict[int, _Stream] = {}
        self._dirty = asyncio.Event()
        """Set when the connection may have bytes for the relay."""
        self._window = asyncio.Event()
        """Set when the relay may have opened a flow-control window."""

    async def run(self, first: Chunk) -> None:
        self._conn.initiate_connection()
        self._conn.increment_flow_control_window(_CONNECTION_WINDOW - _DEFAULT_WINDOW)
        loops = [
            asyncio.create_task(self._read(first)),
            asyncio.create_task(self._write()),
            asyncio.create_task(self._ping()),
        ]
        try:
            done, _ = await asyncio.wait(loops, return_when=asyncio.FIRST_COMPLETED)
            for task in done:
                task.result()
            raise TunnelError(f"the relay ended ({self._relay.exit})")
        finally:
            tasks = loops + [s.task for s in self._streams.values() if s.task]
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)

    async def _read(self, chunk: Chunk | None) -> None:
        """Feeds the relay's stdout to the connection until it ends or has a gap, which HTTP/2
        can't resume past."""
        at = 0
        while chunk is not None:
            if chunk.stream is Stream.STDOUT:
                if chunk.offset != at:
                    raise TunnelError("the relay's output has a gap")
                at += len(chunk.data)
                for event in self._conn.receive_data(chunk.data):
                    self._on(event)
                self._dirty.set()
            chunk = await self._relay.next()

    async def _write(self) -> None:
        """The one writer of the relay's stdin, so frames reach it in order."""
        while True:
            await self._dirty.wait()
            self._dirty.clear()
            while data := self._conn.data_to_send(PIPE):
                await self._relay.write(data)

    async def _ping(self) -> None:
        while True:
            await asyncio.sleep(_PING_EVERY)
            self._conn.ping(b"\0" * 8)
            self._dirty.set()

    def _on(self, event: h2.events.Event) -> None:
        if isinstance(event, h2.events.RequestReceived):
            self._open(event)
        elif isinstance(event, h2.events.DataReceived):
            stream = self._streams.get(event.stream_id)
            if stream is None:
                self._ack(event.stream_id, event.flow_controlled_length)
            else:
                stream.inbox.put_nowait((event.data, event.flow_controlled_length))
        elif isinstance(event, h2.events.StreamEnded):
            if stream := self._streams.get(event.stream_id):
                stream.inbox.put_nowait(None)
        elif isinstance(event, h2.events.StreamReset):
            if (stream := self._drop(event.stream_id)) and stream.task:
                stream.task.cancel()
        elif isinstance(event, h2.events.WindowUpdated | h2.events.RemoteSettingsChanged):
            self._window.set()
        elif isinstance(event, h2.events.ConnectionTerminated):
            raise TunnelError(f"the relay closed the connection: {event.error_code!r}")

    def _open(self, event: h2.events.RequestReceived) -> None:
        sid = event.stream_id
        headers = {str(k): str(v) for k, v in event.headers or []}
        if headers.get(":method") != "POST" or headers.get(":path") != "/":
            self._respond(sid, "404", end_stream=True)
            return
        stream = self._streams[sid] = _Stream()
        stream.task = asyncio.create_task(self._splice(sid, stream))

    async def _splice(self, sid: int, stream: _Stream) -> None:
        """Joins stream ``sid`` to a new connection to ``to`` until both directions end."""
        try:
            reader, writer = await asyncio.open_connection(*self._to)
        except OSError as e:
            _log.debug("tunnel: connecting to %s:%d failed: %r", *self._to, e)
            self._drop(sid)
            self._respond(sid, "502", end_stream=True)
            return
        try:
            self._respond(sid, "200", end_stream=False)
            async with asyncio.TaskGroup() as group:
                group.create_task(self._to_local(sid, stream, writer))
                group.create_task(self._to_box(sid, reader))
        except* (OSError, h2.exceptions.ProtocolError) as e:
            _log.debug("tunnel: stream %d ended: %r", sid, e.exceptions[0])
            with contextlib.suppress(h2.exceptions.ProtocolError):
                self._conn.reset_stream(sid)
                self._dirty.set()
        finally:
            self._drop(sid)
            writer.close()

    async def _to_local(self, sid: int, stream: _Stream, writer: asyncio.StreamWriter) -> None:
        while (item := await stream.inbox.get()) is not None:
            data, size = item
            try:
                writer.write(data)
                await writer.drain()
            finally:
                self._ack(sid, size)
        if writer.can_write_eof():
            writer.write_eof()

    async def _to_box(self, sid: int, reader: asyncio.StreamReader) -> None:
        while data := await reader.read(_READ):
            view = memoryview(data)
            while view:
                n = await self._capacity(sid, len(view))
                self._conn.send_data(sid, bytes(view[:n]))
                self._dirty.set()
                view = view[n:]
        self._conn.end_stream(sid)
        self._dirty.set()

    async def _capacity(self, sid: int, wanted: int) -> int:
        """How many bytes stream ``sid`` may send now, waiting for the relay to open its window."""
        while True:
            window = self._conn.local_flow_control_window(sid)
            n = min(window, self._conn.max_outbound_frame_size, wanted)
            if n > 0:
                return n
            self._window.clear()
            await self._window.wait()

    def _drop(self, sid: int) -> _Stream | None:
        """Forgets stream ``sid``, handing back the window of what it received but never sent on,
        which would otherwise be lost to the connection for good."""
        stream = self._streams.pop(sid, None)
        while stream and not stream.inbox.empty():
            if item := stream.inbox.get_nowait():
                self._ack(sid, item[1])
        return stream

    def _ack(self, sid: int, size: int) -> None:
        """Hands ``size`` bytes of window back once they have left for the local connection."""
        self._conn.acknowledge_received_data(size, sid)
        self._dirty.set()

    def _respond(self, sid: int, status: str, *, end_stream: bool) -> None:
        with contextlib.suppress(h2.exceptions.ProtocolError):
            self._conn.send_headers(sid, [(":status", status)], end_stream=end_stream)
            self._dirty.set()


class _ThreadedRelay:
    """A blocking relay command, driven from the tunnel's event loop through worker threads."""

    def __init__(self, relay: Exec) -> None:
        self._relay = relay

    @property
    def exit(self) -> ExecExit | None:
        return self._relay.exit

    async def next(self) -> Chunk | None:
        return await asyncio.to_thread(self._relay.next)

    async def write(self, data: bytes) -> None:
        await asyncio.to_thread(self._relay.write, data)

    async def kill(self) -> None:
        await asyncio.to_thread(self._relay.kill)


class Tunnel:
    """A served port, from :meth:`Sandbox.forward`, served by a background thread. :meth:`close`
    stops it and waits until the relay is gone, so the port is free."""

    def __init__(
        self, loop: asyncio.AbstractEventLoop, thread: threading.Thread, inner: AsyncTunnel
    ) -> None:
        self._loop = loop
        self._thread = thread
        self._inner = inner
        self._closing = threading.Lock()

    @property
    def running(self) -> bool:
        """Whether the tunnel still serves; it stops if the relay ends or the API fails."""
        return self._inner.running

    def close(self) -> None:
        """Stops the tunnel; raises what stopped it, if it failed before. Closing twice is fine."""
        with self._closing:
            if not self._thread.is_alive():
                return
            try:
                asyncio.run_coroutine_threadsafe(self._inner.close(), self._loop).result()
            finally:
                self._loop.call_soon_threadsafe(self._loop.stop)
                self._thread.join()

    def __enter__(self) -> Tunnel:
        return self

    def __exit__(
        self, t: type[BaseException] | None, e: BaseException | None, tb: TracebackType | None
    ) -> None:
        self.close()


def start_tunnel(relay: Exec, to: tuple[str, int]) -> Tunnel:
    """Serves the blocking ``relay`` from an event loop on a daemon thread of its own."""
    loop = asyncio.new_event_loop()

    def run() -> None:
        asyncio.set_event_loop(loop)
        try:
            loop.run_forever()
        finally:
            loop.close()

    thread = threading.Thread(target=run, name="epochal-tunnel", daemon=True)
    thread.start()
    try:
        inner = asyncio.run_coroutine_threadsafe(serve(_ThreadedRelay(relay), to), loop).result()
    except BaseException:
        loop.call_soon_threadsafe(loop.stop)
        thread.join()
        raise
    return Tunnel(loop, thread, inner)
