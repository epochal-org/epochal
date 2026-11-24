# epochal-cli

`epochal` is the Epochal command line. Its one command today, `epochal time-portal`, runs an agent
in an Epochal sandbox held to an `--as-of`, and searches or reads the web as it stood at an instant.

```
this machine                                         box (clock = as-of, no agent binary)
 epochal ── model calls ──► EPOCHAL_LLM_BASE_URL (EPOCHAL_LLM_API_KEY)
    │  built-in harness: sample, run tool calls, repeat
    ├── bash / read / write / edit ──► sandbox API ──► bash in the box
    └── web_search / web_fetch ──► time portal at the as-of
```

By default `run` uses the built-in harness, which runs on this machine and uses the box as its
computer. The box holds no agent binary and the prompt is dated at the as-of, so nothing in the
agent's prompt, tools or box postdates it. Your model key never enters the box, and the web tools are `epochal-mcp`'s: the
agent sees no timestamps.

`--harness claude|codex|grok` instead installs Claude Code, Codex or Grok Build in the box. Each
is a current build, so its binary, version and built-in prompts postdate the as-of and can reveal
the present to the agent: such a run is not strictly held to `--as-of`. `run` says so and asks
before making the box; `--yes` skips the question, and without a terminal it is required.

## Install

```sh
cargo install epochal-cli     # or, from a clone of this repo: cargo install --path cli
export EPOCHAL_API_KEY=...    # from https://console.epochal.com/keys
export EPOCHAL_LLM_BASE_URL=https://api.anthropic.com EPOCHAL_LLM_API_KEY=...
```

This installs the `epochal` command. Building needs Rust 1.88 or later.

## Environment

| Variable | What it is |
|---|---|
| `EPOCHAL_API_KEY` | Your Epochal API key, for sandboxes and search/fetch |
| `EPOCHAL_BASE_URL` | The Epochal API; defaults to `https://api.epochal.com` |
| `EPOCHAL_LLM_BASE_URL` | The model endpoint, for the built-in harness and every `--harness`: e.g. `https://api.anthropic.com`, `https://api.openai.com/v1`, or a gateway |
| `EPOCHAL_LLM_API_KEY` | Its key, sent as `x-api-key` to an Anthropic Messages call and as `Authorization: Bearer` to an OpenAI-compatible one |
| `EPOCHAL_LLM_API` | The built-in harness's API: `anthropic` (Messages) or `openai` (Chat Completions); defaults to `anthropic` for a model whose id starts with `claude` or `anthropic` (`anthropic/claude-…`), `openai` otherwise |
| `RUST_LOG` | Log filter for stderr; defaults to `warn` |

Both `EPOCHAL_LLM_BASE_URL` and `EPOCHAL_LLM_API_KEY` are required; there is no default endpoint.
An Anthropic base URL is taken with or without its `/v1`; an OpenAI-compatible one as its SDK takes
it, so `https://openrouter.ai/api/v1` and `https://api.openai.com/v1` both work.

### Third-party harnesses

With `--harness`, the harness's model calls go to the same `EPOCHAL_LLM_BASE_URL` with the same
`EPOCHAL_LLM_API_KEY`. Both are read on this machine, checked before a box is made, and never sent
to the box, which gets a placeholder key. The harness's model calls and its `epochal` MCP tools are
served from this machine through the box's relay. Model responses keep only `content-type`, and
every `created`/`created_at` epoch becomes the as-of.

Each harness speaks its own wire format, so the endpoint must speak it too, and the key is sent the
way that format expects:

| Harness | Calls | Endpoint must speak | Key sent as |
|---|---|---|---|
| `claude` | `/v1/messages`, `/v1/messages/count_tokens` | Anthropic Messages | `x-api-key` |
| `codex` | `/v1/responses` | OpenAI Responses | `Authorization: Bearer` |
| `grok` | `/v1/responses` | OpenAI Responses | `Authorization: Bearer` |

