"""Epochal sandboxes: Linux boxes. Their one mode today is the time portal, which holds a box's
clock and network to a past instant, so nothing run in it can see what came after it.

    from epochal.sandbox import Network, Sandbox

    with Sandbox.time_portal("2025-03-12").with_network(Network.TIME_PORTAL).build() as sandbox:
        sandbox.files.upload_dir("./src", "/home/epochal/src")
        result = sandbox.command(["python3", "analyze.py"]).with_cwd("/home/epochal/src").output()
        print(result.text)

:class:`AsyncSandbox` has the same methods as coroutines. Every call goes through
:mod:`epochal.api`, which retries failures that may pass on their own.
"""

from epochal.api import (
    AsyncClient,
    Client,
)
from epochal.api.time_portal import (
    ExecExit,
    FileStat,
    FileType,
    Network,
    Stream,
)
from epochal.sandbox._async import (
    AsyncCommand,
    AsyncExec,
    AsyncFiles,
    AsyncSandbox,
    AsyncSandboxBuilder,
)
from epochal.sandbox._common import (
    Chunk,
    CommandError,
    Output,
    SandboxNotLiveError,
    TunnelError,
)
from epochal.sandbox._sync import Command, Exec, Files, Sandbox, SandboxBuilder
from epochal.sandbox._tunnel import AsyncTunnel, Tunnel

__all__ = [
    "AsyncClient",
    "AsyncCommand",
    "AsyncExec",
    "AsyncFiles",
    "AsyncSandbox",
    "AsyncSandboxBuilder",
    "AsyncTunnel",
    "Chunk",
    "Client",
    "Command",
    "CommandError",
    "Exec",
    "ExecExit",
    "FileStat",
    "FileType",
    "Files",
    "Network",
    "Output",
    "Sandbox",
    "SandboxBuilder",
    "SandboxNotLiveError",
    "Stream",
    "Tunnel",
    "TunnelError",
]
