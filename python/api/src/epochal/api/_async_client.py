# Generated from _client.py by scripts/unasync.py; edit that file, then run the script.
from __future__ import annotations

import asyncio
import base64
import dataclasses
import datetime as dt
from collections.abc import Mapping, Sequence
from types import TracebackType
from typing import Any, TypeVar

import httpx
from epochal.api import _base as b
from epochal.api import _wire
from epochal.api._base import DEFAULT_TIMEOUT, Call, Config
from epochal.api._errors import EpochalError, InvalidArgumentError
from epochal.api._types import (
    Category,
    Document,
    ExecOutput,
    FetchResponse,
    FileList,
    FileRange,
    FileStat,
    Network,
    SandboxInfo,
    SandboxList,
    SandboxState,
    SearchResponse,
)
from epochal.api._wire import Duration, Instant

T = TypeVar("T")


class AsyncClient:
    """An asyncio Epochal API client: reuses connections. Each API
    module is a handle from it, such as :meth:`time_portal`.

    Use it as an async context manager, or await :meth:`close`, to release its connections.

    Args:
        api_key: Defaults to ``EPOCHAL_API_KEY``.
        base_url: Defaults to ``EPOCHAL_BASE_URL``, then ``https://api.epochal.com``.
        max_retries: How many times a call is retried after a dropped connection, a timeout, or
            a 408, 429, 500, 502, 503, 504 or 520-524, with jittered exponential backoff that
            honors ``Retry-After``, sleeping at most 60 s in all. Search, fetch and page are
            billed, so they are retried only on 408, 429 and 503. ``0`` disables retries.
        timeout: For the client this makes; by default 10 s to connect and 85 s to read.
        http_client: An ``httpx.AsyncClient`` to send with, for proxies or custom transports. It is
            not closed by :meth:`close`; redirects are never followed through it.
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        max_retries: int = b.DEFAULT_MAX_RETRIES,
        timeout: httpx.Timeout | float | None = None,
        http_client: httpx.AsyncClient | None = None,
    ) -> None:
        self._config = Config(api_key, base_url, max_retries)
        self._owned = http_client is None
        self._http = http_client or httpx.AsyncClient(
            timeout=DEFAULT_TIMEOUT if timeout is None else timeout
        )

    @property
    def base_url(self) -> str:
        """Where requests go."""
        return self._config.base_url

    def __repr__(self) -> str:
        return f"AsyncClient(base_url={self.base_url!r})"

    def time_portal(self, as_of: Instant) -> AsyncTimePortal:
        """The time portal held to ``as_of``: web search, web fetch and sandboxes as they stood
        then. A bare day means its last second, 23:59:59 UTC."""
        return AsyncTimePortal(self, as_of)

    async def close(self) -> None:
        """Releases the connections of the client this made; one passed in is left open."""
        if self._owned:
            await self._http.aclose()

    async def __aenter__(self) -> AsyncClient:
        return self

    async def __aexit__(
        self, t: type[BaseException] | None, e: BaseException | None, tb: TracebackType | None
    ) -> None:
        await self.close()

    async def _send(self, call: Call[T]) -> T:
        slept, attempt = 0.0, 0
        while True:
            attempt += 1
            try:
                return await self._attempt(call, retried=attempt > 1)
            except EpochalError as e:
                wait = self._config.backoff(call, e, attempt, slept)
                if wait is None:
                    raise
            await asyncio.sleep(wait)
            slept += wait

    async def _attempt(self, call: Call[T], *, retried: bool) -> T:
        try:
            request = self._config.request(self._http, call)
            response = await self._http.send(request, follow_redirects=False)
        except httpx.HTTPError as e:
            raise b.transport_error(e) from e
        return b.result(call, response, retried=retried)


class AsyncTimePortal:
    """The time portal held to one instant, from :meth:`AsyncClient.time_portal`: search, fetch and
    new boxes are held to :attr:`as_of`. Calls on an existing box take its id, and the box keeps
    the instant it was made with. Each call is sent through the client, with its connections and
    retries."""

    __slots__ = ("_as_of", "_client")

    def __init__(self, client: AsyncClient, as_of: Instant) -> None:
        self._client = client
        self._as_of = _wire.instant(as_of, day_end=True)

    @property
    def as_of(self) -> dt.datetime:
        """The instant search, fetch and new boxes are held to, UTC."""
        return self._as_of

    @property
    def _headers(self) -> dict[str, str]:
        return b.as_of(self._as_of)

    def __repr__(self) -> str:
        return f"AsyncTimePortal({self._client!r}, as_of={self.as_of.isoformat()!r})"

    @property
    def client(self) -> AsyncClient:
        """The client its calls go through."""
        return self._client

    async def _send(self, call: Call[T]) -> T:
        return await self._client._send(
            dataclasses.replace(call, path=f"{b.TIME_PORTAL}{call.path}")
        )

    async def search(
        self,
        query: str,
        *,
        num_results: int | None = None,
        domains: Sequence[str] = (),
        exclude_domains: Sequence[str] = (),
        languages: Sequence[str] = (),
        published_after: Instant | None = None,
        categories: Sequence[Category | str] = (),
        include_text: bool = False,
        extra_body: Mapping[str, Any] | None = None,
    ) -> SearchResponse:
        """Web search as it would have answered at :attr:`as_of`.

        Args:
            num_results: 1 to 10; the server defaults to 10.
            domains: Only pages from these sites, subdomains included.
            exclude_domains: Never pages from these sites, subdomains included.
            languages: Preferred result languages as BCP 47 tags, most preferred first.
            published_after: Only pages published at or after this instant.
            categories: Only pages in these categories; empty searches everything. A name
                this version doesn't list is sent as given.
            include_text: Also return each page's full text.
            extra_body: Fields this version doesn't model, sent in the body as given.
        """
        body = b.compact(
            query=query,
            num_results=num_results,
            domains=list(domains),
            exclude_domains=list(exclude_domains),
            languages=list(languages),
            published_after=None
            if published_after is None
            else _wire.instant_param(published_after),
            categories=[str(c) for c in categories],
            include_text=include_text or None,
        ) | dict(extra_body or {})
        parse = b.json(SearchResponse.from_json)
        return await self._send(
            Call("POST", "/search", parse, headers=self._headers, json=body, billed=True)
        )

    async def fetch(
        self,
        url: str,
        *,
        max_characters: int | None = None,
        extra_body: Mapping[str, Any] | None = None,
    ) -> FetchResponse:
        """One page as it read at :attr:`as_of`, as markdown with its metadata. A page that could
        not be read is returned, with ``status`` :attr:`PageStatus.ERROR`, not raised.

        Args:
            max_characters: Cut ``content`` to this many characters; the server defaults to
                100,000.
            extra_body: Fields this version doesn't model, sent in the body as given.
        """
        body = b.compact(url=url, max_characters=max_characters) | dict(extra_body or {})
        parse = b.json(FetchResponse.from_json)
        return await self._send(
            Call("POST", "/fetch", parse, headers=self._headers, json=body, billed=True)
        )

    async def page(self, url: str) -> Document:
        """The archived page itself (HTML, PDF, ...) as it stood at :attr:`as_of`, with its type.
        A page that could not be read is returned, with ``status`` :attr:`PageStatus.ERROR`, not
        raised."""

        def parse(r: httpx.Response) -> Document:
            return Document.from_response(r.content, r.headers)

        body = {"url": url, "format": "page"}
        return await self._send(
            Call("POST", "/fetch", parse, headers=self._headers, json=body, billed=True)
        )

    async def create_sandbox(
        self,
        *,
        network: Network | str | None = None,
        vcpus: int | None = None,
        memory_gib: int | None = None,
        timeout: Duration | None = None,
        idle_timeout: Duration | None = None,
        env: Mapping[str, str] | None = None,
        metadata: Mapping[str, str] | None = None,
        apps: Sequence[str] = (),
        idempotency_key: str | None = None,
    ) -> SandboxInfo:
        """Creates a box held to :attr:`as_of`; it is usually still ``CREATING``. Anything not set
        takes the API's default: no network, 1 vCPU, 2 GiB, a 1 h lifetime and a 10 min idle
        timeout. The box boots the newest Ubuntu build dated on or before :attr:`as_of`.

        Args:
            timeout: The box ends this long after it boots.
            idle_timeout: The box ends after this long with no command running.
            env: In the environment of every command in the box. Values are not logged.
            metadata: Labels kept with the box and returned with it.
            apps: Apps mounted read-only at ``/opt/epochal/bin``, by name. An app is a current
                build, not one as of ``as_of``, so a box that uses one is not strictly held to
                ``as_of``.
            idempotency_key: A create repeated with the same key makes one box. Defaults to a
                fresh key, which covers this call's own retries.
        """
        body = b.compact(
            network=None if network is None else b.known(Network(network), network),
            vcpus=vcpus,
            memory_gib=memory_gib,
            timeout=None if timeout is None else _wire.duration(timeout),
            idle_timeout=None if idle_timeout is None else _wire.duration(idle_timeout),
            env=dict(env or {}),
            metadata=dict(metadata or {}),
            apps=list(apps),
        )
        headers = {**self._headers, **b.idempotent(idempotency_key)}
        parse = b.json(SandboxInfo.from_json)
        return await self._send(Call("POST", b.SANDBOX, parse, headers=headers, json=body))

    async def get_sandbox(self, sandbox_id: str, *, wait: Duration | None = None) -> SandboxInfo:
        """A box as it is now, or, with ``wait``, once it leaves ``CREATING`` or ``wait`` passes,
        whichever is first; the API holds a wait for at most :data:`MAX_WAIT`."""
        params = [] if wait is None else [("wait", _wire.duration(wait))]
        parse = b.json(SandboxInfo.from_json)
        return await self._send(
            Call("GET", b.box(sandbox_id), parse, params=params, hold=wait is not None)
        )

    async def list_sandboxes(
        self,
        *,
        state: SandboxState | str | None = None,
        cursor: str | None = None,
        limit: int | None = None,
    ) -> SandboxList:
        """One page of the account's boxes; ``limit`` is 1 to 100."""
        params = b.params(
            state=None if state is None else b.known(SandboxState(state), state),
            cursor=cursor,
            limit=limit,
        )

        def parse(r: httpx.Response) -> SandboxList:
            j = r.json()
            return SandboxList(
                [SandboxInfo.from_json(s) for s in j["sandboxes"]], j.get("next_cursor")
            )

        return await self._send(Call("GET", b.SANDBOX, parse, params=params))

    async def terminate_sandbox(self, sandbox_id: str) -> None:
        """Ends a box now. Terminating an ended box succeeds."""
        await self._send(Call("DELETE", b.box(sandbox_id), b.none))

    async def start_exec(
        self,
        sandbox_id: str,
        argv: Sequence[str],
        *,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
        timeout: Duration | None = None,
        stdin: bytes = b"",
        stdin_open: bool = False,
        idempotency_key: str | None = None,
    ) -> str:
        """Starts ``argv``, run without a shell, and returns the command's id.

        Args:
            cwd: The working directory; the image's ``HOME`` by default.
            timeout: Kills the command after this long; 1 h by default.
            stdin: Bytes written to stdin at start, at most 1 MiB.
            stdin_open: Keeps stdin open for :meth:`send_exec_input`; otherwise it is closed
                after ``stdin``.
            idempotency_key: A start repeated with the same key runs the command once.
        """
        body = b.compact(
            argv=b.check_argv(argv),
            env=dict(env or {}),
            cwd=cwd,
            timeout=None if timeout is None else _wire.duration(timeout),
            stdin_b64=base64.b64encode(stdin).decode() if stdin else None,
            stdin_open=stdin_open or None,
        )
        path, headers = b.box(sandbox_id, "/execs"), b.idempotent(idempotency_key)
        return await self._send(
            Call("POST", path, lambda r: str(r.json()["id"]), headers=headers, json=body)
        )

    async def read_exec_output(
        self,
        sandbox_id: str,
        exec_id: str,
        *,
        stdout_offset: int = 0,
        stderr_offset: int = 0,
        wait: Duration | None = None,
        max_bytes: int | None = None,
    ) -> ExecOutput:
        """A command's output after the offsets; with ``wait``, held until there is new output,
        the command ends, or ``wait`` passes (at most :data:`MAX_WAIT`)."""
        params = b.params(
            stdout_offset=stdout_offset,
            stderr_offset=stderr_offset,
            wait=None if wait is None else _wire.duration(wait),
            max_bytes=max_bytes,
        )
        path, parse = b.execution(sandbox_id, exec_id, "/output"), b.json(ExecOutput.from_json)
        return await self._send(Call("GET", path, parse, params=params, hold=wait is not None))

    async def send_exec_input(
        self,
        sandbox_id: str,
        exec_id: str,
        seq: int,
        *,
        data: bytes | None = None,
        eof: bool = False,
        signal: int | None = None,
    ) -> int:
        """Applies one of ``data`` (at most 1 MiB), ``eof`` or ``signal`` as input number ``seq``
        (from 0) and returns the next ``seq``. An input already applied is not applied again; one
        out of order raises :class:`APIError` with the ``next_seq`` the command expects."""
        if (data is not None) + eof + (signal is not None) != 1:
            raise InvalidArgumentError("send exactly one of data, eof or signal")
        body = b.compact(
            seq=seq,
            data_b64=None if data is None else base64.b64encode(data).decode(),
            eof=eof or None,
            signal=signal,
        )
        path = b.execution(sandbox_id, exec_id, "/input")
        return await self._send(Call("POST", path, lambda r: int(r.json()["next_seq"]), json=body))

    async def stat_file(self, sandbox_id: str, path: str) -> FileStat:
        """Metadata of ``path``, without following a final symlink."""
        parse = b.json(FileStat.from_json)
        return await self._send(
            Call("GET", b.box(sandbox_id, "/files/stat"), parse, params=[("path", path)])
        )

    async def list_files(
        self,
        sandbox_id: str,
        path: str,
        *,
        recursive: bool = False,
        cursor: str | None = None,
        limit: int | None = None,
    ) -> FileList:
        """One page of the entries under ``path``; ``limit`` is 1 to 100."""
        params = b.params(
            path=path, recursive="true" if recursive else None, cursor=cursor, limit=limit
        )

        def parse(r: httpx.Response) -> FileList:
            j = r.json()
            return FileList([FileStat.from_json(e) for e in j["entries"]], j.get("next_cursor"))

        return await self._send(Call("GET", b.box(sandbox_id, "/files/list"), parse, params=params))

    async def read_file(
        self, sandbox_id: str, path: str, *, offset: int = 0, length: int | None = None
    ) -> FileRange:
        """Bytes of ``path`` from ``offset``: ``length`` of them, or as many as the box sends in
        one call (16 MiB). A read at or past the end returns no bytes rather than raising."""
        if offset < 0 or (length is not None and length <= 0):
            raise InvalidArgumentError(
                "a read starts at offset >= 0 and asks for at least one byte"
            )
        last = "" if length is None else str(offset + length - 1)

        def parse(r: httpx.Response) -> FileRange:
            first, total = b.content_range(r.headers.get("content-range"))
            if r.status_code == httpx.codes.REQUESTED_RANGE_NOT_SATISFIABLE:
                return FileRange(offset, b"", total)
            if r.status_code == httpx.codes.PARTIAL_CONTENT:
                return FileRange(offset if first is None else first, r.content, total)
            if r.status_code == httpx.codes.OK:
                return FileRange(0, r.content, len(r.content))
            raise ValueError(f"{path}: a range read answered HTTP {r.status_code}")

        def accept(r: httpx.Response) -> bool:
            return r.is_success or r.status_code == httpx.codes.REQUESTED_RANGE_NOT_SATISFIABLE

        headers = {"range": f"bytes={offset}-{last}"}
        url = b.box(sandbox_id, "/files")
        return await self._send(Call("GET", url, parse, [("path", path)], headers, accept=accept))

    async def write_file(
        self, sandbox_id: str, path: str, data: bytes, *, offset: int = 0
    ) -> FileStat:
        """Writes ``data`` (at most 8 MiB) into ``path`` at ``offset`` and returns the file after.
        Offset 0 replaces the file; a later offset writes into it, zero-filling any gap."""
        params = [("path", path), ("offset", str(offset))]
        headers = {"content-type": "application/octet-stream"}
        parse = b.json(FileStat.from_json)
        url = b.box(sandbox_id, "/files")
        return await self._send(
            Call("PUT", url, parse, params, headers, content=bytes(data), hold=True)
        )

    async def remove_file(self, sandbox_id: str, path: str, *, recursive: bool = False) -> None:
        """Removes ``path``; a directory only with ``recursive``."""
        params = b.params(path=path, recursive="true" if recursive else None)
        await self._send(Call("DELETE", b.box(sandbox_id, "/files"), b.none, params=params))
