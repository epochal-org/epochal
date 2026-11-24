# epochal-sandbox

Run commands, move files and serve local ports in [Epochal](https://epochal.com) sandboxes: Linux
machines (*boxes*). Their one mode today is the Time Portal, which holds a box's clock and network
to a past instant, so nothing run in it can see what came after it.

Calls are made by [`epochal-api`](https://crates.io/crates/epochal-api), and retried here when a failure may pass on its own: every call is safe to repeat.

## Setup

```sh
cargo add epochal-sandbox
cargo add tokio --features macros,rt-multi-thread
export EPOCHAL_API_KEY=...   # from https://console.epochal.com/keys
```

Requires Rust 1.88 or later. `EPOCHAL_BASE_URL` overrides the API address (default
`https://api.epochal.com`); `SandboxBuilder::with_client` takes a client you built yourself.

## Example

```rust,no_run
use epochal_sandbox::chrono::{TimeZone, Utc};
use epochal_sandbox::{Network, Sandbox, Stream};

#[tokio::main]
async fn main() -> Result<(), epochal_sandbox::Error> {
    let as_of = Utc.with_ymd_and_hms(2025, 3, 12, 0, 0, 0).unwrap();
    let sandbox = Sandbox::time_portal(as_of)
        .with_network(Network::TimePortal)
        .with_vcpus(1)
        .with_memory_gib(2)
        .build()
        .await?;

    let out = sandbox.command(["date", "-u"]).output().await?;
    println!("{}", String::from_utf8_lossy(&out.stdout));

    sandbox.files().upload_dir("./src", "/home/epochal/src").await?;
    let run = sandbox
        .command(["python3", "-u", "analyze.py"])
        .with_cwd("/home/epochal/src")
        .start()
        .await?;
    while let Some(chunk) = run.next().await? {
        if chunk.stream == Stream::Stdout {
            print!("{}", String::from_utf8_lossy(&chunk.data));
        }
    }
    sandbox.files().download_dir("/home/epochal/out", "./out").await?;

    sandbox.terminate().await?;
    Ok(())
}
```

## What's in it

- **Boxes:** `Sandbox::time_portal(as_of)` sets up a time-portal box, and takes `with_network`,
  `with_vcpus`, `with_memory_gib`, `with_timeout`, `with_idle_timeout`, `with_env`, `with_label`,
  `with_app`, `with_relay`, `with_client` and `with_idempotency_key`. Every box boots the newest Ubuntu build dated on or
  before `as_of`, and its package mirrors are held to `as_of` too. An app is a current build, so a
  box that uses one is not strictly held to `as_of`. Unset, a box gets no network,
  1 vCPU, 2 GiB, a 1 h lifetime and a 10 min idle timeout. `build` waits until the box is live,
  and terminates one that fails to come up. Call `terminate` when done: a box left running is
  billed until its timeout.
- **Commands:** `sandbox.command(argv)` takes `with_cwd`, `with_env`, `with_timeout`, `with_stdin`,
  `with_open_stdin` and `with_idempotency_key`. `output` runs it to its end; `start` returns an
  `Exec` whose `next` yields each `ExecFrame` of output as it comes. A started command takes
  `write_stdin` and `close_stdin` (after `with_open_stdin`), `signal` and `kill`; each input is
  applied once, in order, even across retries. What a `write_stdin` whose outcome is unknown left
  unsent goes ahead of the next input; a piece the API refuses is dropped with the rest. Commands
  run as `root` in `/home/epochal`.
- **Files:** `files()` gives `stat`, `upload_file`, `download_file`, `upload_dir` and
  `download_dir`, which move a tree as one tar. A file moves in chunks, four in flight at once. Uploads carry no host mtimes or owners, and
  downloads never write outside the target directory.
- **Serving a box from here:** on a box built `with_relay()`, `serve_http(port, router)` answers
  each connection to the box's `127.0.0.1:<port>` with an [`axum`](https://crates.io/crates/axum)
  `Router` running on your machine, with no `Date` header, so the box isn't told today's date. The
  box needs no network for this. `Tunnel::stop` ends it.
- **Errors:** `Error` is `Api` (from the API crate), `NotLive` (the box failed before it was live),
  `CommandFailed`, `Io`, `Tunnel` or `Unexpected`.

`chrono` is re-exported, so `as_of` instants need no extra dependency.

License: Apache-2.0.
