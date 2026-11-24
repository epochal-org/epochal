# Generated from _async.py by scripts/unasync.py; edit that file, then run the script.
from __future__ import annotations

import contextlib
import datetime as dt
import io
import os
import tempfile
import threading
from collections.abc import Iterator, Mapping, Sequence
from functools import partial
from types import TracebackType
from typing import IO, Any, Self

from epochal.api import (
    MAX_WAIT,
    APIError,
    Client,
    Duration,
    EpochalError,
    Instant,
    InvalidArgumentError,
)
from epochal.api.time_portal import (
    ExecExit,
    FileRange,
    FileStat,
    Network,
    SandboxInfo,
    SandboxState,
    Stream,
    TimePortal,
)
from epochal.sandbox import _common as c
from epochal.sandbox._common import Chunk, Output
from epochal.sandbox._tunnel import Tunnel, start_tunnel
from epochal.sandbox._window import Window


class Sandbox:
    """A live box, from :meth:`time_portal`. Use it as a context manager, or call
    :meth:`terminate`; a box left running ends at its timeout and is billed until then."""

    def __init__(self, portal: TimePortal, info: SandboxInfo, *, owns_client: bool) -> None:
        self._client = portal.client
        self._portal = portal
        self._owns_client = owns_client
        self._terminated = False
        self.info = info
        """The box as the API described it when it came up."""

    @staticmethod
    def time_portal(as_of: Instant) -> SandboxBuilder:
        """A time-portal box held to ``as_of``: set it up with ``with_`` calls, then ``build()``
        it."""
        return SandboxBuilder(as_of)

    @property
    def id(self) -> str:
        return self.info.id

    @property
    def as_of(self) -> dt.datetime:
        """The instant the box is held to."""
        return self.info.as_of

    @property
    def files(self) -> Files:
        """The box's filesystem."""
        return Files(self)

    def __repr__(self) -> str:
        return f"Sandbox(id={self.id!r}, as_of={self.as_of.isoformat()!r})"

    def __enter__(self) -> Sandbox:
        return self

    def __exit__(
        self, t: type[BaseException] | None, e: BaseException | None, tb: TracebackType | None
    ) -> None:
        self.terminate()

    def terminate(self) -> None:
        """Ends the box now, and closes the client if the box made it. Ending an ended box
        succeeds."""
        if not self._terminated:
            self._portal.terminate_sandbox(self.id)
            self._terminated = True
            if self._owns_client:
                self._client.close()

    def command(self, argv: Sequence[str]) -> Command:
        """``argv``, run without a shell: set it up with ``with_`` calls, then start or collect."""
        return Command(self, argv)

    def forward(self, port: int, *, to: tuple[str, int]) -> Tunnel:
        """Serves ``127.0.0.1:<port>`` in the box from here: each connection a box process opens
        there is joined to a new connection to ``to`` on this host. Bytes pass through as they
        are, so what that server sends, such as a ``Date`` header, reaches the box. Returns once
        the relay listens; stop it with :meth:`Tunnel.close`. Raises
        :class:`InvalidArgumentError` if the box was not built ``with_relay()``."""
        if c.RELAY_APP not in (app.name for app in self.info.apps):
            raise InvalidArgumentError("forward needs a box built with_relay()")
        relay = self.command([c.RELAY, str(port)]).with_timeout(self.info.timeout)
        return start_tunnel(relay.with_open_stdin().start(), to)


