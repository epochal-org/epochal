"""Writes the blocking twin of the async sandbox module and the async twin of the blocking client,
so each pair can't drift apart; ``tests`` fail when a twin is stale.

    python scripts/unasync.py

A few exact rules handle what differs in kind; the rest is renaming. Each rule must match, so an
edit that a rule no longer fits fails loudly. The output is formatted by ruff, a dev dependency."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
API = ROOT / "api/src/epochal/api"
SANDBOX = ROOT / "sandbox/src/epochal/sandbox"
HEADER = "# Generated from {} by scripts/unasync.py; edit that file, then run the script.\n"

TO_ASYNC = [
    ("import time\n", "import asyncio\n"),
    (
        "A blocking Epochal API client. Safe to share across threads;",
        "An asyncio Epochal API client:",
    ),
    ("as a context manager, or call", "as an async context manager, or await"),
    ("httpx.Client", "httpx.AsyncClient"),
    ("self._http.close()", "await self._http.aclose()"),
    ("def __enter__(self) -> Client:", "async def __aenter__(self) -> AsyncClient:"),
    ("def __exit__(", "async def __aexit__("),
    ("        self.close()\n", "        await self.close()\n"),
    ("self._http.send(", "await self._http.send("),
    ("time.sleep(wait)", "await asyncio.sleep(wait)"),
    ("return self._attempt(", "return await self._attempt("),
]

TO_SYNC = [
    ("import asyncio\n", "import threading\n"),
    ("asyncio.Lock()", "threading.Lock()"),
    ("await asyncio.shield(_abandon(", "_abandon("),
    ("owns=owns))", "owns=owns)"),
    ("AsyncTunnel, serve", "Tunnel, start_tunnel"),
    ("return await serve(", "return start_tunnel("),
    ("await (await self.start()).output()", "self.start().output()"),
    (
        "    def __aiter__(self) -> AsyncIterator[Chunk]:\n        return self._chunks()\n\n"
        "    async def _chunks(self) -> AsyncIterator[Chunk]:\n"
        "        while (chunk := await self.next()) is not None:",
        "    def __iter__(self) -> Iterator[Chunk]:\n        while (chunk := self.next()) is not None:",
    ),
    ("[chunk async for chunk in self]", "list(self)"),
    ("with ``async for``", "with ``for``"),
    ("an async context manager, or await", "a context manager, or call"),
]


def to_async_client(s: str) -> str:
    s = _apply(TO_ASYNC, s)
    s = re.sub(r"(?<![.\w])(Client|TimePortal)\b", r"Async\1", s)
    s = re.sub(
        r"\n    def (?!base_url\b|time_portal\b|as_of\b|_headers\b|client\b|__)",
        "\n    async def ",
        s,
    )
    s = s.replace("return self._client._send(", "return await self._client._send(")
    return re.sub(r"(\n {8,}(?:return )?)self\._send\(", r"\1await self._send(", s)


def to_sync_sandbox(s: str) -> str:
    s = _apply(TO_SYNC, s)
    s = re.sub(r"await asyncio\.to_thread\((\w+(?:\.\w+)?), ", r"\1(", s)
    s = re.sub(r"__a(enter|exit)__", r"__\1__", s)
    s = (
        re.sub(r"\bAsync(?=[A-Z])", "", s)
        .replace("async with ", "with ")
        .replace("async for ", "for ")
    )
    s = s.replace("async def ", "def ").replace("await ", "")
    left = [line for line in s.splitlines() if re.search(r"\basync\b|\bawait\b", line)]
    if left:
        raise SystemExit(f"unasync: async code left in the blocking twin: {left}")
    return s


def _apply(rules: list[tuple[str, str]], s: str) -> str:
    for old, new in rules:
        if old not in s:
            raise SystemExit(f"unasync: rule no longer matches: {old!r}")
        s = s.replace(old, new)
    return s


def _ruff(path: Path, source: str) -> str:
    for args in (["check", "--select", "I", "--fix", "--quiet"], ["format", "--quiet"]):
        source = subprocess.run(
            [sys.executable, "-m", "ruff", *args, "--stdin-filename", str(path), "-"],
            input=source,
            capture_output=True,
            text=True,
            check=True,
            cwd=ROOT,
        ).stdout
    return source


TWINS = [
    (API / "_client.py", API / "_async_client.py", to_async_client),
    (SANDBOX / "_async.py", SANDBOX / "_sync.py", to_sync_sandbox),
]


def twins() -> dict[Path, str]:
    """Each generated file and what it should hold."""
    return {
        dst: _ruff(dst, HEADER.format(src.name) + make(src.read_text())) for src, dst, make in TWINS
    }


if __name__ == "__main__":
    for path, text in twins().items():
        path.write_text(text)
