"""Runs Claude Code or Codex in a box held to a past instant: a minimal Python version of the
``epochal time-portal run`` CLI.

Your model key never leaves your machine except to your model endpoint. This script reads
``EPOCHAL_LLM_API_KEY`` here and adds it only to the model calls it sends from here to
``EPOCHAL_LLM_BASE_URL``: as ``x-api-key`` on Claude Code's Anthropic Messages calls, as a bearer
token on Codex's and Grok's OpenAI Responses calls. The box gets a placeholder key, and
``epochal-mcp`` is started without either variable. Prompts and model replies do pass through the
Epochal API, since that is how the relay reaches the box; the key is not in them.

The box gets the harness as an app, and the relay, which serves two of its ports from here:

- ``127.0.0.1:8787``: the harness's model calls, sent on to your model endpoint with your key.
  Replies keep only ``content-type``, and their ``created`` epochs become the box's instant.
- ``127.0.0.1:8788``: ``epochal-mcp``, run here at the box's instant, whose ``web_search``
  and ``web_fetch`` read the web as it stood, with your Epochal key.

From a clone of https://github.com/epochal-org/epochal (``cargo install`` needs Rust):

    cargo install --path mcp                     # puts epochal-mcp on your PATH
    pip install epochal-sandbox
    export EPOCHAL_API_KEY=... EPOCHAL_LLM_API_KEY=...
    export EPOCHAL_LLM_BASE_URL=https://api.anthropic.com
    python python/sandbox/examples/agent.py claude claude-sonnet-5-5 2025-03-12 "Find ..."

The harness's events stream to stdout as JSON Lines.
"""

from __future__ import annotations

import contextlib
import datetime as dt
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from epochal.api.time_portal import ExitReason
from epochal.sandbox import Sandbox, Stream

PORT = 8787
VCPUS, MEMORY_GIB = 1, 2  # The API default; the free hour runs out sooner on a larger box.
WIND_DOWN = dt.timedelta(minutes=5)
"""Kept back from the harness, out of the box's life, for downloads and cleanup after it."""
MCP_PORT = 8788
PLACEHOLDER_KEY = "epochal-local-proxy"
MODEL_CALLS = {"/v1/messages", "/v1/messages/count_tokens", "/v1/responses"}
BASE_ENV, KEY_ENV = "EPOCHAL_LLM_BASE_URL", "EPOCHAL_LLM_API_KEY"
APPS = {"claude": "claude-code", "codex": "codex", "grok": "grok"}
GROK_HOME = "/tmp/epochal-grok"
# Built-ins that reach xAI's own services rather than the model endpoint.
GROK_DISALLOWED = "image_gen,image_edit,image_to_video,reference_to_video,send_feedback"
FORWARDED = ("content-type", "accept", "anthropic-version", "anthropic-beta", "openai-beta")
CREATED = re.compile(rb'"(created|created_at)"(\s*):(\s*)\d+')


def launch(
    harness: str, model: str, prompt: str, cwd: str, port: int, mcp_port: int
) -> tuple[list[str], dict[str, str]]:
    """The harness's command and environment, pointed at the relay's ports."""
    base = f"http://127.0.0.1:{port}"
    mcp = f"http://127.0.0.1:{mcp_port}/mcp"
    if harness == "claude":
        env = {
            "ANTHROPIC_BASE_URL": base,
            "ANTHROPIC_AUTH_TOKEN": PLACEHOLDER_KEY,
            "ANTHROPIC_MODEL": model,
            "ANTHROPIC_DEFAULT_OPUS_MODEL": model,
            "ANTHROPIC_DEFAULT_SONNET_MODEL": model,
            "ANTHROPIC_DEFAULT_HAIKU_MODEL": model,
            "CLAUDE_CODE_SUBAGENT_MODEL": model,
            "IS_SANDBOX": "1",
            "DISABLE_AUTOUPDATER": "1",
            "DISABLE_TELEMETRY": "1",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
        }
        mcp_config = json.dumps({"mcpServers": {"epochal": {"type": "http", "url": mcp}}})
        argv = [
            "claude", "-p", prompt, "--output-format", "stream-json", "--verbose",
            "--dangerously-skip-permissions", "--no-session-persistence",
            "--strict-mcp-config", "--mcp-config", mcp_config,
            # Its own web tools would read today's web; `epochal` serves the as-of web.
            "--disallowedTools", "WebSearch WebFetch", "--model", model,
        ]  # fmt: skip
        return argv, env
    if harness == "grok":
        # grok reads its model and MCP servers only from config.toml, so the box writes one first.
        config = (
            f'[models]\ndefault = "epochal"\n\n[model.epochal]\nmodel = {json.dumps(model)}\n'
            f'base_url = "{base}/v1"\napi_backend = "responses"\napi_key = "{PLACEHOLDER_KEY}"\n\n'
            f'[mcp_servers.epochal]\nurl = "{mcp}"\n\n[cli]\nauto_update = false\n'
        )
        write = (
            'mkdir -p "$GROK_HOME" && printf "%s" "$EPOCHAL_GROK_CONFIG" > "$GROK_HOME/config.toml"'
            ' && exec "$0" "$@"'
        )
        argv = [
            "/bin/sh", "-c", write, "grok", "-p", prompt,
            "--output-format", "streaming-messages-json", "--always-approve",
            "--disable-web-search", "--disallowed-tools", GROK_DISALLOWED,
            "-m", "epochal", "--cwd", cwd,
        ]  # fmt: skip
        return argv, {"GROK_HOME": GROK_HOME, "EPOCHAL_GROK_CONFIG": config}
    overrides = [
        'model_provider="epochal"',
        'model_providers.epochal.name="epochal"',
        f'model_providers.epochal.base_url="{base}/v1"',
        'model_providers.epochal.env_key="OPENAI_API_KEY"',
        'model_providers.epochal.wire_api="responses"',
        "check_for_update_on_startup=false",
        'web_search="disabled"',
        f'mcp_servers.epochal.url="{mcp}"',
    ]
    argv = [
        "codex", "exec", "--json", "--dangerously-bypass-approvals-and-sandbox",
        "--skip-git-repo-check", "--ephemeral", "-C", cwd, "-m", model,
    ]  # fmt: skip
    for kv in overrides:
        argv += ["-c", kv]
    return [*argv, prompt], {"OPENAI_API_KEY": PLACEHOLDER_KEY}


