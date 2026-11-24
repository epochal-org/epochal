"""What ``Client`` and ``AsyncClient`` share: configuration, the request a call sends, and turning
a response into a result or an error, and an error into a retry or a raise."""

from __future__ import annotations

import datetime as dt
import os
import random
import secrets
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from importlib import metadata
from typing import Any, Generic, TypeVar

import httpx
from epochal.api import _wire
from epochal.api._errors import (
    API_KEY_ENV,
    APIError,
    DecodeError,
    EpochalError,
    InvalidArgumentError,
    MissingApiKeyError,
    TransportError,
)

T = TypeVar("T")

TIME_PORTAL = "/v1/time-portal"
SANDBOX = "/sandbox"
MAX_WAIT = dt.timedelta(seconds=55)
"""The longest the API holds a call that waits, such as ``get_sandbox`` with a ``wait``."""
DEFAULT_BASE_URL = "https://api.epochal.com"
BASE_URL_ENV = "EPOCHAL_BASE_URL"
DEFAULT_MAX_RETRIES = 5
_GRACE = 5.0
_LONGEST_ANSWER = 80.0
"""The longest the API takes to answer any call: a file write's deadline, past ``MAX_WAIT`` plus
the server's own slack on a wait."""
DEFAULT_TIMEOUT = httpx.Timeout(10.0, read=_LONGEST_ANSWER + _GRACE)
"""Connect within 10 s; give up on a response that sends nothing for 5 s past the longest answer."""

_FIRST_BACKOFF = 0.5
_MAX_BACKOFF = 8.0
_RETRY_BUDGET = 60.0
"""The most one call sleeps across its retries; a ``Retry-After`` past it ends the call."""
_NOT_PROCESSED = frozenset({408, 429, 503})
"""Statuses that mean the server did no work, the only ones a billed call is retried on."""

try:
    _VERSION = metadata.version("epochal-api")
except metadata.PackageNotFoundError:
    _VERSION = "0"
USER_AGENT = f"epochal-api-python/{_VERSION}"


@dataclass(frozen=True, slots=True)
class Call(Generic[T]):
    """One request, built once and sent as many times as it is retried."""

    method: str
    path: str
    parse: Callable[[httpx.Response], T]
    params: list[tuple[str, str]] = field(default_factory=list)
    headers: dict[str, str] = field(default_factory=dict)
    json: Any = None
    content: bytes | None = None
    accept: Callable[[httpx.Response], bool] = lambda r: r.is_success
    hold: bool = False
    """The API may hold the response back, a wait up to ``MAX_WAIT`` and a file write longer, which
    the read timeout must allow."""
    billed: bool = False
    """Search and fetch are billed and carry no idempotency key: only retried if not processed."""


class Config:
    __slots__ = ("api_key", "base_url", "max_retries")

    def __init__(self, api_key: str | None, base_url: str | None, max_retries: int) -> None:
        api_key = api_key or os.environ.get(API_KEY_ENV)
        if not api_key:
            raise MissingApiKeyError
        if max_retries < 0:
            raise InvalidArgumentError("max_retries must be >= 0")
        self.api_key = api_key
        base_url = base_url or os.environ.get(BASE_URL_ENV) or DEFAULT_BASE_URL
        if not base_url.startswith(("https://", "http://")):
            raise InvalidArgumentError(f"base URL {base_url!r} is not http(s)")
        self.base_url = base_url.rstrip("/")
        self.max_retries = max_retries

    def request(self, http: httpx.Client | httpx.AsyncClient, call: Call[Any]) -> httpx.Request:
        headers = {"x-api-key": self.api_key, "user-agent": USER_AGENT, **call.headers}
        timeout, longest = http.timeout, _LONGEST_ANSWER + _GRACE
        if call.hold and timeout.read is not None and timeout.read < longest:
            timeout = httpx.Timeout(
                timeout.connect, read=longest, write=timeout.write, pool=timeout.pool
            )
        return http.build_request(
            call.method,
            f"{self.base_url}{call.path}",
            params=tuple(call.params),
            headers=headers,
            json=call.json,
            content=call.content,
            timeout=timeout,
        )

    def backoff(
        self, call: Call[Any], error: EpochalError, attempt: int, slept: float
    ) -> float | None:
        """Seconds to sleep before retrying ``call`` after ``error`` on ``attempt`` (from 1), or
        ``None`` to raise it. Sandbox calls are safe to repeat: reads change nothing, writes land at
        an offset, stdin goes by ``seq``, and creates and starts carry an idempotency key. A billed
        call is repeated only when the server says it did nothing."""
        if not error.retryable or attempt > self.max_retries:
            return None
        if call.billed and not (isinstance(error, APIError) and error.status in _NOT_PROCESSED):
            return None
        ceiling = min(_FIRST_BACKOFF * 2 ** min(attempt - 1, 16), _MAX_BACKOFF)
        wait = ceiling * random.uniform(0.5, 1.0)
        if isinstance(error, APIError) and error.retry_after is not None:
            wait = max(wait, error.retry_after.total_seconds())
        return None if slept + wait > _RETRY_BUDGET else wait


