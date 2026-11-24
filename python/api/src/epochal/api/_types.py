"""What the API returns, read from its JSON. Fields a newer server adds are ignored, and enum values
it adds read as ``UNKNOWN``, so an older SDK keeps working."""

from __future__ import annotations

import base64
import datetime as dt
import enum
from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any

from epochal.api import _wire, entities


class _Open(enum.StrEnum):
    """An enum whose values a newer server may add; those read as ``UNKNOWN``."""

    @classmethod
    def _missing_(cls, value: object) -> Any:
        return cls["UNKNOWN"]


class PageStatus(_Open):
    """Whether a fetched page was read."""

    SUCCESS = "success"
    ERROR = "error"
    UNKNOWN = "unknown"


class Network(_Open):
    """What a sandbox may reach on the network."""

    NONE = "none"
    """Nothing: names do not resolve and no connection leaves the box."""
    TIME_PORTAL = "time-portal"
    """HTTP and HTTPS to the web as it stood at the box's ``as_of``, through the time portal, and
    package mirrors held to it. Each page read is billed."""
    UNKNOWN = "unknown"


class Category(enum.StrEnum):
    """A kind of page a search can be narrowed to; a name not listed here can be passed as a str."""

    NEWS = "news"
    FINANCE = "finance"
    """SEC filings."""
    LEGAL = "legal"
    """Court opinions, statutes, regulations and sanctions."""
    PUBLICATIONS = "publications"
    """Academic articles."""
    COMPANIES = "companies"
    """Company profiles."""
    PEOPLE = "people"
    """People profiles."""
    MAPS = "maps"
    """Places."""


class SandboxState(_Open):
    """Where a sandbox is in its life."""

    CREATING = "creating"
    LIVE = "live"
    ENDED = "ended"
    """Terminated, timed out or idle; its files and command output are gone."""
    FAILED = "failed"
    """Never booted; :attr:`SandboxInfo.error` says why."""
    UNKNOWN = "unknown"


class Stream(_Open):
    """Which stream a piece of output came from."""

    STDOUT = "stdout"
    STDERR = "stderr"
    UNKNOWN = "unknown"


class FileType(_Open):
    """What kind of file a path is."""

    FILE = "file"
    DIRECTORY = "directory"
    SYMLINK = "symlink"
    OTHER = "other"
    """Anything else, including kinds a newer server names."""

    @classmethod
    def _missing_(cls, value: object) -> Any:
        return cls.OTHER


class ExitReason(_Open):
    """Why a command ended."""

    EXITED = "exited"
    SIGNALED = "signaled"
    OUT_OF_MEMORY = "out_of_memory"
    TIMEOUT = "timeout"
    SANDBOX_ENDED = "sandbox_ended"
    LOST = "lost"
    """The box stopped reporting on it."""
    UNKNOWN = "unknown"


