# epochal-api

Rust client for the [Epochal](https://epochal.com) API. Its one module today is the Time Portal:
web search, web fetch and sandboxes held to a past instant, so nothing published or crawled after
it is used.

Each method is one API call. Waiting, polling and retries are left to the caller; for sandboxes
that do all three, use [`epochal-sandbox`](https://crates.io/crates/epochal-sandbox).

## Setup

```sh
cargo add epochal-api
cargo add tokio --features macros,rt-multi-thread
export EPOCHAL_API_KEY=...   # from https://console.epochal.com/keys
```

Requires Rust 1.88 or later. The client reads `EPOCHAL_API_KEY`, and `EPOCHAL_BASE_URL` if set
(default `https://api.epochal.com`); `Client::builder().with_api_key(...)` and `.with_base_url(...)`
override them.

## Example

```rust,no_run
use epochal_api::chrono::{TimeZone, Utc};
use epochal_api::time_portal::web::{FetchRequest, PageStatus, SearchRequest};
use epochal_api::Client;

#[tokio::main]
async fn main() -> Result<(), epochal_api::Error> {
    let as_of = Utc.with_ymd_and_hms(2025, 3, 12, 0, 0, 0).unwrap();
    let portal = Client::builder().build()?.time_portal(as_of);

    let found = portal.search(&SearchRequest::new("fed rate decision")).await?;
    for hit in &found.results {
        println!("{} {}", hit.title, hit.url);
        if let Some(entity) = &hit.entity {
            println!("  {}: {:?}", entity.kind, entity.typed());
        }
    }

    let mut read = FetchRequest::new("https://www.federalreserve.gov/");
    read.max_characters = Some(2_000);
    let page = portal.fetch(&read).await?;
    match page.status {
        PageStatus::Success => println!("{}", page.content),
        _ => println!("not read: {:?}", page.page_error),
    }
    Ok(())
}
```

## What's in it

`Client` holds the key and the connection pool; each API module is a handle from it.
`client.time_portal(as_of)` returns a `TimePortal` held to `as_of`, whose methods are the time
portal's calls:

- **Web:** `search`, `fetch` (a page as markdown, with its metadata) and `page` (the archived
  document itself), each held to the portal's `as_of`. A page that could not be read is `Ok`, with
  `status` `PageStatus::Error` and the site's reason in `page_error`. Request and response types
  are in `time_portal::web`.
- **Entities:** a `search` result that is a `https://world.epochal.com/` page carries its entity in
  `results[i].entity` (`None` on every other result), and `fetch` returns such a page's entity in
  `entity`. `SearchResponse.entities` is reserved; entities come on `results[i].entity`.
  `entity.properties` is the JSON as sent; `entity.typed()` reads it into a `TypedEntity`: one model
  per public type (organization, person, place, crypto, weather) from `time_portal::entities`, or
  `TypedEntity::Unknown` with the JSON for a type, or a `schema_version` of one, this version
  doesn't model. Their enums are open: a new value is `Other(value)`.
- **Sandboxes:** `create_sandbox`, `get_sandbox`, `list_sandboxes`, `terminate_sandbox`,
  `start_exec`, `read_exec_output`, `send_exec_input`, `stat_file`, `list_files`, `read_file`,
  `write_file` and `remove_file`. Types are in `time_portal::sandbox`. `create_sandbox` makes a box
  held to the portal's `as_of`; the other calls name a box by id, and it keeps its own instant. A
  call that waits, such as `get_sandbox` with a `wait`, is held for at most `MAX_WAIT` (55 s).
  `create_sandbox` and `start_exec` send an `Idempotency-Key`: the request's own, or else a fresh
  `idempotency_key()`. To retry one without making a second box or running the command twice, set
  the key once and send it with every attempt.
- **Errors:** every call returns `Error`: `MissingApiKey`, `InvalidArgument` (refused before any
  request), `Transport`, `Api` (with `status`, `kind`, `message`, `request_id`, `retry_after`) and
  `Decode`.
- **Newer servers:** a search category this version doesn't name is `Category::Other(name)`, and
  `SearchRequest` and `FetchRequest` have an `extra` map of fields sent as given, so a new search
  or fetch option needs no new release.
- **Connections:** `Client` and `TimePortal` are cheap to clone, and clones share one connection
  pool. Redirects are never followed, so the key never leaves for another host.

`chrono` is re-exported, so `as_of` instants need no extra dependency.

License: Apache-2.0.
