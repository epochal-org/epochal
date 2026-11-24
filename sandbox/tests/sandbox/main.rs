//! The client through its public API against an in-process fake of the sandbox API.

mod fake;
mod relay;

use std::fs;
use std::net::TcpListener as StdTcpListener;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::routing::{get, post};
use epochal_api::Error as ApiError;
use epochal_sandbox::chrono::{DateTime, Utc};
use epochal_sandbox::{
    Client, Error, ExecExit, FileType, Network, Sandbox, SandboxBuilder, Stream,
};
use fake::{FRAME, Fake, KEY, NO_BOOT, RANGE_CAP, RELAY};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const AS_OF: &str = "2025-03-01T12:00:00Z";

fn client(url: &str) -> Client {
    Client::builder()
        .with_api_key(KEY)
        .with_base_url(format!("{url}/"))
        .build()
        .unwrap()
}

fn instant(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

fn builder(fake: &Fake) -> SandboxBuilder {
    Sandbox::time_portal(instant(AS_OF)).with_client(client(&fake.url))
}

async fn live(fake: &Fake) -> Sandbox {
    builder(fake).build().await.unwrap()
}

fn is_api(err: &Error, status: u16, want: &str) -> bool {
    matches!(err, Error::Api(ApiError::Api { status: s, kind, .. }) if *s == status && kind == want)
}

#[tokio::test]
async fn a_box_is_created_as_asked_reported_live_and_terminated() {
    let fake = Fake::start().await;
    let sb = builder(&fake)
        .with_app("claude-code")
        .with_relay()
        .with_network(Network::TimePortal)
        .with_vcpus(4)
        .with_memory_gib(8)
        .with_timeout(Duration::from_secs(5400))
        .with_idle_timeout(Duration::from_secs(600))
        .with_label("job", "nightly")
        .with_idempotency_key("job-42")
        .build()
        .await
        .unwrap();

    let log = fake.log();
    let create = &log[0];
    assert_eq!(
        (create.method.as_str(), create.path.as_str()),
        ("POST", "/v1/time-portal/sandbox")
    );
    assert_eq!(create.headers["accept-datetime"], AS_OF);
    assert_eq!(create.headers["x-api-key"], KEY);
    assert_eq!(create.headers["idempotency-key"], "job-42");
    let body: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(
        body,
        json!({
            "apps": ["claude-code", "relay"],
            "metadata": {"job": "nightly"},
            "network": "time-portal",
            "vcpus": 4,
            "memory_gib": 8,
            "timeout": "90m",
            "idle_timeout": "10m",
        })
    );
    assert_eq!(sb.as_of(), instant(AS_OF));
    assert_eq!((sb.vcpus(), sb.memory_gib()), (4, 8));
    assert_eq!(sb.timeout(), Duration::from_secs(5400));
    assert_eq!(sb.network(), Network::TimePortal);
    assert_eq!(sb.apps().collect::<Vec<_>>(), ["claude-code", "relay"]);
    assert_eq!(fake.box_state(sb.id()), "live");

    sb.terminate().await.unwrap();
    assert_eq!(fake.box_state(sb.id()), "ended");
    let last = fake.log().pop().unwrap();
    assert_eq!(last.method, "DELETE");
    assert_eq!(last.path, format!("/v1/time-portal/sandbox/{}", sb.id()));
}

#[tokio::test]
async fn an_empty_create_sends_no_fields_and_takes_the_servers_defaults() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    assert_eq!(fake.log()[0].body.as_ref(), b"{}");
    let key = fake.log()[0].headers["idempotency-key"].clone();
    assert_eq!(
        key.len(),
        32,
        "a key of its own covers the build's retries: {key:?}"
    );
    assert_eq!(sb.network(), Network::None);
    assert_eq!(sb.apps().count(), 0);
}

#[tokio::test]
async fn lost_replies_are_retried_without_a_second_box_or_a_second_run() {
    let fake = Fake::start().await;
    fake.lose_replies(1);
    let sb = live(&fake).await;
    assert_eq!(sb.id(), "sbx_1");
    fake.lose_replies(1);
    let out = sb
        .command(["sh", "-c", "echo ran >> runs; cat runs"])
        .with_cwd("/work")
        .output()
        .await
        .unwrap();
    assert_eq!(out.stdout, b"ran\n", "the command ran once");
    let local = tempfile::tempdir().unwrap();
    let sent = payload(3 * 4096 + 5);
    fs::write(local.path().join("in.bin"), &sent).unwrap();
    fake.lose_replies(2);
    sb.files()
        .upload_file(local.path().join("in.bin"), "/work/copy.bin")
        .await
        .unwrap();
    fake.lose_replies(2);
    sb.files()
        .download_file("/work/copy.bin", local.path().join("back.bin"))
        .await
        .unwrap();
    assert_eq!(fs::read(local.path().join("back.bin")).unwrap(), sent);
    let creates = fake
        .log()
        .iter()
        .filter(|s| s.method == "POST" && s.path.ends_with("/sandbox"))
        .count();
    assert_eq!(creates, 2, "the lost create was sent again under its key");
}

