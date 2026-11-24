"""Encodings the API uses on the wire: instants as RFC 3339, durations as ``90s`` or ``1h30m``,
ids as one path segment."""

from __future__ import annotations

import datetime as dt
import re
from urllib.parse import quote

from epochal.api._errors import InvalidArgumentError

Instant = dt.datetime | dt.date | str
"""An instant: an aware ``datetime``, an ISO 8601 string, or a day (a ``date`` or ``"YYYY-MM-DD"``).
As an ``as_of``, a day means its last second, 23:59:59 UTC, as the API reads one; as
``published_after``, its first."""

Duration = dt.timedelta | int
"""A duration: a ``timedelta`` or whole seconds; the API takes only positive whole seconds."""

_DURATION = re.compile(r"(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?")
_SCALE = {"s": 1, "m": 60, "h": 3600}


def instant(value: Instant, *, day_end: bool = False) -> dt.datetime:
    """``value`` as an aware UTC ``datetime``; a naive one is refused as ambiguous. A day is its
    first second, or its last with ``day_end``."""
    if isinstance(value, str):
        try:
            value = (
                dt.date.fromisoformat(value)
                if len(value) == 10
                else dt.datetime.fromisoformat(value)
            )
        except ValueError:
            raise InvalidArgumentError(f"{value!r} is not an ISO 8601 instant") from None
    if not isinstance(value, dt.datetime):
        at = dt.time(23, 59, 59) if day_end else dt.time()
        return dt.datetime.combine(value, at, dt.UTC)
    if value.tzinfo is None or value.utcoffset() is None:
        raise InvalidArgumentError(f"{value} has no time zone; pass an aware datetime")
    return value.astimezone(dt.UTC)


def instant_header(value: Instant) -> str:
    """``Accept-Datetime``: RFC 3339 in whole seconds, UTC; a day is its last second."""
    return instant(value, day_end=True).strftime("%Y-%m-%dT%H:%M:%SZ")


def instant_param(value: Instant) -> str:
    return instant(value).isoformat().replace("+00:00", "Z")


def parse_instant(text: str) -> dt.datetime:
    parsed = dt.datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        raise ValueError(f"instant {text!r} has no offset")
    return parsed


def parse_optional_instant(text: str | None) -> dt.datetime | None:
    return None if text is None else parse_instant(text)


def duration(value: Duration) -> str:
    """``value`` as the API writes it: the largest unit that divides it exactly."""
    if isinstance(value, dt.timedelta):
        seconds = value.total_seconds()
        whole = int(seconds)
        if whole != seconds or value.microseconds:
            raise InvalidArgumentError(f"{value} is not a whole number of seconds")
    elif isinstance(value, int) and not isinstance(value, bool):
        whole = value
    else:
        raise InvalidArgumentError(f"{value!r} is not a timedelta or whole seconds")
    if whole <= 0:
        raise InvalidArgumentError(f"{value!r} is not a positive duration")
    for unit in ("h", "m"):
        if whole % _SCALE[unit] == 0:
            return f"{whole // _SCALE[unit]}{unit}"
    return f"{whole}s"


def parse_duration(text: str) -> dt.timedelta:
    """Whole ``h``, ``m`` and ``s``, in that order, each at most once: ``90s``, ``1h30m``."""
    match = _DURATION.fullmatch(text)
    h, m, s = (int(part or 0) for part in match.groups()) if match else (0, 0, 0)
    if h * 3600 + m * 60 + s == 0:
        raise ValueError(f"invalid duration {text!r}")
    return dt.timedelta(hours=h, minutes=m, seconds=s)


def segment(id_: str) -> str:
    """``id_`` as one path segment; empty, ``.`` and ``..`` are refused, as URLs drop them."""
    if id_ in ("", ".", ".."):
        raise InvalidArgumentError(f"{id_!r} is not an id")
    return quote(id_, safe="")
