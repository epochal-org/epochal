"""Every test runs against both ``Client`` and ``AsyncClient``."""

from __future__ import annotations

import asyncio
import time
from collections.abc import Callable
from typing import Any

import httpx
import pytest
from epochal.api import AsyncClient, Client
from fake_api import AS_OF, KEY, Api, Fake


@pytest.fixture(params=["sync", "async"])
def make_api(request: pytest.FixtureRequest, monkeypatch: pytest.MonkeyPatch) -> Callable[..., Api]:
    """``make_api(fake, as_of=AS_OF, **options)``: the time portal, held to ``as_of``, of a client
    of the parametrized flavour served by ``fake``
    through an ``httpx`` client of the caller's, set by ``timeout`` and ``follow_redirects``. It
    never sleeps between retries but records each sleep in ``fake.slept``."""
    monkeypatch.delenv("EPOCHAL_API_KEY", raising=False)
    monkeypatch.delenv("EPOCHAL_BASE_URL", raising=False)

    def make(fake: Fake, as_of: Any = AS_OF, **options: Any) -> Api:
        async def no_async_sleep(seconds: float) -> None:
            fake.slept.append(seconds)

        monkeypatch.setattr(time, "sleep", fake.slept.append)
        monkeypatch.setattr(asyncio, "sleep", no_async_sleep)
        options.setdefault("api_key", KEY)
        options.setdefault("base_url", "https://api.test/")
        transport = httpx.MockTransport(fake)
        own = {
            "transport": transport,
            "timeout": options.pop("timeout", httpx.Timeout(5.0)),
            "follow_redirects": options.pop("follow_redirects", False),
        }
        if request.param == "sync":
            http: Any = httpx.Client(**own)
            return Api(Client(http_client=http, **options), as_of)
        http = httpx.AsyncClient(**own)
        return Api(AsyncClient(http_client=http, **options), as_of)

    return make
