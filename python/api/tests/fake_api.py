"""An in-memory API answering through ``httpx.MockTransport``, and a client of either flavour on
it whose calls are made blocking, so each test runs against ``Client`` and ``AsyncClient`` alike."""

from __future__ import annotations

import asyncio
import inspect
import json
from collections.abc import Callable
from dataclasses import dataclass, field
from typing import Any

import httpx
from epochal.api import AsyncClient, Client

KEY = "good-key"
AS_OF = "2026-03-01T00:00:00Z"


@dataclass
class Fake:
    """Answers each request with ``route``'s response, logging what it saw. ``queue`` holds
    responses to return first, one per request, as a server failing then recovering would."""

    route: Callable[[httpx.Request], httpx.Response]
    seen: list[httpx.Request] = field(default_factory=list)
    queue: list[httpx.Response | Exception] = field(default_factory=list)
    slept: list[float] = field(default_factory=list)
    """Each sleep between retries, which the test client skips."""

    def __call__(self, request: httpx.Request) -> httpx.Response:
        request.read()
        self.seen.append(request)
        if request.headers.get("x-api-key") != KEY:
            return error(401, "invalid_api_key", "No key matches that token.")
        if self.queue:
            nxt = self.queue.pop(0)
            if isinstance(nxt, Exception):
                raise nxt
            return nxt
        return self.route(request)

    @property
    def last(self) -> httpx.Request:
        return self.seen[-1]


def error(status: int, kind: str, message: str, **extra: Any) -> httpx.Response:
    body = {"error": {"type": kind, "message": message, "request_id": "req_9Zp4kL", **extra}}
    return httpx.Response(status, json=body)


def body(request: httpx.Request) -> Any:
    return json.loads(request.content)


class Api:
    """The time portal of a client of either flavour, held to ``as_of``, its calls made
    blocking."""

    def __init__(self, client: Client | AsyncClient, as_of: Any = AS_OF) -> None:
        self.client = client
        self.portal = client.time_portal(as_of)

    def __getattr__(self, name: str) -> Callable[..., Any]:
        method = getattr(self.portal, name)

        def call(*args: Any, **kwargs: Any) -> Any:
            result = method(*args, **kwargs)
            return asyncio.run(result) if inspect.isawaitable(result) else result

        return call