def upstream() -> tuple[str, str]:
    """Your model endpoint and its key, from this machine's environment."""
    base, key = os.environ.get(BASE_ENV), os.environ.get(KEY_ENV)
    if not base or not key:
        raise SystemExit(f"set {BASE_ENV} and {KEY_ENV} to your model endpoint and its key")
    url = urllib.parse.urlsplit(base)
    if url.scheme not in ("http", "https") or not url.hostname or "@" in url.netloc:
        raise SystemExit(f"{BASE_ENV} must be an http or https URL with no user or password")
    return base.rstrip("/"), key


def target(base: str, path: str) -> str:
    """Where a harness's ``/v1/...`` call goes at ``base``, as the CLI sends it: a base ending in
    ``/v1`` already has it; Anthropic's adds it; an OpenAI-compatible one adds it only to a bare
    host, as OpenAI's SDK does."""
    rest = path.removeprefix("/v1")
    has_path = bool(urllib.parse.urlsplit(base).path.strip("/"))
    if base.endswith("/v1") or (has_path and not path.startswith("/v1/messages")):
        return base + rest
    return base + "/v1" + rest


class NoRedirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args: Any) -> None:
        return None


def serve(harness: str, as_of: dt.datetime, port: int) -> ThreadingHTTPServer:
    """The model proxy the box's ``127.0.0.1:<port>`` is joined to."""
    base, key = upstream()
    epoch = b"%d" % as_of.timestamp()
    hosts = {f"127.0.0.1:{port}", f"localhost:{port}"}
    opener = urllib.request.build_opener(NoRedirects)  # A redirect would take your key along.

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self) -> None:
            body = self.body()
            if self.headers.get("host") not in hosts:  # As the box sends it; not a web page's.
                self.reply(403, b'{"error":{"message":"not from the box"}}')
            elif self.path.split("?")[0] in MODEL_CALLS:
                self.model(body)
            else:
                self.reply(404, b'{"error":{"message":"only model calls are forwarded"}}')

        def body(self) -> bytes:
            if "chunked" not in self.headers.get("transfer-encoding", "").lower():
                return self.rfile.read(int(self.headers.get("content-length", 0)))
            parts = []  # grok streams its requests.
            while size := int(self.rfile.readline().split(b";")[0], 16):
                parts.append(self.rfile.read(size))
                self.rfile.readline()
            while self.rfile.readline() not in (b"\r\n", b"\n", b""):
                pass  # Trailers.
            return b"".join(parts)

        def model(self, body: bytes) -> None:
            headers = {h: v for h in FORWARDED if (v := self.headers.get(h))}
            if self.path.startswith("/v1/messages"):  # Anthropic Messages, from Claude Code.
                headers["x-api-key"] = key
            else:
                headers["authorization"] = f"Bearer {key}"
            url = target(base, self.path)
            request = urllib.request.Request(url, body, headers, method="POST")
            try:
                answer = opener.open(request, timeout=600)
            except urllib.error.HTTPError as e:
                answer = e
            except OSError:
                self.reply(502, b'{"error":{"message":"the model endpoint failed"}}')
                return
            with answer:
                self.send_response(answer.status)
                self.send_header("Content-Type", answer.headers.get("content-type", ""))
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                # Line by line, as streamed events come, so no epoch is split across two reads.
                pending = b""
                while chunk := answer.read1(65536):
                    cut = (pending + chunk).rfind(b"\n") + 1
                    whole, pending = (pending + chunk)[:cut], (pending + chunk)[cut:]
                    self.send_chunk(whole)
                self.send_chunk(pending)
                self.wfile.write(b"0\r\n\r\n")

        def send_chunk(self, data: bytes) -> None:
            if data:
                data = CREATED.sub(rb'"\1"\2:\g<3>' + epoch, data)
                self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))

        def reply(self, status: int, data: bytes) -> None:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def send_response(self, code: int, message: str | None = None) -> None:
            # The stock one adds a Date header, which would tell the box today's date.
            self.log_request(code)
            self.send_response_only(code, message)

        def log_message(self, format: str, *args: Any) -> None:
            pass

    return ThreadingHTTPServer(("127.0.0.1", 0), Handler)


