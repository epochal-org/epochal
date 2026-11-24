"""Calls on a sandbox of either flavour, made blocking."""

from __future__ import annotations

import asyncio
import inspect
from collections.abc import Iterator
from typing import Any


class Blocking:
    """``target``'s methods and attributes, each awaited on ``loop`` if it is a coroutine."""

    def __init__(self, target: Any, loop: asyncio.AbstractEventLoop | None) -> None:
        self._target = target
        self._loop = loop

    def __getattr__(self, name: str) -> Any:
        value = getattr(self._target, name)
        if not callable(value):
            return _wrap(value, self._loop)

        def call(*args: Any, **kwargs: Any) -> Any:
            return _wrap(value(*args, **kwargs), self._loop)

        return call

    def __iter__(self) -> Iterator[Any]:
        if self._loop is None:
            return iter(self._target)

        async def drain() -> list[Any]:
            return [chunk async for chunk in self._target]

        return iter(asyncio.run_coroutine_threadsafe(drain(), self._loop).result())


def _wrap(value: Any, loop: asyncio.AbstractEventLoop | None) -> Any:
    if loop is not None and inspect.isawaitable(value):
        value = asyncio.run_coroutine_threadsafe(_await(value), loop).result(timeout=30)
    if type(value).__module__.startswith("epochal.sandbox"):
        return Blocking(value, loop)
    return value


async def _await(value: Any) -> Any:
    return await value