def transport_error(error: httpx.HTTPError) -> TransportError:
    """A request built wrong locally fails the same way every time, so it is not retried."""
    local = isinstance(error, httpx.UnsupportedProtocol | httpx.LocalProtocolError)
    return TransportError(f"request failed: {error!r}", retryable=not local)


def result(call: Call[T], response: httpx.Response, *, retried: bool) -> T:
    """The call's result from a read ``response``, or the error it stands for. A retried delete
    that finds its target gone found the earlier attempt's work, whose reply was lost."""
    if retried and call.method == "DELETE" and response.status_code == 404:
        return call.parse(response)
    if not call.accept(response):
        raise APIError.from_response(response)
    try:
        return call.parse(response)
    except (ValueError, KeyError, TypeError, AttributeError) as e:
        raise DecodeError(f"unreadable response to {call.method} {call.path}: {e!r}") from e


def check_argv(argv: Sequence[str]) -> list[str]:
    """``argv`` as a list, refusing a bare string (``"ls -l"`` is one program name, not two)."""
    if isinstance(argv, str | bytes):
        raise InvalidArgumentError("argv is a sequence of strings, run without a shell; not a str")
    argv = list(argv)
    if not argv or not argv[0] or not all(isinstance(a, str) for a in argv):
        raise InvalidArgumentError("argv must name a program and hold only strings")
    return argv


def box(sandbox_id: str, rest: str = "") -> str:
    return f"{SANDBOX}/{_wire.segment(sandbox_id)}{rest}"


def execution(sandbox_id: str, exec_id: str, rest: str) -> str:
    return box(sandbox_id, f"/execs/{_wire.segment(exec_id)}{rest}")


def as_of(value: _wire.Instant) -> dict[str, str]:
    return {"accept-datetime": _wire.instant_header(value)}


def idempotent(key: str | None) -> dict[str, str]:
    """Every create and start carries a key, so the client's own retries never repeat one. A
    random one, so it says nothing about when it was made."""
    return {"idempotency-key": key or secrets.token_hex(16)}


def known(value: Any, given: object) -> str:
    if value.value == "unknown":
        raise InvalidArgumentError(f"{given!r} is not a {type(value).__name__}")
    return str(value.value)


def compact(**fields: Any) -> dict[str, Any]:
    return {k: v for k, v in fields.items() if v is not None and v != [] and v != {}}


def params(**fields: object) -> list[tuple[str, str]]:
    return [(k, str(v)) for k, v in fields.items() if v is not None]


def json(from_json: Callable[[Any], T]) -> Callable[[httpx.Response], T]:
    return lambda r: from_json(r.json())


def none(_: httpx.Response) -> None:
    return None


def content_range(value: str | None) -> tuple[int | None, int | None]:
    """``bytes <first>-<last>/<size>`` or ``bytes */<size>``: the first byte and the size."""
    span, _, size = (value or "").removeprefix("bytes ").rpartition("/")
    first = span.partition("-")[0]
    return (int(first) if first.isdigit() else None, int(size) if size.isdigit() else None)
