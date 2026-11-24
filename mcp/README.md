# epochal-mcp

An [MCP](https://modelcontextprotocol.io) server that gives an agent [Epochal](https://epochal.com)'s
`web_search` and `web_fetch`. Its one mode today is the Time Portal, which holds both to one past
instant. The agent picks
neither the instant nor the API key, and never sees anything published or crawled after it:
answers reach it without timestamps.

It is a command, `epochal-mcp`, and a library for serving the same tools from your own Rust
program.

## Setup

```sh
cargo install epochal-mcp   # or, from a clone of this repo: cargo install --path mcp
export EPOCHAL_API_KEY=...  # from https://console.epochal.com/keys
```

Requires Rust 1.88 or later. `EPOCHAL_BASE_URL` overrides the API address (default
`https://api.epochal.com`), and `RUST_LOG` sets the log filter for stderr.

## Running it

```sh
# Over stdio, for a client that starts the server itself:
epochal-mcp --time-portal 2025-03-12

# Over streamable HTTP at http://127.0.0.1:8788/mcp:
epochal-mcp --time-portal 2025-03-12T12:00:00Z --serve http:8788
```

- `--time-portal` is required: the time portal is the server's one mode. It takes RFC 3339, an HTTP-date, or `YYYY-MM-DD`, which means 23:59:59 that UTC day.
- `--serve http:PORT` listens on 127.0.0.1 only; `http:HOST:PORT` listens on `HOST`. The endpoint
  answers only loopback `Host` names, which keeps web pages from reaching it by DNS rebinding; add
  others with `--allow-host`.
- Responses carry no `Date` header, so a client held to the past isn't told today's date.

To use it from Claude Code, which starts the server itself:

```sh
claude mcp add epochal -e EPOCHAL_API_KEY="$EPOCHAL_API_KEY" -- epochal-mcp --time-portal 2025-03-12
```

## Tools

| Tool | Arguments | Returns |
|---|---|---|
| `web_search` | `query`; `num_results` (1-10, default 10); `domains` | Each result's title, URL and snippet |
| `web_fetch` | `url`; `max_characters` (1-100,000, default 20,000) | The page's title, URL and content as text, at most 100,000 characters |

## As a library

```sh
cargo add epochal-mcp
cargo add tokio --features macros,rt-multi-thread
```

```rust,no_run
use epochal_mcp::{Tools, parse_as_of};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let as_of = parse_as_of("2025-03-12")?;
    let tools = Tools::time_portal(as_of)
        .with_observer(|call| eprintln!("{call:?}"))
        .build()?;
    tools.serve_stdio().await?;
    Ok(())
}
```

`Tools::serve_http(listener)` serves streamable HTTP at `/mcp` on a `TcpListener`, and
`Tools::http_router()` is the same endpoint as an [`axum`](https://crates.io/crates/axum) `Router`
to mount in your own app. `with_observer` sees each web call with its full answer, before the
agent sees it without dates. A `ToolCall` serializes as its `tool`, its `request`, and either its
`response` or, if the call failed, its `error`:

```json
{"tool": "search", "request": {"query": "q"}, "error": "web_search failed: No key matches that token."}
```

License: Apache-2.0.