def mcp_binary() -> str:
    binary = os.environ.get("EPOCHAL_MCP") or shutil.which("epochal-mcp")
    if not binary:
        raise SystemExit("build epochal-mcp and put it on PATH, or name it in EPOCHAL_MCP")
    return binary


@contextlib.contextmanager
def mcp_server(binary: str, as_of: dt.datetime) -> Iterator[int]:
    """``epochal-mcp`` serving the web at ``as_of``; yields its local port once it listens."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port: int = s.getsockname()[1]
    instant = as_of.strftime("%Y-%m-%dT%H:%M:%SZ")
    # It needs only your Epochal key; no model key or endpoint, under any name, goes to it.
    model_vars = ("EPOCHAL_LLM_", "ANTHROPIC_", "OPENAI_", "XAI_")
    env = {k: v for k, v in os.environ.items() if not k.startswith(model_vars)}
    server = subprocess.Popen(
        [binary, "--time-portal", instant, "--serve", f"http:{port}"], env=env
    )
    try:
        deadline = time.monotonic() + 10
        while not listening(port):
            if server.poll() is not None or time.monotonic() > deadline:
                raise SystemExit(f"epochal-mcp did not listen on port {port}")
            time.sleep(0.05)
        yield port
    finally:
        server.terminate()
        server.wait()


def listening(port: int) -> bool:
    with contextlib.suppress(OSError), socket.create_connection(("127.0.0.1", port), 1):
        return True
    return False


def run(
    harness: str,
    model: str,
    as_of: str,
    prompt: str,
    cwd: str = "/home/epochal",
    port: int = PORT,
    mcp_port: int = MCP_PORT,
) -> int:
    """Runs the harness to its end, writing its events to stdout; returns its exit code."""
    upstream()  # Fail on a missing key or binary before a box is made.
    binary = mcp_binary()
    app = APPS[harness]
    argv, env = launch(harness, model, prompt, cwd, port, mcp_port)
    builder = Sandbox.time_portal(as_of).with_app(app).with_relay()
    built = time.monotonic()
    with (
        builder.with_vcpus(VCPUS).with_memory_gib(MEMORY_GIB).build() as box,
        mcp_server(binary, box.as_of) as mcp_local,
        serve(harness, box.as_of, port) as proxy,
    ):
        threading.Thread(target=proxy.serve_forever, daemon=True).start()
        try:
            with (
                box.forward(port, to=("127.0.0.1", proxy.server_port)),
                box.forward(mcp_port, to=("127.0.0.1", mcp_local)),
            ):
                life = box.info.timeout - (time.monotonic() - built) * dt.timedelta(seconds=1)
                budget = int((life - min(WIND_DOWN, box.info.timeout / 4)).total_seconds())
                command = box.command(argv).with_cwd(cwd).with_timeout(max(budget, 1))
                for name, value in env.items():
                    command = command.with_env(name, value)
                execution = command.start()
                for chunk in execution:
                    out = sys.stdout if chunk.stream is Stream.STDOUT else sys.stderr
                    out.buffer.write(chunk.data)
                    out.flush()
        finally:
            proxy.shutdown()
    ended = execution.exit  # As a shell reports it.
    if ended is None:
        return 1
    if ended.code is not None:
        return ended.code
    if ended.signal:
        return 128 + ended.signal
    return {ExitReason.TIMEOUT: 124, ExitReason.OUT_OF_MEMORY: 137}.get(ended.reason, 1)


if __name__ == "__main__":
    if len(sys.argv) != 5 or sys.argv[1] not in APPS:
        raise SystemExit("usage: agent.py claude|codex|grok MODEL AS_OF PROMPT")
    harness, model, as_of, prompt = sys.argv[1:]
    sys.exit(run(harness, model, as_of, prompt))
