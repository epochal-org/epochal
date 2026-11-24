# epochal-sandbox

Run commands and move files in [Epochal](https://epochal.com) sandboxes: Linux machines (*boxes*).
Their one mode today is the Time Portal, which holds a box's clock and network to a past instant,
so nothing run in it can see what came after it.

```sh
pip install epochal-sandbox
export EPOCHAL_API_KEY=...
```

Requires Python 3.11.4 or later. Create an API key at https://console.epochal.com/keys. Docs:
https://docs.epochal.com.

```python
from epochal.sandbox import Network, Sandbox

with Sandbox.time_portal("2025-03-12").with_network(Network.TIME_PORTAL).build() as sandbox:
    print(sandbox.command(["date", "-u"]).output().text)

    sandbox.files.upload_dir("./src", "/home/epochal/src")
    sandbox.command(["python3", "analyze.py"]).with_cwd("/home/epochal/src").output(check=True)

    train = sandbox.command(["python3", "-u", "train.py"]).with_cwd("/home/epochal/src")
    for chunk in train.start():
        print(chunk.data.decode(errors="replace"), end="")

    sandbox.files.download_dir("/home/epochal/out", "./out")
```

`AsyncSandbox` has the same methods as coroutines:

```python
import asyncio

from epochal.sandbox import AsyncSandbox


async def main() -> None:
    async with await AsyncSandbox.time_portal("2025-03-12").build() as sandbox:
        async for chunk in await sandbox.command(["uname", "-a"]).start():
            print(chunk.data.decode(errors="replace"), end="")


asyncio.run(main())
```

- **Boxes:** `Sandbox.time_portal(as_of)` sets up a time-portal box with `with_network`,
  `with_vcpus`, `with_memory_gib`, `with_timeout`, `with_idle_timeout`, `with_env`, `with_label`,
  `with_app`, `with_relay`, `with_client` and `with_idempotency_key`. Every box boots the newest Ubuntu build
  dated on or before `as_of`, and its package mirrors are held to `as_of` too. An app is a current
  build, so a box that uses one is not strictly held to `as_of`. Unset, a box gets
  no network, 1 vCPU, 2 GiB, a 1 h lifetime and a 10 min idle timeout. A bare day as `as_of`
  means its last second, 23:59:59 UTC.
- **Lifecycle:** `build()` waits until the box is live, and terminates a box that fails to come
  up. Leaving the `with` block terminates it; a box you don't terminate runs, and is billed, until
  its timeout.
- **Commands:** `sandbox.command(argv)` sets one up with `with_cwd`, `with_env`, `with_timeout`,
  `with_stdin`, `with_open_stdin` and `with_idempotency_key`. `output()` runs it to its end;
  `check=True` raises `CommandError` if it didn't exit 0. `start()` returns an `Exec` to iterate
  for output as it comes, `write` to (after `with_open_stdin`), `signal` or `kill`. Commands run as
  `root` in `/home/epochal`.
- **Builders** are immutable: each `with_` call returns a new one, so one can be the base of
  several.
- **Files:** `read`/`write` bytes, `upload_file`/`download_file`, and `upload_dir`/`download_dir`,
  which move a tree as one tar. Uploads carry no host mtimes or owners, and downloads never write
  outside the target directory.
- **Errors:** everything raised is an `EpochalError` from
  [`epochal-api`](https://pypi.org/project/epochal-api/), which makes
  every call and retries it safely; plus `SandboxNotLiveError` (the box failed or ended before it
  was live; `.info` says why), `CommandError` (`.output` has the result) and `TunnelError`.

## Serving a box from your machine: the relay

A box can't reach your machine, and usually can't reach the internet either. The relay lets a
program in the box call a server you run locally, such as a model endpoint, a tool server or a
mock of a live API, without the box getting a network or any secret the server holds.

```python
# With a server of yours listening on 127.0.0.1:8000:
with (
    Sandbox.time_portal("2025-03-12").with_relay().build() as sandbox,
    sandbox.forward(8787, to=("127.0.0.1", 8000)),
):
    print(sandbox.command(["curl", "-s", "http://127.0.0.1:8787/"]).output().text)
```

`with_relay()` adds the relay to the box. `forward(port, to=...)` starts it listening on the
box's `127.0.0.1:<port>`, and returns once it listens. Each connection a program in the box opens
there is joined to a new TCP connection from your machine to `to`. The bytes travel through the
Epochal API, so the box needs no network. Any TCP protocol works, with up to 128 connections at
once. Leaving the `with` block, or `close()`, stops it; `close()` raises `TunnelError` if the
tunnel had already failed, for example because the box ended. An open tunnel is a running
command, so it keeps the box from going idle.

Bytes pass through as they are, so keep the box's view of time intact: a server that answers with
today's date, in a `Date` header or a payload, tells the box what it is.
[`examples/forward.py`](https://github.com/epochal-org/epochal/blob/main/python/sandbox/examples/forward.py)
is a runnable version: a local HTTP API, with a key that stays on your machine, called from a box
with no network.

## Running Claude Code or Codex in a box

**Your model key never leaves your machine except to your model endpoint.**
[`examples/agent.py`](https://github.com/epochal-org/epochal/blob/main/python/sandbox/examples/agent.py)
reads `EPOCHAL_LLM_API_KEY` locally and adds it only to the model calls it sends from your machine
to `EPOCHAL_LLM_BASE_URL`, the same two variables the CLI reads. Claude Code needs an Anthropic
Messages endpoint and gets the key as `x-api-key`; Codex and Grok need an OpenAI Responses
endpoint and get it as a bearer token. The box gets a placeholder key, and `epochal-mcp` starts
without either variable. Prompts and model replies do pass through the Epochal API, since that is
how the relay reaches the box; the key is not in them.

The example is a minimal Python version of the `epochal time-portal run` CLI. Its web tools come from
[`epochal-mcp`](https://github.com/epochal-org/epochal/tree/main/mcp), which you build from
the repo until release binaries are published:

```sh
git clone https://github.com/epochal-org/epochal && cd epochal
cargo install --path mcp          # needs Rust; puts epochal-mcp on your PATH
pip install epochal-sandbox
export EPOCHAL_API_KEY=... EPOCHAL_LLM_BASE_URL=https://api.anthropic.com EPOCHAL_LLM_API_KEY=...
python python/sandbox/examples/agent.py claude claude-sonnet-5-5 2025-03-12 "Find Nvidia's latest quarterly revenue"
```

It builds a box with the harness (`claude-code`, `codex` or `grok`) and the relay, and serves two of the
box's ports from your machine:

- `127.0.0.1:8787`: model calls (`/v1/messages`, `/v1/responses`), sent on to your endpoint with
  your key. Replies keep only `content-type`, and every `created`/`created_at` epoch becomes the
  box's instant.
- `127.0.0.1:8788`: `epochal-mcp`, run at the box's instant, whose `web_search` and
  `web_fetch` read the web as it stood, with your Epochal key, shown without dates.

The harness runs with its own web tools turned off, and its JSON events go to stdout. Both local
servers listen on loopback only; the model proxy answers only requests addressed to the box's port
and follows no redirect with your key. While a run lasts, other processes on your machine can
reach them, so run it on a machine you don't share.