`--model` names the model at the endpoint for every harness, but only the built-in harness picks
its API from it or from `EPOCHAL_LLM_API`.

## Usage

```sh
epochal time-portal run --model claude-sonnet-5-5 --as-of 2025-03-12 \
  "Find Nvidia's latest quarterly revenue"
EPOCHAL_LLM_BASE_URL=https://api.openai.com/v1 EPOCHAL_LLM_API_KEY=$OPENAI_API_KEY \
  epochal time-portal run --model gpt-6-sol \
  --as-of 2025-03-12 --upload ./data.csv --download /home/epochal/out:./out -o answer.txt \
  "Analyze data.csv into out/"
epochal time-portal run --harness claude --yes --model claude-sonnet-5-5 \
  --as-of 2025-03-12 "Find Nvidia's latest quarterly revenue"

epochal time-portal search "nvidia quarterly revenue" --as-of 2025-03-12
epochal time-portal fetch https://nvidianews.nvidia.com/ --as-of 2025-03-12 --format json
```

`--as-of` takes RFC 3339, an HTTP-date, or `YYYY-MM-DD`, which means 23:59:59 that UTC day.

Every `time-portal` subcommand writes its result in `--format text|json` (default `text`) to stdout, or to
`-o FILE`, which is written whole or not at all. `--trace FILE` also writes JSON Lines: each
agent event and web call of a run, or the API's whole answer for `search` and `fetch`.

A run's JSON holds its `answer` and every web call in `calls`, the same objects `--trace` writes:
the `tool`, its `request`, and the API's `response`, or `error` if the call failed.

```json
{"tool": "search", "request": {"query": "nvidia revenue"}, "response": {"as_of": "2025-03-12T23:59:59Z", "results": []}}
{"tool": "fetch", "request": {"url": "https://a.example"}, "error": "web_fetch failed: …"}
```

`run` takes box flags:

| Flag | Default | Takes |
| --- | --- | --- |
| `--internet` | off | The box's own processes (curl, pip, git) reach the web as it stood at the as-of, through the time portal and package mirrors held to it; each page read is billed. Off, nothing resolves. The agent's model and web tool calls go through this machine either way. |
| `--cpu` | `1` | Virtual CPUs, at least 1. |
| `--memory` | `2Gi` | Whole GiB in binary units: `4Gi`, `4GiB`, `4096MiB`. Decimal units (`4GB`) and fractions (`1.5Gi`) are refused, since the box is reserved in whole GiB. |
| `--timeout` | `1h` | Box lifetime as a duration of whole seconds: `90s`, `10m`, `1h30m`. A bare number has no unit and is refused. |
| `--upload LOCAL[:BOX_PATH]` | | Repeatable; `BOX_PATH` defaults to `--cwd/<name>`. A `LOCAL` that exists whole is taken as is; otherwise it splits at the last `:`, so `BOX_PATH` holds none. |
| `--download BOX_PATH[:LOCAL]` | | Repeatable; copied out after the run, `LOCAL` defaulting to the path's last part, here. Split at the first `:`, so `BOX_PATH` holds none. |
| `--cwd` | `/home/epochal` | The box's home, where the agent works. |
| `--harness` | built-in | `claude`, `codex` or `grok`, installed in the box; not strictly held to `--as-of`. |
| `--yes` | | Run a `--harness` without asking; required without a terminal. |
| `--small-model` | `--model` | Model for `--harness claude`'s background calls; refused otherwise. |
| `--max-turns` | | Cap on model turns; not for `codex`. The built-in harness's last turn must answer, so `1` means no tool calls. |
| `--keep` | | Leave the box running afterwards. |

Progress goes to stderr: the box's setup, then each tool call with its result. Ctrl-C, SIGTERM or
SIGHUP stops the run and removes the box; a second one exits at once and leaves the box running.

License: Apache-2.0.