class SandboxBuilder:
    """The box to get, from :meth:`Sandbox.time_portal`. Each ``with_`` returns a new
    builder, so one can be the base of several. Anything not set takes the API's default: no
    network, 1 vCPU, 2 GiB, a 1 h lifetime, a 10 min idle timeout."""

    def __init__(self, as_of: Instant, options: Mapping[str, Any] | None = None) -> None:
        self._as_of = as_of
        self._options = dict(options or {})

    def __repr__(self) -> str:
        shown = {k: v for k, v in self._options.items() if k not in ("client", "env")}
        return f"SandboxBuilder(as_of={self._as_of!r}, {shown!r})"

    def with_client(self, client: Client) -> Self:
        """The API client, which the box leaves open; by default one of its own, from
        ``EPOCHAL_API_KEY`` and ``EPOCHAL_BASE_URL``, closed with the box."""
        return self._with(client=client)

    def with_app(self, name: str) -> Self:
        """An app mounted read-only at ``/opt/epochal/bin``, by name.

        It is a current build, not one as of ``as_of``, so a box that uses it is not strictly
        held to its instant.
        """
        return self._with(apps=(*self._options.get("apps", ()), name))

    def with_relay(self) -> Self:
        """The relay, so :meth:`Sandbox.forward` can serve the box's ports from here."""
        return self if c.RELAY_APP in self._options.get("apps", ()) else self.with_app(c.RELAY_APP)

    def with_network(self, network: Network | str) -> Self:
        """What the box may reach; :attr:`Network.NONE` when unset."""
        return self._with(network=network)

    def with_vcpus(self, vcpus: int) -> Self:
        """Virtual CPUs reserved for the box."""
        return self._with(vcpus=vcpus)

    def with_memory_gib(self, memory_gib: int) -> Self:
        """Memory reserved for the box, in GiB."""
        return self._with(memory_gib=memory_gib)

    def with_timeout(self, timeout: Duration) -> Self:
        """The box ends this long after it boots, in whole seconds."""
        return self._with(timeout=timeout)

    def with_idle_timeout(self, idle_timeout: Duration) -> Self:
        """The box ends after this long with no command running, in whole seconds."""
        return self._with(idle_timeout=idle_timeout)

    def with_env(self, name: str, value: str) -> Self:
        """A variable in the environment of every command in the box."""
        return self._with(env={**self._options.get("env", {}), name: value})

    def with_label(self, key: str, value: str) -> Self:
        """A label kept with the box, as its ``metadata``."""
        return self._with(metadata={**self._options.get("metadata", {}), key: value})

    def with_idempotency_key(self, key: str) -> Self:
        """The create's ``Idempotency-Key``, so a build retried by the caller gets the first box."""
        return self._with(idempotency_key=key)

    def build(self) -> Sandbox:
        """Creates the box and waits until it is live. A box that was made but did not come up
        is terminated before the error is raised."""
        options = dict(self._options)
        given: Client | None = options.pop("client", None)
        owns = given is None
        client = given or Client()
        info: SandboxInfo | None = None
        portal: TimePortal | None = None
        try:
            portal = client.time_portal(self._as_of)
            info = portal.create_sandbox(**options)
            while info.state is SandboxState.CREATING:
                info = portal.get_sandbox(info.id, wait=MAX_WAIT)
            if info.state is not SandboxState.LIVE:
                raise c.SandboxNotLiveError(info)
        except BaseException:
            _abandon(client, portal, info.id if info else None, owns=owns)
            raise
        return Sandbox(client.time_portal(info.as_of), info, owns_client=owns)

    def _with(self, **options: Any) -> Self:
        return type(self)(self._as_of, {**self._options, **options})


class Command:
    """A command to run, from :meth:`Sandbox.command`. Each ``with_`` returns a new command;
    :meth:`start` it to follow its output, or collect it with :meth:`output`."""

    def __init__(
        self, sandbox: Sandbox, argv: Sequence[str], options: Mapping[str, Any] | None = None
    ) -> None:
        self._sandbox = sandbox
        # A bare str is kept as is, for the API to refuse; any other sequence is copied.
        self._argv = argv if isinstance(argv, str | bytes) else list(argv)
        self._options = dict(options or {})

    def __repr__(self) -> str:
        return f"Command({self._argv!r})"

    def with_cwd(self, cwd: str) -> Self:
        """The working directory; the image's ``HOME``, ``/home/epochal``, when unset."""
        return self._with(cwd=cwd)

    def with_env(self, name: str, value: str) -> Self:
        """A variable added to the box's environment for this command."""
        return self._with(env={**self._options.get("env", {}), name: value})

    def with_timeout(self, timeout: Duration) -> Self:
        """Kills the command after this long, in whole seconds; 1 h when unset."""
        return self._with(timeout=timeout)

    def with_stdin(self, stdin: bytes) -> Self:
        """Bytes on the command's stdin, which is then closed unless :meth:`with_open_stdin`."""
        return self._with(stdin=bytes(stdin))

    def with_open_stdin(self) -> Self:
        """Keeps stdin open for :meth:`Exec.write` until :meth:`Exec.close_stdin`."""
        return self._with(stdin_open=True)

    def with_idempotency_key(self, key: str) -> Self:
        """The start's ``Idempotency-Key``, so a start retried by the caller runs it once."""
        return self._with(idempotency_key=key)

    def start(self) -> Exec:
        """Starts the command; iterate the :class:`Exec` for its output as it comes."""
        box = self._sandbox
        exec_id = box._portal.start_exec(box.id, self._argv, **self._options)
        return Exec(box._portal, box.id, exec_id)

    def output(self, *, check: bool = False) -> Output:
        """Runs the command to its end. With ``check``, one that did not exit 0 raises
        :class:`CommandError`."""
        output = self.start().output()
        if check and not output.success:
            raise c.CommandError(self._argv[0], output)
        return output

    def _with(self, **options: Any) -> Self:
        return type(self)(self._sandbox, self._argv, {**self._options, **options})