#[tokio::test]
async fn a_box_that_never_boots_is_an_error_naming_why() {
    let fake = Fake::start().await;
    let err = Sandbox::time_portal(instant(NO_BOOT))
        .with_client(client(&fake.url))
        .build()
        .await
        .unwrap_err();
    match err {
        Error::NotLive { id, reason } => {
            assert_eq!(id, "sbx_1");
            assert_eq!(reason.as_deref(), Some("The host went away."));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fake.log().last().unwrap().method,
        "DELETE",
        "the dead box is terminated"
    );
}

#[tokio::test]
async fn a_timeout_the_api_cannot_express_is_refused_before_any_request() {
    let fake = Fake::start().await;
    let err = builder(&fake)
        .with_timeout(Duration::from_millis(1500))
        .build()
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Api(ApiError::InvalidArgument(_))),
        "{err:?}"
    );
    assert!(fake.log().is_empty());
}

#[tokio::test]
async fn a_command_streams_both_outputs_in_order_and_reports_its_exit_code() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    fs::write(fake.root(sb.id()).join("work/in.txt"), "from the box\n").unwrap();
    let script = r#"printf 'out-1 %s\n' "$GREETING"; printf 'err-1\n' >&2; cat in.txt; printf 'err-2\n' >&2; pwd; exit 7"#;
    let exec = sb
        .command(["sh", "-c", script])
        .with_cwd("/work")
        .with_env("GREETING", "hello there")
        .with_timeout(Duration::from_secs(60))
        .start()
        .await
        .unwrap();
    assert_eq!(exec.exit(), None);
    let (mut stdout, mut stderr, mut frames) = (Vec::new(), Vec::new(), 0);
    while let Some(frame) = exec.next().await.unwrap() {
        frames += 1;
        assert!(frame.data.len() <= FRAME, "{frame:?}");
        let (into, stream) = match frame.stream {
            Stream::Stdout => (&mut stdout, Stream::Stdout),
            _ => (&mut stderr, Stream::Stderr),
        };
        assert_eq!(frame.offset, into.len() as u64, "{stream:?} offsets run on");
        into.extend(frame.data);
    }
    let root = fake.root(sb.id());
    let expected = format!("out-1 hello there\nfrom the box\n{}/work\n", root.display());
    assert_eq!(String::from_utf8(stdout).unwrap(), expected);
    assert_eq!(stderr, b"err-1\nerr-2\n");
    assert!(frames > 3, "the fake split the output, {frames} frames");
    assert_eq!(exec.exit(), Some(ExecExit::Exited { code: 7 }));
    assert_eq!(exec.next().await.unwrap(), None);

    let body: Value = serde_json::from_slice(&fake.log()[2].body).unwrap();
    assert_eq!(body["env"], json!({"GREETING": "hello there"}));
    assert_eq!(body["timeout"], "1m");
}

#[tokio::test]
async fn output_collects_a_whole_command() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let out = sb
        .command(["sh", "-c", "echo yes; echo no >&2"])
        .output()
        .await
        .unwrap();
    assert!(out.success());
    assert_eq!(
        (out.stdout.as_slice(), out.stderr.as_slice()),
        (&b"yes\n"[..], &b"no\n"[..])
    );
    let failed = sb.command(["false"]).output().await.unwrap();
    assert!(!failed.success());
    assert_eq!(failed.exit, ExecExit::Exited { code: 1 });
}

#[tokio::test]
async fn stdin_written_in_pieces_and_closed_reaches_the_command_whole() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let exec = sb
        .command(["cat"])
        .with_stdin("start ")
        .with_open_stdin()
        .start()
        .await
        .unwrap();
    for part in [&b"one "[..], b"two"] {
        exec.write_stdin(part).await.unwrap();
    }
    exec.close_stdin().await.unwrap();
    assert_eq!(exec.output().await.unwrap().stdout, b"start one two");

    let big = 2 * (1 << 20) + 5;
    let wc = sb
        .command(["wc", "-c"])
        .with_open_stdin()
        .start()
        .await
        .unwrap();
    let before = fake.log().len();
    wc.write_stdin(&vec![b'x'; big]).await.unwrap();
    wc.close_stdin().await.unwrap();
    let out = wc.output().await.unwrap();
    assert_eq!(
        String::from_utf8(out.stdout).unwrap().trim(),
        big.to_string()
    );
    let inputs = fake.log()[before..]
        .iter()
        .filter(|s| s.path.ends_with("/input"))
        .count();
    assert_eq!(inputs, 4, "three pieces of at most 1 MiB, then the close");
}

