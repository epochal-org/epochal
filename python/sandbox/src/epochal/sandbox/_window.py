"""Up to :data:`IN_FLIGHT` calls at once, for moving a file in chunks: :class:`AsyncWindow` on the
event loop, :class:`Window` on threads. A call starts as soon as one before it ends, so a slow one
holds back only what must wait for its result, and a failed one is raised as soon as it ends.
Leaving the block abandons the calls not taken; :class:`Window` first waits out any running, as a
thread can't be stopped.

Each wait works from one snapshot of which calls had ended, and reads the result of every ended
call it drops, so a call that ends meanwhile is seen on the next look, never lost."""

from __future__ import annotations

import asyncio
from collections import deque
from collections.abc import Callable, Coroutine, Iterator
from concurrent.futures import FIRST_COMPLETED, Future, ThreadPoolExecutor, wait
from typing import Any, Generic, Self, TypeVar

from epochal.sandbox._common import IN_FLIGHT

T = TypeVar("T")


class AsyncWindow(Generic[T]):
    def __init__(self) -> None:
        self._running: deque[asyncio.Task[T]] = deque()

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(self, *_: object) -> None:
        for task in self._running:
            task.cancel()
        await asyncio.gather(*self._running, return_exceptions=True)

    def __len__(self) -> int:
        return len(self._running)

    def start(self, call: Callable[[], Coroutine[Any, Any, T]]) -> None:
        self._running.append(asyncio.ensure_future(call()))

    def fill(self, calls: Iterator[Callable[[], Coroutine[Any, Any, T]]]) -> None:
        """Starts calls from ``calls`` until the window is full or they run out."""
        while len(self) < IN_FLIGHT and (call := next(calls, None)):
            self.start(call)

    async def next(self) -> T:
        """The result of the oldest call, once it ends; any call's failure, once that one ends."""
        while True:
            pending = _check(self._running)
            if self._running[0].done():
                return self._running.popleft().result()
            await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)

    async def room(self) -> None:
        """Drops the calls that ended, raising a failure among them, and waits only while the
        window is still full."""
        while True:
            pending = _reap(self._running)
            if len(self) < IN_FLIGHT:
                return
            await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)


class Window(Generic[T]):
    def __init__(self) -> None:
        self._pool = ThreadPoolExecutor(IN_FLIGHT, "epochal-files")
        self._running: deque[Future[T]] = deque()

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_: object) -> None:
        self._pool.shutdown(cancel_futures=True)

    def __len__(self) -> int:
        return len(self._running)

    def start(self, call: Callable[[], T]) -> None:
        self._running.append(self._pool.submit(call))

    def fill(self, calls: Iterator[Callable[[], T]]) -> None:
        """Starts calls from ``calls`` until the window is full or they run out."""
        while len(self) < IN_FLIGHT and (call := next(calls, None)):
            self.start(call)

    def next(self) -> T:
        """The result of the oldest call, once it ends; any call's failure, once that one ends."""
        while True:
            oldest = self._running[0]
            pending = _check(self._running)
            if oldest not in pending:
                return self._running.popleft().result()
            wait(pending, return_when=FIRST_COMPLETED)

    def room(self) -> None:
        """Drops the calls that ended, raising a failure among them, and waits only while the
        window is still full."""
        while True:
            pending = _reap(self._running)
            if len(self) < IN_FLIGHT:
                return
            wait(pending, return_when=FIRST_COMPLETED)


Call = TypeVar("Call", asyncio.Task[Any], Future[Any])


def _check(running: deque[Call]) -> list[Call]:
    """The calls not yet ended, as of one look; raises the failure of any that had."""
    ended = [c for c in running if c.done()]
    for call in ended:
        if call.exception() is not None:
            call.result()
    return [c for c in running if c not in ended]


def _reap(running: deque[Call]) -> list[Call]:
    """:func:`_check`, then drops the calls that had ended; each one's result was read."""
    pending = _check(running)
    for call in [c for c in running if c not in pending]:
        running.remove(call)
    return pending
