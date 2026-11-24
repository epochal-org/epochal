"""Every test runs against both ``Sandbox`` and ``AsyncSandbox``, made blocking by
:class:`Blocking`. The async flavour runs on one event loop thread for the whole test, so an
``AsyncExec`` or ``AsyncTunnel`` made in one call is used on the loop it belongs to."""

from __future__ import annotations

import asyncio
import threading
from collections.abc import Callable, Iterator
from typing import Any

import pytest
from epochal.api import AsyncClient, Client
from epochal.sandbox import AsyncSandbox, Sandbox
from fake_sandbox import KEY, FakeSandboxApi
from sandbox_harness import Blocking

AS_OF = "2025-03-01T00:00:00Z"


@pytest.fixture
def fake() -> Iterator[FakeSandboxApi]:
    api = FakeSandboxApi()
    yield api
    api.close()


def _built(builder: Any, as_of: Any, settings: dict[str, Any]) -> Any:
    """``time_portal(as_of)`` with ``with_<name>(value)`` for each setting; a tuple is its
    arguments."""
    for name, value in settings.items():
        args = value if isinstance(value, tuple) else () if value is True else (value,)
        builder = getattr(builder, f"with_{name}")(*args)
    return builder


@pytest.fixture(params=["sync", "async"])
def create(
    request: pytest.FixtureRequest, fake: FakeSandboxApi, monkeypatch: pytest.MonkeyPatch
) -> Iterator[Callable[..., Blocking]]:
    """``create(as_of=…, max_retries=…, **settings)``: a box of the parametrized flavour on
    ``fake``, built with ``with_<name>(value)`` per setting. It gets a client closed after the
    test, which ``max_retries`` sets, unless ``own_client=True``. Boxes are left to the fake."""
    monkeypatch.setenv("EPOCHAL_API_KEY", KEY)
    monkeypatch.setenv("EPOCHAL_BASE_URL", fake.url)
    flavour = Sandbox if request.param == "sync" else AsyncSandbox
    loop = asyncio.new_event_loop() if flavour is AsyncSandbox else None
    thread = threading.Thread(target=loop.run_forever, daemon=True) if loop else None
    if thread:
        thread.start()
    clients: list[Any] = []

    def run(value: Any) -> Any:
        if loop is None:
            return value
        return asyncio.run_coroutine_threadsafe(value, loop).result(30)

    def make(
        as_of: Any = AS_OF, max_retries: int = 5, own_client: bool = False, **settings: Any
    ) -> Blocking:
        async def client() -> AsyncClient:
            return AsyncClient(max_retries=max_retries)

        builder = flavour.time_portal(as_of)
        if not own_client:
            clients.append(run(client()) if loop else Client(max_retries=max_retries))
            builder = builder.with_client(clients[-1])
        return Blocking(run(_built(builder, as_of, settings).build()), loop)

    yield make
    for c in clients:
        run(c.close())
    if loop and thread:
        run(loop.shutdown_asyncgens())
        loop.call_soon_threadsafe(loop.stop)
        thread.join()
        loop.close()