#[tokio::test]
async fn a_write_whose_reply_was_lost_is_sent_once_before_the_next_input() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let exec = sb.command(["cat"]).with_open_stdin().start().await.unwrap();
    fake.lose_replies(1);
    exec.write_stdin(b"one ").await.unwrap();
    exec.write_stdin(b"two").await.unwrap();
    exec.close_stdin().await.unwrap();
    assert_eq!(exec.output().await.unwrap().stdout, b"one two");
}

#[tokio::test]
async fn a_signal_ends_a_command_and_kill_waits_for_it() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let exec = sb.command(["sleep", "30"]).start().await.unwrap();
    exec.signal(15).await.unwrap();
    assert_eq!(
        exec.output().await.unwrap().exit,
        ExecExit::Signaled { signal: 15 }
    );

    let other = sb.command(["sleep", "30"]).start().await.unwrap();
    other.kill().await.unwrap();
    assert_eq!(other.exit(), Some(ExecExit::Signaled { signal: 9 }));
    other.kill().await.unwrap();
}

#[tokio::test]
async fn kill_kills_even_when_stdin_left_over_from_a_cancelled_write_is_refused() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    // The command closes its own stdin, so the leftover write is refused as a conflict.
    let exec = sb
        .command(["sh", "-c", "exec 0<&- sleep 30"])
        .with_open_stdin()
        .start()
        .await
        .unwrap();
    fake.delay_inputs(Duration::from_secs(5));
    let cancelled = tokio::time::timeout(Duration::from_millis(200), exec.write_stdin(b"x")).await;
    assert!(cancelled.is_err(), "the write was still in flight");
    fake.delay_inputs(Duration::ZERO);
    tokio::time::sleep(Duration::from_millis(200)).await;

    exec.kill().await.unwrap();

    assert_eq!(exec.exit(), Some(ExecExit::Signaled { signal: 9 }));
}

#[tokio::test]
async fn a_command_on_an_ended_box_is_the_apis_conflict() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    sb.terminate().await.unwrap();
    let err = sb.command(["true"]).start().await.unwrap_err();
    assert!(is_api(&err, 409, "conflict"), "{err:?}");
}

/// Bytes no chunk boundary lines up with, so ranged reads and chunked writes are exercised.
fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 251) as u8).collect()
}

#[tokio::test]
async fn a_file_goes_up_and_comes_back_byte_for_byte() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let local = tempfile::tempdir().unwrap();
    let data = payload(3 * RANGE_CAP as usize + 123);
    fs::write(local.path().join("up.bin"), &data).unwrap();

    sb.files()
        .upload_file(local.path().join("up.bin"), "/work/deep/f.bin")
        .await
        .unwrap();
    assert_eq!(
        fs::read(fake.root(sb.id()).join("work/deep/f.bin")).unwrap(),
        data
    );
    let stat = sb.files().stat("/work/deep/f.bin").await.unwrap();
    assert_eq!(
        (stat.file_type, stat.size),
        (FileType::File, data.len() as u64)
    );
    assert_eq!(
        sb.files().stat("/work/deep").await.unwrap().file_type,
        FileType::Directory
    );

    sb.files()
        .download_file("/work/deep/f.bin", local.path().join("down.bin"))
        .await
        .unwrap();
    assert_eq!(fs::read(local.path().join("down.bin")).unwrap(), data);
    let ranged = fake
        .log()
        .iter()
        .filter(|s| s.method == "GET" && s.path.contains("/files?"))
        .count();
    assert_eq!(ranged, 4, "one ranged read per {RANGE_CAP} bytes");

    fs::write(local.path().join("empty"), "").unwrap();
    sb.files()
        .upload_file(local.path().join("empty"), "/work/empty")
        .await
        .unwrap();
    sb.files()
        .download_file("/work/empty", local.path().join("empty2"))
        .await
        .unwrap();
    assert_eq!(fs::read(local.path().join("empty2")).unwrap(), b"");
}

