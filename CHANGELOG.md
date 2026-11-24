# Changelog

Every package in this repository shares one version. Each release lists what changed for users,
grouped by package, breaking changes first.

## v0.1.0

First release of the Epochal SDKs. Their one module is the **Time Portal**: web search, web fetch
and Linux sandboxes held to a past instant, so nothing published or crawled after it is seen.

### Rust

- `epochal-api`: client for the Epochal API, one method per call: Time Portal web search and
  fetch, typed entities, and the raw sandbox calls.
- `epochal-sandbox`: build Time Portal sandboxes, run commands, move files and serve their ports
  from your machine, with retries on transient failures.
- `epochal-mcp`: MCP server, over stdio or streamable HTTP, giving an agent `web_search` and
  `web_fetch` at a fixed instant; also a library for serving the same tools from your program.
- `epochal-cli`: the `epochal` command. `epochal time-portal run` runs an agent in a sandbox
  with its built-in harness or Claude Code, Codex or Grok Build; `search` and `fetch` query the
  web from the shell.

### Python

- `epochal-api`: the API client, sync and async.
- `epochal-sandbox`: sandboxes, sync and async, with examples for running an agent and forwarding
  a port.
