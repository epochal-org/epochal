"""What ``Sandbox`` and ``AsyncSandbox`` share, free of I/O: errors, results, output bookkeeping,
and the local half of moving directories as tar."""

from __future__ import annotations

import os
import secrets
import tarfile
from collections import deque
from collections.abc import Iterator
from dataclasses import dataclass, field
from pathlib import PurePosixPath
from typing import IO

from epochal.api import (
    APIError,
    EpochalError,
    InvalidArgumentError,
)
from epochal.api.time_portal import (
    ExecExit,
    ExecFrame,
    ExecOutput,
    ExitReason,
    SandboxInfo,
    Stream,
)

WRITE_CHUNK = 8 << 20
"""The most one file write carries."""
IN_FLIGHT = 4
"""Calls one file holds open at once past its first: enough to overlap each one's round trip."""
PIPE = 1 << 20
"""The most one stdin write carries."""
RELAY_APP = "relay"
RELAY = "/opt/epochal/bin/epochal-relay"
SIGKILL = 9

_EXTRACT = 'mkdir -p -- "$2" && tar -xmf "$1" --no-same-owner -C "$2"; s=$?; rm -f -- "$1"; exit $s'
_ARCHIVE = 'tar -cf "$2" --exclude="$3" -C "$1" . || { s=$?; rm -f -- "$2"; exit $s; }'
"""``$3`` keeps the archive out of itself when it lands under ``$1``; a failed one is removed."""


def spans(offset: int, total: int | None, chunk: int | None) -> Iterator[tuple[int, int | None]]:
    """The reads from ``offset`` to the end: ranges of ``chunk`` bytes once the size is known,
    else one of as many bytes as the box sends."""
    if total is None or chunk is None:
        return iter([(offset, None)])
    return ((at, min(chunk, total - at)) for at in range(offset, total, chunk))


class SandboxNotLiveError(EpochalError):
    """The box ended or failed before it was live."""

    def __init__(self, info: SandboxInfo) -> None:
        reason = info.error.message if info.error else "no reason given"
        super().__init__(f"sandbox {info.id} did not come up ({info.state}): {reason}")
        self.info = info


class CommandError(EpochalError):
    """A command did not exit 0: one run with ``check=True``, or one moving a directory."""

    def __init__(self, step: str, output: Output) -> None:
        stderr = output.stderr.decode(errors="replace").strip()
        super().__init__(f"{step} failed ({output.exit}): {stderr}")
        self.output = output


class TunnelError(EpochalError):
    """The relay behind a tunnel could not start, or the tunnel failed while it served."""


Chunk = ExecFrame
"""A piece of a command's output; bytes are returned once, in the order the box saw them."""


@dataclass(frozen=True, slots=True)
class Output:
    """A finished command's output."""

    stdout: bytes = field(repr=False)
    stderr: bytes = field(repr=False)
    exit: ExecExit

    @property
    def success(self) -> bool:
        """Whether it exited with status 0."""
        return self.exit.success

    @property
    def text(self) -> str:
        """stdout decoded as UTF-8, invalid bytes replaced."""
        return self.stdout.decode(errors="replace")


class Reader:
    """Where a command's output stands: offsets read to, chunks not yet returned, and the exit."""

    __slots__ = ("ended", "offsets", "pending")

    def __init__(self) -> None:
        self.offsets = {Stream.STDOUT: 0, Stream.STDERR: 0}
        self.pending: deque[Chunk] = deque()
        self.ended: ExecExit | None = None

    def take(self, output: ExecOutput) -> None:
        """Queues the new bytes in ``output``: a repeat of bytes already read is cut, and a gap
        means the box kept no copy of them."""
        for frame in output.frames:
            at = self.offsets.get(frame.stream)
            end = frame.offset + len(frame.data)
            if at is None or end <= at:
                continue
            repeated = max(at - frame.offset, 0)
            self.offsets[frame.stream] = end
            self.pending.append(
                ExecFrame(frame.stream, frame.offset + repeated, frame.data[repeated:])
            )
        self.ended = output.exit


def collect(chunks: list[Chunk], exit_: ExecExit | None) -> Output:
    out = b"".join(c.data for c in chunks if c.stream is Stream.STDOUT)
    err = b"".join(c.data for c in chunks if c.stream is not Stream.STDOUT)
    return Output(out, err, exit_ or ExecExit(ExitReason.UNKNOWN))


def gone(error: Exception) -> bool:
    """The command, or its box, is already over."""
    return isinstance(error, APIError) and error.kind in ("conflict", "not_found")


def absolute(path: str) -> str:
    """Box paths also reach shell commands as arguments, where a leading ``/`` keeps one from
    being read as an option."""
    if not path.startswith("/"):
        raise InvalidArgumentError(f"box path {path!r} must be absolute")
    return path


def temp_name(kind: str) -> str:
    """A scratch path in the box; random, never time-ordered, so it doesn't date the host."""
    return f"/tmp/.{kind}-{secrets.token_hex(8)}.tar"


def extract_argv(tarball: str, dest: str) -> list[str]:
    return ["sh", "-c", _EXTRACT, "sh", tarball, dest, ""]


def archive_argv(src: str, tarball: str) -> list[str]:
    """``tar -C src .`` names ``tarball`` ``./<rel>`` when it lies under ``src``."""
    rel = (
        PurePosixPath(tarball).relative_to(src)
        if tarball.startswith(src.rstrip("/") + "/")
        else None
    )
    exclude = f"./{rel}" if rel else tarball
    return ["sh", "-c", _ARCHIVE, "sh", src, tarball, exclude]


def tar_tree(root: str | os.PathLike[str], out: IO[bytes]) -> None:
    """Writes the tree under ``root`` to ``out`` as a tar, and rewinds it. Entries carry no mtime
    or owner, so nothing uploaded dates the host; symlinks are kept as links."""
    if not os.path.isdir(root):
        raise InvalidArgumentError(f"{root} is not a directory")

    def scrub(info: tarfile.TarInfo) -> tarfile.TarInfo:
        info.mtime = 0
        info.uid = info.gid = 0
        info.uname = info.gname = ""
        return info

    with tarfile.open(fileobj=out, mode="w", format=tarfile.PAX_FORMAT) as tar:
        tar.add(root, arcname=".", filter=scrub)
    out.seek(0)


def untar(tarball: IO[bytes], dest: str | os.PathLike[str]) -> None:
    """Unpacks into ``dest`` with the ``data`` filter: nothing outside ``dest``, no device files,
    no links pointing out, modes masked to the user's own. A member it refuses is skipped."""
    os.makedirs(dest, exist_ok=True)
    with tarfile.open(fileobj=tarball, mode="r") as tar:
        tar.extractall(dest, filter=_skip_unsafe)


def _skip_unsafe(member: tarfile.TarInfo, path: str) -> tarfile.TarInfo | None:
    try:
        return tarfile.data_filter(member, path)
    except tarfile.FilterError:
        return None