@dataclass(frozen=True, slots=True)
class Entity:
    """A company, person, place, crypto asset or city's weather. :attr:`typed` reads ``properties``
    into the class of its type."""

    id: str
    """Its stable URL, ``https://world.epochal.com/<type>/<key>``."""
    type: str
    """Such as ``organization``, ``person`` or ``weather``; newer servers may name more."""
    schema_version: int
    """The version of ``properties``' layout for this ``type``."""
    updated_at: dt.datetime
    properties: dict[str, Any]
    """Laid out as the API reference describes for ``type`` and ``schema_version``."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Entity:
        return cls(
            id=j["id"],
            type=j["type"],
            schema_version=j["schema_version"],
            updated_at=_wire.parse_instant(j["updated_at"]),
            properties=j.get("properties") or {},
        )

    @property
    def typed(self) -> entities.TypedEntity:
        """``properties`` read into the class of ``type`` at layout ``schema_version``, such as
        :class:`~epochal.api.entities.OrganizationProperties`, or
        :class:`~epochal.api.entities.UnknownProperties` for a type or layout this version doesn't
        model.
        Raises :class:`~epochal.api.DecodeError` when they don't match the documented layout;
        Rust's ``Entity::typed`` returns a ``Result`` instead.

        The result holds ``properties`` only: ``id``, ``type``, ``schema_version`` and
        ``updated_at`` stay on this entity. Fields this version doesn't model are left out of it;
        ``properties`` keeps them. Each access reads ``properties`` again; keep the result to use
        it twice."""
        return entities.typed(self.type, self.schema_version, self.properties)


@dataclass(frozen=True, slots=True)
class SearchResult:
    """One search result."""

    title: str
    url: str
    snippet: str
    """A short passage from the page that matches the query."""
    text: str | None = None
    """The page's extracted text, when ``include_text`` was set."""
    published_at: dt.datetime | None = None
    domain: str | None = None
    language: str | None = None
    """BCP 47 tag, if detected."""
    entity: Entity | None = None
    """The entity this ``https://world.epochal.com/`` result is the page of, as fetch returns it
    for ``url``; ``None`` for every other result."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> SearchResult:
        entity = j.get("entity")
        return cls(
            title=j["title"],
            url=j["url"],
            snippet=j["snippet"],
            text=j.get("text"),
            published_at=_wire.parse_optional_instant(j.get("published_at")),
            domain=j.get("domain"),
            language=j.get("language"),
            entity=Entity.from_json(entity) if entity is not None else None,
        )


@dataclass(frozen=True, slots=True)
class SearchResponse:
    """Web search results, ranked best first."""

    as_of: dt.datetime
    request_id: str
    """Identifies the request in Epochal's logs; quote it when reporting a problem."""
    results: list[SearchResult]
    entities: list[Entity] = field(default_factory=list)
    """Reserved; entities come on :attr:`SearchResult.entity`."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> SearchResponse:
        return cls(
            as_of=_wire.parse_instant(j["as_of"]),
            request_id=j["request_id"],
            results=[SearchResult.from_json(r) for r in j["results"]],
            entities=[Entity.from_json(e) for e in j.get("entities") or ()],
        )


@dataclass(frozen=True, slots=True)
class PageError:
    """Why a page could not be read."""

    message: str
    http_status_code: int | None = None
    """The status the site answered with; ``None`` when no site answered or nothing is archived at
    the as-of instant."""

    def __str__(self) -> str:
        if self.http_status_code is None:
            return self.message
        return f"{self.message} (HTTP {self.http_status_code})"


@dataclass(frozen=True, slots=True)
class FetchResponse:
    """A page as markdown with its metadata. A page that could not be read is still a response,
    and is billed: ``status`` is :attr:`PageStatus.ERROR`, ``page_error`` says why and ``content``
    is empty."""

    as_of: dt.datetime
    request_id: str
    url: str
    final_url: str
    """The URL the content was read from, after redirects and normalization."""
    title: str
    content: str
    """The page's extracted text as markdown."""
    truncated: bool
    """Whether ``max_characters`` cut ``content`` short."""
    status: PageStatus = PageStatus.SUCCESS
    page_error: PageError | None = None
    id: str | None = None
    """Identifies the exact stored version served, if known."""
    description: str | None = None
    author: str | None = None
    published_at: dt.datetime | None = None
    language: str | None = None
    entity: Entity | None = None
    """The entity this ``https://world.epochal.com/`` URL is the page of."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> FetchResponse:
        error, entity = j.get("page_error"), j.get("entity")
        return cls(
            as_of=_wire.parse_instant(j["as_of"]),
            request_id=j["request_id"],
            url=j["url"],
            final_url=j["final_url"],
            title=j["title"],
            content=j["content"],
            truncated=j["truncated"],
            status=PageStatus(j.get("status", PageStatus.SUCCESS)),
            page_error=PageError(error["message"], error.get("http_status_code"))
            if error
            else None,
            id=j.get("id"),
            description=j.get("description"),
            author=j.get("author"),
            published_at=_wire.parse_optional_instant(j.get("published_at")),
            language=j.get("language"),
            entity=Entity.from_json(entity) if entity is not None else None,
        )


@dataclass(frozen=True, slots=True)
class Document:
    """A page as the archived document itself. A page that could not be read is still a document,
    and is billed: ``status`` is :attr:`PageStatus.ERROR`, ``page_error`` says why and ``body`` is
    empty."""

    body: bytes
    content_type: str | None = None
    """Such as ``text/html; charset=utf-8``, when the archive knows it."""
    memento_datetime: str | None = None
    """The instant the document was held to, from ``Memento-Datetime``, in RFC 3339."""
    status: PageStatus = PageStatus.SUCCESS
    page_error: PageError | None = None

    @classmethod
    def from_response(cls, body: bytes, headers: Mapping[str, str]) -> Document:
        status, error = PageStatus.SUCCESS, None
        code = headers.get("x-fetch-status-code")
        if code is not None:
            try:
                number = int(code)
            except ValueError:
                status = PageStatus.UNKNOWN
            else:
                if not 200 <= number < 300:
                    message = headers.get("x-fetch-status-message") or "The page could not be read."
                    status, error = PageStatus.ERROR, PageError(message, number or None)
        return cls(
            body,
            headers.get("content-type"),
            headers.get("memento-datetime"),
            status,
            error,
        )


@dataclass(frozen=True, slots=True)
class SandboxFailure:
    """Why a sandbox failed or ended."""

    kind: str
    """A stable machine-readable code, such as ``lifetime_expired``, ``idle_expired``,
    ``out_of_memory`` or ``spend_limit_reached``."""
    message: str


@dataclass(frozen=True, slots=True)
class SandboxApp:
    name: str
    version: str


@dataclass(frozen=True, slots=True)
class SandboxInfo:
    """A sandbox as the API describes it."""

    id: str
    state: SandboxState
    as_of: dt.datetime
    """The instant its clock and network are held to."""
    network: Network
    vcpus: int
    memory_gib: int
    timeout: dt.timedelta
    """Its lifetime from boot."""
    idle_timeout: dt.timedelta
    created_at: dt.datetime
    booted_at: dt.datetime | None = None
    """When it booted; billing starts here."""
    expires_at: dt.datetime | None = None
    ended_at: dt.datetime | None = None
    error: SandboxFailure | None = None
    metadata: dict[str, str] = field(default_factory=dict)
    apps: list[SandboxApp] = field(default_factory=list)

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> SandboxInfo:
        error = j.get("error")
        return cls(
            id=j["id"],
            state=SandboxState(j["state"]),
            as_of=_wire.parse_instant(j["as_of"]),
            network=Network(j["network"]),
            vcpus=j["vcpus"],
            memory_gib=j["memory_gib"],
            timeout=_wire.parse_duration(j["timeout"]),
            idle_timeout=_wire.parse_duration(j["idle_timeout"]),
            created_at=_wire.parse_instant(j["created_at"]),
            booted_at=_wire.parse_optional_instant(j.get("booted_at")),
            expires_at=_wire.parse_optional_instant(j.get("expires_at")),
            ended_at=_wire.parse_optional_instant(j.get("ended_at")),
            error=SandboxFailure(error["type"], error["message"]) if error else None,
            metadata=dict(j.get("metadata") or {}),
            apps=[SandboxApp(a["name"], a["version"]) for a in j.get("apps") or ()],
        )


@dataclass(frozen=True, slots=True)
class SandboxList:
    """A page of sandboxes."""

    sandboxes: list[SandboxInfo]
    next_cursor: str | None = None
    """Where the next page starts; ``None`` on the last page."""


@dataclass(frozen=True, slots=True)
class ExecExit:
    """How a command ended."""

    reason: ExitReason
    code: int | None = None
    """The exit status, when ``reason`` is :attr:`ExitReason.EXITED`."""
    signal: int | None = None
    """The signal number, when ``reason`` is :attr:`ExitReason.SIGNALED`."""

    @property
    def success(self) -> bool:
        """Whether it exited with status 0."""
        return self.reason is ExitReason.EXITED and self.code == 0

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> ExecExit:
        return cls(ExitReason(j["reason"]), j.get("code"), j.get("signal"))


@dataclass(frozen=True, slots=True)
class ExecFrame:
    """A piece of a command's output."""

    stream: Stream
    offset: int
    """Where ``data`` starts in its stream."""
    data: bytes = field(repr=False)


