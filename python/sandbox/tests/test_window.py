from __future__ import annotations

import asyncio
import threading
import time

import pytest
from epochal.sandbox._common import IN_FLIGHT
from epochal.sandbox._window import AsyncWindow, Window


class Boom(Exception):
    pass


def test_a_failed_call_is_raised_while_an_older_one_stalls() -> None:
    async def go() -> None:
        stalled = asyncio.Event()

        async def stall() -> int:
            await stalled.wait()
            return 1

        async def fail() -> int:
            raise Boom

        async with AsyncWindow[int]() as window:
            window.start(stall)
            window.start(fail)
            with pytest.raises(Boom):
                await asyncio.wait_for(window.next(), 5)

    asyncio.run(go())


def test_a_failed_call_is_raised_while_an_older_one_stalls_on_threads() -> None:
    stalled = threading.Event()

    def fail() -> int:
        raise Boom

    with Window[int]() as window:
        window.start(lambda: int(stalled.wait(5)))
        window.start(fail)
        with pytest.raises(Boom):
            window.next()
        assert not stalled.is_set(), "raised while the older call still ran"
        stalled.set()


def test_results_come_back_in_the_order_the_calls_started() -> None:
    async def go() -> list[int]:
        async def after(delay: float, value: int) -> int:
            await asyncio.sleep(delay)
            return value

        async with AsyncWindow[int]() as window:
            window.start(lambda: after(0.05, 1))
            window.start(lambda: after(0, 2))
            return [await window.next(), await window.next()]

    assert asyncio.run(go()) == [1, 2]


def test_waiting_on_the_oldest_call_does_not_spin_when_a_newer_one_is_done() -> None:
    with Window[int]() as window:
        window.start(lambda: (time.sleep(0.3), 1)[1])
        window.start(lambda: 2)
        cpu = time.process_time()
        assert (window.next(), window.next()) == (1, 2)
        assert time.process_time() - cpu < 0.1, "busy-waited on the finished newer call"


def test_room_uses_a_slot_freed_by_a_finished_call_while_the_rest_stall() -> None:
    held = threading.Event()
    with Window[int]() as window:
        window.start(lambda: 0)
        for _ in range(IN_FLIGHT - 1):
            window.start(lambda: int(held.wait(5)))
        time.sleep(0.05)
        started = time.monotonic()
        window.room()
        assert time.monotonic() - started < 1, "waited on the stalled calls"
        assert len(window) == IN_FLIGHT - 1
        assert not held.is_set()
        held.set()

    async def go() -> None:
        released = asyncio.Event()

        async def stall() -> int:
            await released.wait()
            return 1

        async def finish() -> int:
            return 0

        async with AsyncWindow[int]() as window:
            window.start(finish)
            for _ in range(IN_FLIGHT - 1):
                window.start(stall)
            await asyncio.sleep(0)
            await asyncio.wait_for(window.room(), 1)
            assert len(window) == IN_FLIGHT - 1
            assert not released.is_set()

    asyncio.run(go())