class Exec:
    """A started command. Output is read by offset, so each poll resumes where the last stopped;
    iterate it with ``for`` to get each :class:`Chunk` as it comes."""

    def __init__(self, portal: TimePortal, sandbox_id: str, exec_id: str) -> None:
        self._portal = portal
        self._sandbox_id = sandbox_id
        self.id = exec_id
        """The command's id within its box."""
        self._reader = c.Reader()
        self._reading = threading.Lock()
        self._writing = threading.Lock()
        self._next_seq = 0
        self._in_flight: dict[str, Any] | None = None
        self._unsent = memoryview(b"")
        self._exit: ExecExit | None = None

    def __repr__(self) -> str:
        return f"Exec(id={self.id!r}, exit={self._exit!r})"

    @property
    def exit(self) -> ExecExit | None:
        """How the command ended, once its output has been read to the end."""
        return self._exit

    def next(self) -> Chunk | None:
        """The next piece of output; ``None`` once the command has ended and all of its output
        was returned."""
        with self._reading:
            reader = self._reader
            while not reader.pending:
                if reader.ended is not None:
                    self._exit = reader.ended
                    return None
                reader.take(
                    self._portal.read_exec_output(
                        self._sandbox_id,
                        self.id,
                        stdout_offset=reader.offsets[Stream.STDOUT],
                        stderr_offset=reader.offsets[Stream.STDERR],
                        wait=MAX_WAIT,
                    )
                )
            return reader.pending.popleft()

    def __iter__(self) -> Iterator[Chunk]:
        while (chunk := self.next()) is not None:
            yield chunk

    def output(self) -> Output:
        """The rest of the output, and how the command ended."""
        return c.collect(list(self), self._exit)

    def write(self, data: bytes) -> None:
        """Writes ``data`` to stdin, in pieces of at most 1 MiB; needs ``with_open_stdin``. If
        this raises an error that may pass on its own, what it had not yet written is sent, once
        and in order, ahead of the next input; so don't write ``data`` again yourself. If the API
        refuses a piece, the rest is dropped."""
        with self._writing:
            self._unsent = memoryview(bytes(self._unsent) + data if self._unsent else bytes(data))
            self._flush()

    def close_stdin(self) -> None:
        """Closes stdin, for a command started ``with_open_stdin``."""
        self._input({"eof": True})

    def signal(self, signal: int) -> None:
        """Sends ``signal``, such as ``signal.SIGTERM``, to the command."""
        self._input({"signal": int(signal)})

    def kill(self) -> None:
        """Kills the command and waits until it has ended; one already over counts as killed."""
        if self._exit is None:
            # A refused stdin piece ahead of it is dropped with the rest of its write, so the
            # second try sends only the signal; only its own refusal means the command is over.
            try:
                try:
                    self.signal(c.SIGKILL)
                except APIError as e:
                    if not c.gone(e):
                        raise
                    self.signal(c.SIGKILL)
            except APIError as e:
                if not c.gone(e):
                    raise
        try:
            for _ in self:
                pass
        except APIError as e:
            if not c.gone(e):
                raise

    def _input(self, action: dict[str, Any]) -> None:
        """Applies ``action`` once, after anything an earlier failed call left unwritten."""
        with self._writing:
            self._flush()
            self._send(action)

    def _flush(self) -> None:
        """Sends what earlier calls left: first an input sent with no answer seen, which may have
        been applied (a new input under its ``seq`` would be taken for its retry and dropped), then
        the rest of a write, a piece at a time."""
        if self._in_flight is not None:
            self._send(self._in_flight)
        while self._unsent:  # A view, so taking each piece copies only that piece.
            piece, self._unsent = self._unsent[: c.PIPE], self._unsent[c.PIPE :]
            self._send({"data": bytes(piece)})

    def _send(self, action: dict[str, Any]) -> None:
        self._in_flight = action
        try:
            self._next_seq = self._portal.send_exec_input(
                self._sandbox_id, self.id, self._next_seq, **action
            )
        except EpochalError as e:
            if isinstance(e, APIError | InvalidArgumentError) and not e.retryable:
                self._in_flight, self._unsent = None, memoryview(b"")  # Refused: drop the rest.
            raise
        self._in_flight = None


