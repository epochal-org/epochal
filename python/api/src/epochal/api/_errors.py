from __future__ import annotations

import datetime as dt

import httpx

API_KEY_ENV = "EPOCHAL_API_KEY"

_RETRYABLE_STATUSES = frozenset({408, 429, 500, 502, 503, 504, *range(520, 525)})


class EpochalError(Exception):
    """Base class of every error this SDK raises."""

    @property
    def retryable(self) -> bool:
        """Whether the same call may succeed if repeated."""
        return False


class MissingApiKeyError(EpochalError):
    """No API key was given and ``EPOCHAL_API_KEY`` is unset or empty."""

    def __init__(self) -> None:
        super().__init__(f"no API key: pass api_key= or set {API_KEY_ENV}")


class InvalidArgumentError(EpochalError, ValueError):
    """An argument the API can't take, refused before any request."""


class TransportError(EpochalError):
    """The request got no complete response: connect, timeout or connection failure.

    The original ``httpx`` exception is the ``__cause__``.
    """

    def __init__(self, message: str, *, retryable: bool = True) -> None:
        super().__init__(message)
        self._retryable = retryable

    @property
    def retryable(self) -> bool:
        return self._retryable


class APIError(EpochalError):
    """The API, or a proxy in front of it, refused the request."""

    status: int
    """The HTTP status."""
    kind: str
    """A stable machine-readable code such as ``invalid_api_key`` or ``conflict``; ``unknown`` when
    the body is not the API's error envelope."""
    message: str
    """What went wrong, in one sentence."""
    request_id: str | None
    """Identifies the request in Epochal's logs, if the API assigned one."""
    retry_after: dt.timedelta | None
    """How long to wait before retrying, from ``Retry-After``, when the API says."""
    next_seq: int | None
    """For an out-of-order exec input, the ``seq`` the command expects next."""

    def __init__(
        self,
        status: int,
        kind: str,
        message: str,
        *,
        request_id: str | None = None,
        retry_after: dt.timedelta | None = None,
        next_seq: int | None = None,
    ) -> None:
        super().__init__(f"{message} (HTTP {status})")
        self.status = status
        self.kind = kind
        self.message = message
        self.request_id = request_id
        self.retry_after = retry_after
        self.next_seq = next_seq

    @property
    def retryable(self) -> bool:
        return self.status in _RETRYABLE_STATUSES

    @classmethod
    def from_response(cls, response: httpx.Response) -> APIError:
        """The error a non-2xx ``response`` stands for."""
        try:
            error = response.json()["error"]
            kind, message = str(error["type"]), str(error["message"])
            request_id, next_seq = error.get("request_id"), error.get("next_seq")
        except (ValueError, KeyError, TypeError):
            kind, message = "unknown", response.reason_phrase or "request failed"
            request_id = next_seq = None
        return cls(
            response.status_code,
            kind,
            message,
            request_id=request_id if isinstance(request_id, str) else None,
            retry_after=_retry_after(response),
            next_seq=next_seq if isinstance(next_seq, int) else None,
        )


_MAX_RETRY_AFTER = 86_400.0


class DecodeError(EpochalError):
    """A successful response whose body did not match the documented shape."""


def _retry_after(response: httpx.Response) -> dt.timedelta | None:
    """Only the delay-seconds form; a date would need the server's clock to be trusted. A delay
    past a day is held to a day, which is past any retry budget anyway."""
    try:
        seconds = float(response.headers["retry-after"])
    except (KeyError, ValueError):
        return None
    return dt.timedelta(seconds=min(seconds, _MAX_RETRY_AFTER)) if seconds >= 0 else None