@dataclass(frozen=True, slots=True)
class ExecOutput:
    """Output after the offsets asked for. A frame may repeat bytes before the offset asked for,
    and a gap means the box kept no copy of the bytes in it."""

    frames: list[ExecFrame]
    exit: ExecExit | None = None
    """How the command ended, once it has and every frame up to its end was returned."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> ExecOutput:
        frames = [
            ExecFrame(
                Stream(f["stream"]), f["offset"], base64.b64decode(f["data_b64"], validate=True)
            )
            for f in j["frames"]
        ]
        exit_ = j.get("exit")
        return cls(frames, ExecExit.from_json(exit_) if exit_ else None)


@dataclass(frozen=True, slots=True)
class FileStat:
    """One path's metadata."""

    path: str
    """Its absolute path; in a directory listing, its path relative to the listed directory."""
    type: FileType
    size: int
    mode: int
    """Permission bits, e.g. ``0o644``."""
    mtime: dt.datetime | None = None
    """When it last changed, by the box's clock."""
    link_target: str | None = None

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> FileStat:
        return cls(
            path=j["path"],
            type=FileType(j["type"]),
            size=j["size"],
            mode=j["mode"],
            mtime=_wire.parse_optional_instant(j.get("mtime")),
            link_target=j.get("link_target"),
        )


@dataclass(frozen=True, slots=True)
class FileList:
    """A page of a directory's entries."""

    entries: list[FileStat]
    next_cursor: str | None = None


@dataclass(frozen=True, slots=True)
class FileRange:
    """Bytes of a file."""

    start: int
    """Where ``data`` starts in the file."""
    data: bytes = field(repr=False)
    """Empty when the read starts at or past the end."""
    total: int | None = None
    """The file's whole size, when the box said."""
