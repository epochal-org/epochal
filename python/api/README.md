# epochal-api

Python client for the [Epochal](https://epochal.com) API. Its one module today is the Time Portal:
web search, web fetch and sandboxes held to a past instant, so nothing published or crawled after
it is used.

```sh
pip install epochal-api
export EPOCHAL_API_KEY=...
```

Requires Python 3.11.4 or later. Create an API key at https://console.epochal.com/keys;
`EPOCHAL_BASE_URL` overrides the API address (default `https://api.epochal.com`). API docs:
https://docs.epochal.com.

```python
from epochal.api import Client
from epochal.api.time_portal import PageStatus

with Client() as client:
    portal = client.time_portal("2025-03-12")
    found = portal.search("fed rate decision")
    for hit in found.results:
        print(hit.title, hit.url)
        if hit.entity:
            print(" ", hit.entity.type, hit.entity.typed)

    page = portal.fetch("https://example.com/news")
    print(page.content if page.status is PageStatus.SUCCESS else page.page_error)
```

`AsyncClient` has the same methods as coroutines:

```python
import asyncio

from epochal.api import AsyncClient


async def main() -> None:
    async with AsyncClient() as client:
        found = await client.time_portal("2025-03-12").search("fed rate decision")
        print(len(found.results))


asyncio.run(main())
```

- **Modules:** `Client` holds the key, connections and retries; each API module is a handle from
  it. `client.time_portal(as_of)` is a `TimePortal` held to `as_of` (`AsyncTimePortal` on an
  `AsyncClient`), and its types are in `epochal.api.time_portal`. Search, fetch, page and
  `create_sandbox` are held to the portal's `as_of`; the other calls name a box by id, and it keeps
  its own instant.
- **Methods** of `TimePortal`: `search`, `fetch`, `page` (the archived page itself; a page that could not be read
  is returned with `status` `PageStatus.ERROR` and the site's reason in `page_error`, not raised),
  and the sandbox calls
  `create_sandbox`, `get_sandbox`, `list_sandboxes`, `terminate_sandbox`, `start_exec`,
  `read_exec_output`, `send_exec_input`, `stat_file`, `list_files`, `read_file`, `write_file`,
  `remove_file`.
- **Entities:** a `search` result that is a `https://world.epochal.com/` page carries its entity in
  `results[i].entity` (`None` on every other result), and `fetch` returns such a page's entity in
  `entity`. `SearchResponse.entities` is reserved; entities come on `results[i].entity`.
  `entity.properties` is the JSON as sent; `entity.typed` reads it into one class per public type
  (organization, person, place, crypto, weather) from `epochal.api.entities`, or `UnknownProperties`
  with the JSON for a type, or a `schema_version` of one, this version doesn't model. Their enums
  are open: a new value reads as the `str` sent.
- **Instants** are an aware `datetime`, an ISO 8601 string, or a day (a `date` or
  `"YYYY-MM-DD"`). As an `as_of`, a day means its last second, 23:59:59 UTC, as the API reads it.
  A datetime without a time zone raises `InvalidArgumentError` from `time_portal`, before any
  request.
- **Retries:** a dropped connection, a timeout, or a 408, 429, 500, 502, 503, 504 or 520-524 is
  retried up to `max_retries` times (default 5), with jittered exponential backoff that honors
  `Retry-After`, sleeping at most 60 s in all. Search, fetch and page are billed and can't be
  deduplicated, so they are retried only on 408, 429 and 503, when the server did nothing. Creates
  and command starts carry an `Idempotency-Key`, so a retry never makes a second sandbox or runs a
  command twice.
- **Newer servers:** `categories` takes any name, and `search` and `fetch` take `extra_body=` for
  fields this version doesn't model, sent as given, so a new search or fetch option needs no new
  release.
- **Connections** are pooled per client: share one client and close it when done. Pass
  `http_client=` to bring your own `httpx` client (proxies, custom transports).
- **Errors** all derive from `EpochalError`: `APIError` (with `status`, `kind`, `request_id`,
  `retry_after`), `TransportError`, `DecodeError`, `InvalidArgumentError` and
  `MissingApiKeyError`. Each says whether it is `retryable`.

To run commands and move files in sandboxes, use
[`epochal-sandbox`](https://pypi.org/project/epochal-sandbox/), which is
built on this package.
