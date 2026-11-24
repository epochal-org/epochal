"""The time portal: the web, and Linux sandboxes, held to a past instant. Its calls are methods of
:class:`TimePortal`, held to one instant, from :meth:`epochal.api.Client.time_portal`
(:class:`AsyncTimePortal` from :meth:`epochal.api.AsyncClient.time_portal`); its request and
response types are here too.

    from epochal.api import Client
    from epochal.api.time_portal import PageStatus

    with Client() as client:
        page = client.time_portal("2025-03-12").fetch("https://example.com/news")
        print(page.content if page.status is PageStatus.SUCCESS else page.page_error)
"""

from epochal.api._async_client import AsyncTimePortal
from epochal.api._client import TimePortal
from epochal.api._types import (
    Category,
    Document,
    Entity,
    ExecExit,
    ExecFrame,
    ExecOutput,
    ExitReason,
    FetchResponse,
    FileList,
    FileRange,
    FileStat,
    FileType,
    Network,
    PageError,
    PageStatus,
    SandboxApp,
    SandboxFailure,
    SandboxInfo,
    SandboxList,
    SandboxState,
    SearchResponse,
    SearchResult,
    Stream,
)

__all__ = [
    "AsyncTimePortal",
    "Category",
    "Document",
    "Entity",
    "ExecExit",
    "ExecFrame",
    "ExecOutput",
    "ExitReason",
    "FetchResponse",
    "FileList",
    "FileRange",
    "FileStat",
    "FileType",
    "Network",
    "PageError",
    "PageStatus",
    "SandboxApp",
    "SandboxFailure",
    "SandboxInfo",
    "SandboxList",
    "SandboxState",
    "SearchResponse",
    "SearchResult",
    "Stream",
    "TimePortal",
]