#[tokio::test]
async fn a_large_file_overlaps_its_ranges_and_lands_in_order() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let data = payload(13 * RANGE_CAP as usize + 5);
    fs::write(fake.root(sb.id()).join("work/big.bin"), &data).unwrap();
    let delay = Duration::from_millis(200);
    fake.delay_files(delay);

    let local = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    sb.files()
        .download_file("/work/big.bin", local.path().join("big.bin"))
        .await
        .unwrap();
    let took = started.elapsed();

    assert_eq!(fs::read(local.path().join("big.bin")).unwrap(), data);
    assert_eq!(fake.peak_file_calls(), 4);
    let serial = delay * 14;
    assert!(took < serial / 2, "{took:?}, serially {serial:?}");
}

#[tokio::test]
async fn a_large_upload_sends_its_chunks_after_the_first_together() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let data = payload(4 * (8 << 20) + 1);
    let local = tempfile::tempdir().unwrap();
    fs::write(local.path().join("big.bin"), &data).unwrap();
    fake.delay_files(Duration::from_millis(200));

    sb.files()
        .upload_file(local.path().join("big.bin"), "/work/big.bin")
        .await
        .unwrap();

    assert_eq!(fake.peak_file_calls(), 4);
    assert!(fs::read(fake.root(sb.id()).join("work/big.bin")).unwrap() == data);
}

#[tokio::test]
async fn a_slow_upload_chunk_does_not_hold_back_the_chunks_after_it() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    const CHUNK: usize = 8 << 20;
    let data = payload(6 * CHUNK);
    let local = tempfile::tempdir().unwrap();
    fs::write(local.path().join("big.bin"), &data).unwrap();
    fake.hold_write(CHUNK as u64, 5 * CHUNK as u64);

    sb.files()
        .upload_file(local.path().join("big.bin"), "/work/big.bin")
        .await
        .unwrap();

    assert_eq!(
        fake.passed_held_write(),
        Some(true),
        "the last chunk started while the second was held"
    );
    assert!(fs::read(fake.root(sb.id()).join("work/big.bin")).unwrap() == data);
}

#[tokio::test]
async fn short_ranges_that_name_a_larger_file_are_read_to_its_end() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let data = payload(5 * RANGE_CAP as usize + 3);
    fs::write(fake.root(sb.id()).join("work/f.bin"), &data).unwrap();
    fake.short_ranges(1000);

    let local = tempfile::tempdir().unwrap();
    sb.files()
        .download_file("/work/f.bin", local.path().join("f.bin"))
        .await
        .unwrap();
    assert_eq!(fs::read(local.path().join("f.bin")).unwrap(), data);
}