class Files:
    """A box's filesystem. Paths in the box are absolute."""

    def __init__(self, sandbox: Sandbox) -> None:
        self._sandbox = sandbox
        self._portal = sandbox._portal

    def stat(self, path: str) -> FileStat:
        """Metadata of ``path``, without following a final symlink."""
        return self._portal.stat_file(self._sandbox.id, c.absolute(path))

    def read(self, path: str) -> bytes:
        """The whole file at ``path``."""
        out = io.BytesIO()
        self._read_to(c.absolute(path), out)
        return out.getvalue()

    def write(self, path: str, data: bytes) -> None:
        """Replaces the file at ``path`` with ``data``, creating missing parents."""
        self._write_from(c.absolute(path), io.BytesIO(data))

    def remove(self, path: str, *, recursive: bool = False) -> None:
        """Removes ``path``; a directory only with ``recursive``."""
        self._portal.remove_file(self._sandbox.id, c.absolute(path), recursive=recursive)

    def upload_file(self, local: str | os.PathLike[str], path: str) -> None:
        """Copies the local file ``local`` to ``path``, replacing it."""
        path = c.absolute(path)
        with open(local, "rb") as f:
            self._write_from(path, f)

    def download_file(self, path: str, local: str | os.PathLike[str]) -> None:
        """Copies the file at ``path`` to ``local``."""
        path = c.absolute(path)
        self.stat(path)
        with open(local, "wb") as f:
            try:
                self._read_to(path, f)
            except BaseException:
                with contextlib.suppress(OSError):
                    os.remove(local)
                raise

    def upload_dir(self, local: str | os.PathLike[str], path: str) -> None:
        """Copies the tree under the local directory ``local`` into ``path``, replacing files
        there. Nothing uploaded dates the host: entries carry no mtime or owner."""
        path = c.absolute(path)
        temp = c.temp_name("u")
        with tempfile.TemporaryFile() as tarball:
            c.tar_tree(local, tarball)
            try:
                self._write_from(temp, tarball)
            except EpochalError:
                with contextlib.suppress(EpochalError):
                    self._portal.remove_file(self._sandbox.id, temp)
                raise
        self._sandbox.command(c.extract_argv(temp, path)).output(check=True)

    def download_dir(self, path: str, local: str | os.PathLike[str]) -> None:
        """Copies the tree under ``path`` into the local directory ``local``, made if missing.
        Links that could point outside ``local`` and special files are skipped."""
        path = c.absolute(path)
        temp = c.temp_name("d")
        self._sandbox.command(c.archive_argv(path, temp)).output(check=True)
        with tempfile.TemporaryFile() as spool:
            try:
                self._read_to(temp, spool)
            finally:
                with contextlib.suppress(EpochalError):
                    self._portal.remove_file(self._sandbox.id, temp)
            spool.seek(0)
            c.untar(spool, local)

    def _read_to(self, path: str, out: IO[bytes]) -> None:
        """Streams the file in order. The first read takes as much as the box sends; once it
        names the size, the rest come in ranges of that length, a few in flight, and a short one
        restarts them where it ended. Without a size, reads go one by one."""
        offset, total, chunk = 0, None, None
        while True:
            spans = c.spans(offset, total, chunk)
            calls = (partial(self._read_span, path, at, n) for at, n in spans)
            with Window[tuple[int, int | None, FileRange]]() as window:
                window.fill(calls)
                while window:
                    at, length, got = window.next()
                    if got.start != at:
                        raise EpochalError(f"{path}: asked for byte {at}, got byte {got.start}")
                    out.write(got.data)
                    offset, total = offset + len(got.data), got.total
                    if not got.data or (total is not None and offset >= total):
                        return
                    chunk = chunk or len(got.data)
                    if length is not None and len(got.data) < length:
                        break
                    window.fill(calls)

    def _read_span(
        self, path: str, at: int, length: int | None
    ) -> tuple[int, int | None, FileRange]:
        got = self._portal.read_file(self._sandbox.id, path, offset=at, length=length)
        return at, length, got

    def _write_from(self, path: str, data: IO[bytes]) -> None:
        """Writes ``data`` in chunks: the first, at offset 0, replaces the file and goes alone;
        the rest land at their offsets, a few in flight, each read once there is room for it."""
        write = partial(self._portal.write_file, self._sandbox.id, path)
        chunk = data.read(c.WRITE_CHUNK)
        write(chunk, offset=0)
        offset = len(chunk)
        with Window[FileStat]() as window:
            while len(chunk) == c.WRITE_CHUNK:
                window.room()
                chunk = data.read(c.WRITE_CHUNK)
                if chunk:
                    window.start(partial(write, chunk, offset=offset))
                offset += len(chunk)
            while window:
                window.next()


def _abandon(
    client: Client, portal: TimePortal | None, sandbox_id: str | None, *, owns: bool
) -> None:
    """Ends a box that did not come up, then closes the client if it was made for it."""
    try:
        if portal is not None and sandbox_id is not None:
            with contextlib.suppress(Exception):
                portal.terminate_sandbox(sandbox_id)
    finally:
        if owns:
            client.close()