#[tokio::test]
async fn a_missing_path_is_not_found_and_a_relative_one_is_refused() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let err = sb.files().stat("/work/nope").await.unwrap_err();
    assert!(is_api(&err, 404, "not_found"), "{err:?}");
    let err = sb.files().stat("work").await.unwrap_err();
    assert!(
        matches!(err, Error::Api(ApiError::InvalidArgument(_))),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_directory_goes_up_and_comes_back_whole() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let local = tempfile::tempdir().unwrap();
    let src = local.path().join("src");
    fs::create_dir_all(src.join("a/b")).unwrap();
    fs::write(src.join("top.txt"), "top").unwrap();
    fs::write(src.join("a/b/run.sh"), "#!/bin/sh\n").unwrap();
    fs::set_permissions(src.join("a/b/run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(src.join("a/big.bin"), payload(2 * RANGE_CAP as usize + 5)).unwrap();
    symlink("b/run.sh", src.join("a/link")).unwrap();

    sb.files().upload_dir(&src, "/work/proj").await.unwrap();
    let boxed = fake.root(sb.id()).join("work/proj");
    assert_eq!(fs::read_to_string(boxed.join("top.txt")).unwrap(), "top");
    assert_eq!(
        fs::read_link(boxed.join("a/link")).unwrap().to_str(),
        Some("b/run.sh")
    );
    let leftovers: Vec<_> = fs::read_dir(fake.root(sb.id()).join("tmp"))
        .unwrap()
        .collect();
    assert!(leftovers.is_empty(), "the upload's tar is removed");

    let outside = boxed.join("escape");
    symlink("../../../etc", &outside).unwrap();
    let dest = local.path().join("back");
    sb.files().download_dir("/work/proj", &dest).await.unwrap();
    assert_eq!(fs::read_to_string(dest.join("top.txt")).unwrap(), "top");
    assert_eq!(
        fs::read(dest.join("a/big.bin")).unwrap(),
        payload(2 * RANGE_CAP as usize + 5)
    );
    let mode = fs::metadata(dest.join("a/b/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
    assert_eq!(
        fs::read_to_string(dest.join("a/link")).unwrap(),
        "#!/bin/sh\n"
    );
    assert!(
        fs::symlink_metadata(dest.join("escape")).is_err(),
        "a link out of the tree is skipped"
    );
    let leftovers: Vec<_> = fs::read_dir(fake.root(sb.id()).join("tmp"))
        .unwrap()
        .collect();
    assert!(leftovers.is_empty(), "the download's tar is removed");
}

#[tokio::test]
async fn downloading_a_missing_directory_says_what_failed() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let local = tempfile::tempdir().unwrap();
    let err = sb
        .files()
        .download_dir("/work/none", local.path().join("x"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::CommandFailed { stderr, .. } if stderr.contains("none")),
        "{err:?}"
    );
    let leftovers: Vec<_> = fs::read_dir(fake.root(sb.id()).join("tmp"))
        .unwrap()
        .collect();
    assert!(
        leftovers.is_empty(),
        "the failed archive is removed: {leftovers:?}"
    );
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    StdTcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// One HTTP/1.1 request to `port`, as a box process would make it; the raw response. A stalled
/// tunnel fails the test instead of hanging it.
async fn http(port: u16, request: &str) -> String {
    let exchange = async {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        socket.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    };
    tokio::time::timeout(Duration::from_secs(10), exchange)
        .await
        .expect("the tunnel answered within 10 s")
}

fn router() -> Router {
    Router::new()
        .route("/hello", get(|| async { "hello from the owner" }))
        .route("/echo", post(|body: Bytes| async move { body }))
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                "slow but whole"
            }),
        )
}

#[tokio::test]
async fn serve_http_answers_the_boxs_requests_from_here_until_stopped() {
    let fake = Fake::start().await;
    let sb = builder(&fake)
        .with_relay()
        .with_timeout(Duration::from_secs(1800))
        .build()
        .await
        .unwrap();
    let port = free_port();
    let tunnel = sb.serve_http(port, router()).await.unwrap();

    let hello = http(
        port,
        "GET /hello HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(hello.starts_with("HTTP/1.1 200"), "{hello}");
    assert!(hello.ends_with("hello from the owner"), "{hello}");
    assert!(
        !hello.to_ascii_lowercase().contains("\ndate:"),
        "no Date header: {hello}"
    );

    let big = "x".repeat(300 << 10);
    let posts = (0..4).map(|i| {
        let body = format!("{i}{big}");
        async move {
            let request = format!(
                "POST /echo HTTP/1.1\r\nhost: x\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            (body, http(port, &request).await)
        }
    });
    for (body, response) in spawn_all(posts).await {
        assert!(
            response.ends_with(&body),
            "an echo of {} bytes came back whole",
            body.len()
        );
    }

    let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    socket
        .write_all(b"GET /slow HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    socket.shutdown().await.unwrap();
    let mut slow = String::new();
    socket.read_to_string(&mut slow).await.unwrap();
    assert!(
        slow.ends_with("slow but whole"),
        "a client done writing still gets the reply: {slow:?}"
    );

    let missing = http(
        port,
        "GET /nope HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");

    tunnel.stop().await.unwrap();
    let relays: Vec<_> = fake
        .execs(sb.id())
        .into_iter()
        .filter(|(argv, _)| argv[0] == RELAY)
        .collect();
    assert_eq!(relays.len(), 1);
    assert_eq!(relays[0].0, [RELAY.to_owned(), port.to_string()]);
    assert_eq!(
        relays[0].1,
        Some(json!({"reason": "signaled", "signal": 9}))
    );
    assert!(
        TcpListener::bind(("127.0.0.1", port)).await.is_ok(),
        "the port is free again"
    );

    let relay_exec = fake
        .log()
        .into_iter()
        .find(|s| s.path.ends_with("/execs"))
        .unwrap();
    let body: Value = serde_json::from_slice(&relay_exec.body).unwrap();
    assert_eq!(
        (body["timeout"].as_str(), body["stdin_open"].as_bool()),
        (Some("30m"), Some(true))
    );
}

#[tokio::test]
async fn serve_http_needs_the_relay_app() {
    let fake = Fake::start().await;
    let sb = live(&fake).await;
    let err = sb.serve_http(free_port(), router()).await.unwrap_err();
    assert!(
        matches!(err, Error::Api(ApiError::InvalidArgument(_))),
        "{err:?}"
    );
    assert!(fake.execs(sb.id()).is_empty());
}

async fn spawn_all<F>(futures: impl IntoIterator<Item = F>) -> Vec<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handles: Vec<_> = futures.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.unwrap());
    }
    out
}
