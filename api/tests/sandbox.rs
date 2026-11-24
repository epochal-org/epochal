//! The sandbox calls against an in-process server that records each request and replies as the
//! API reference documents, with its examples.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use common::spawn;
use epochal_api::chrono::{DateTime, Utc};
use epochal_api::time_portal::TimePortal;
use epochal_api::time_portal::sandbox::{
    ExecExit, ExecInput, ExecOutputRequest, ExecStartRequest, FileListRequest, FileType, Network,
    SandboxCreateRequest, SandboxInfo, SandboxListRequest, SandboxState, Stream,
};
use epochal_api::{Client, Error};
use serde_json::{Value, json};

const AS_OF: &str = "2026-03-01T00:00:00Z";

fn as_of() -> DateTime<Utc> {
    AS_OF.parse().unwrap()
}
const ID: &str = "sbx_7f3k9q2m4x8p1c6w";
const FAILED_ID: &str = "sbx_failed";
const EXEC: &str = "exec_m3p8x2k7";

const SANDBOX_EXAMPLE: &str = r#"{"id": "sbx_7f3k9q2m4x8p1c6w", "state": "live", "as_of": "2026-03-01T00:00:00Z", "network": "time-portal", "vcpus": 2, "memory_gib": 4, "timeout": "1h", "idle_timeout": "10m", "created_at": "2026-11-02T18:04:11Z", "booted_at": "2026-11-02T18:04:14Z", "expires_at": "2026-11-02T19:04:14Z", "ended_at": null, "error": null, "metadata": {"job": "nightly"}, "apps": [{"name": "relay", "version": "1.2.0"}]}"#;

const FAILED_EXAMPLE: &str = r#"{"id": "sbx_failed", "state": "failed", "as_of": "2026-03-01T00:00:00Z", "network": "none", "vcpus": 1, "memory_gib": 1, "timeout": "1h", "idle_timeout": "10m", "created_at": "2026-11-02T18:04:11Z", "error": {"type": "spend_limit_reached", "message": "The organization's spend limit is reached."}, "apps": []}"#;

const OUTPUT_EXAMPLE: &str = r#"{"frames": [{"stream": "stdout", "offset": 0, "data_b64": "MjAyNi0wMy0wMQo="}], "exit": {"reason": "exited", "code": 0}}"#;

const STAT_EXAMPLE: &str = r#"{"path": "/work/out/result.csv", "type": "file", "size": 18432, "mode": 420, "mtime": "2026-03-01T00:02:13Z"}"#;

const FILE: &[u8] = b"0123456789";

#[derive(Clone, Debug)]
struct Seen {
    method: Method,
    uri: String,
    headers: HeaderMap,
    body: Bytes,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn conflict(next_seq: u64) -> Response {
    let body = json!({"error": {"type": "conflict", "message": "Input 'seq' is out of order.", "request_id": "req_1", "next_seq": next_seq}});
    (StatusCode::CONFLICT, Json(body)).into_response()
}

async fn handle(
    State(log): State<Log>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let range = headers
        .get("range")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    log.lock().unwrap().push(Seen {
        method: method.clone(),
        uri: uri.to_string(),
        headers,
        body: body.clone(),
    });
    let path = uri.path().trim_start_matches("/v1/time-portal/sandbox");
    let query = uri.query().unwrap_or_default();
    let in_box = path.strip_prefix("/sbx_7f3k9q2m4x8p1c6w");
    match (method.as_str(), path, in_box) {
        ("POST", "", _) => (StatusCode::ACCEPTED, SANDBOX_EXAMPLE).into_response(),
        ("GET", "", _) => {
            format!(r#"{{"sandboxes": [{SANDBOX_EXAMPLE}], "next_cursor": "c2"}}"#).into_response()
        }
        ("GET", "/sbx_failed", _) => FAILED_EXAMPLE.into_response(),
        ("GET", _, Some("")) => SANDBOX_EXAMPLE.into_response(),
        ("DELETE", _, Some("")) => StatusCode::NO_CONTENT.into_response(),
        ("POST", _, Some("/execs")) => r#"{"id": "exec_m3p8x2k7"}"#.into_response(),
        ("GET", _, Some("/execs/exec_m3p8x2k7/output")) => OUTPUT_EXAMPLE.into_response(),
        ("POST", _, Some("/execs/exec_m3p8x2k7/input")) => {
            let seq = serde_json::from_slice::<Value>(&body).unwrap()["seq"]
                .as_u64()
                .unwrap();
            if seq == 3 {
                r#"{"next_seq": 4}"#.into_response()
            } else {
                conflict(3)
            }
        }
        ("GET", _, Some("/files/stat")) => STAT_EXAMPLE.into_response(),
        ("GET", _, Some("/files/list")) => {
            format!(r#"{{"entries": [{STAT_EXAMPLE}], "next_cursor": null}}"#).into_response()
        }
        ("GET", _, Some("/files")) if query.contains("%2Fempty") => StatusCode::OK.into_response(),
        ("GET", _, Some("/files")) => read(range.as_deref()),
        ("PUT", _, Some("/files")) => STAT_EXAMPLE.into_response(),
        ("DELETE", _, Some("/files")) => StatusCode::NO_CONTENT.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `FILE` as a box serves it: the asked range clamped to its end, or 416 past it.
fn read(range: Option<&str>) -> Response {
    let (first, last) = range
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split_once('-'))
        .unwrap();
    let first: usize = first.parse().unwrap();
    let size = FILE.len();
    if first >= size {
        let content_range = format!("bytes */{size}");
        return (
            StatusCode::RANGE_NOT_SATISFIABLE,
            [("content-range", content_range)],
        )
            .into_response();
    }
    let last = last.parse::<usize>().map_or(size - 1, |l| l.min(size - 1));
    let content_range = format!("bytes {first}-{last}/{size}");
    (
        StatusCode::PARTIAL_CONTENT,
        [("content-range", content_range)],
        FILE[first..=last].to_vec(),
    )
        .into_response()
}

async fn fake_api() -> (TimePortal, Log) {
    let log = Log::default();
    let app = Router::new()
        .route("/v1/time-portal/sandbox", any(handle))
        .route("/v1/time-portal/sandbox/{*rest}", any(handle))
        .with_state(log.clone());
    let client = Client::builder()
        .with_api_key("good-key")
        .with_base_url(spawn(app).await)
        .build()
        .unwrap()
        .time_portal(as_of());
    (client, log)
}

fn last(log: &Log) -> Seen {
    log.lock().unwrap().last().unwrap().clone()
}

fn in_box(rest: &str) -> String {
    format!("/v1/time-portal/sandbox/{ID}{rest}")
}

#[tokio::test]
async fn create_sends_the_instant_the_fields_set_and_the_key() {
    let (client, log) = fake_api().await;
    let mut request = SandboxCreateRequest::default();
    request.network = Some(Network::TimePortal);
    request.timeout = Some(Duration::from_secs(5400));
    request.metadata.insert("job".into(), "nightly".into());
    request.apps = vec!["relay".into()];
    request.idempotency_key = Some("job-42-box".into());

    let sandbox = client.create_sandbox(&request).await.unwrap();

    let seen = last(&log);
    assert_eq!(
        (seen.method.as_str(), seen.uri.as_str()),
        ("POST", "/v1/time-portal/sandbox")
    );
    assert_eq!(seen.header("accept-datetime"), Some(AS_OF));
    assert_eq!(seen.header("x-api-key"), Some("good-key"));
    assert_eq!(seen.header("idempotency-key"), Some("job-42-box"));
    assert_eq!(
        seen.json(),
        json!({"network": "time-portal", "timeout": "90m", "metadata": {"job": "nightly"},
               "apps": ["relay"]}),
        "the key travels only as a header"
    );
    assert_eq!(sandbox.id, ID);
    assert_eq!(sandbox.state, SandboxState::Live);
    assert_eq!(sandbox.network, Network::TimePortal);
    assert_eq!(sandbox.timeout, Duration::from_secs(3600));
    assert_eq!(sandbox.idle_timeout, Duration::from_secs(600));
    assert_eq!(sandbox.booted_at.as_deref(), Some("2026-11-02T18:04:14Z"));
    assert_eq!(sandbox.metadata["job"], "nightly");
    assert_eq!(sandbox.apps[0].name, "relay");
    assert_eq!(sandbox.error, None);
}

#[tokio::test]
async fn creates_and_starts_without_a_key_each_send_a_fresh_one() {
    let (client, log) = fake_api().await;
    let start = ExecStartRequest::new(["true"]);

    let mut keys = Vec::new();
    for _ in 0..2 {
        client
            .create_sandbox(&SandboxCreateRequest::default())
            .await
            .unwrap();
        let seen = last(&log);
        assert_eq!(seen.json(), json!({}));
        keys.push(seen.header("idempotency-key").unwrap().to_owned());
        client.start_exec(ID, &start).await.unwrap();
        keys.push(last(&log).header("idempotency-key").unwrap().to_owned());
    }

    for key in &keys {
        assert_eq!(key.len(), 32, "{key:?}");
    }
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 4, "no two calls share a key");
}

#[tokio::test]
async fn a_duration_the_api_cannot_express_is_refused_before_any_request() {
    let (client, log) = fake_api().await;
    let mut request = SandboxCreateRequest::default();
    request.idle_timeout = Some(Duration::from_millis(1500));

    let err = client.create_sandbox(&request).await.unwrap_err();

    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_failed_box_says_why_by_kind() {
    let (client, _) = fake_api().await;

    let failed = client.get_sandbox(FAILED_ID, None).await.unwrap();

    assert_eq!(failed.state, SandboxState::Failed);
    let error = failed.error.unwrap();
    assert_eq!(error.kind, "spend_limit_reached");
    assert!(error.message.contains("spend limit"), "{}", error.message);
}

#[tokio::test]
async fn get_waits_by_duration_and_terminate_ends_the_box() {
    let (client, log) = fake_api().await;

    client
        .get_sandbox(ID, Some(Duration::from_secs(55)))
        .await
        .unwrap();
    assert_eq!(last(&log).uri, in_box("?wait=55s"));

    client.terminate_sandbox(ID).await.unwrap();
    let seen = last(&log);
    assert_eq!(
        (seen.method.as_str(), seen.uri.clone()),
        ("DELETE", in_box(""))
    );
}

#[tokio::test]
async fn list_filters_pages_and_returns_the_next_cursor() {
    let (client, log) = fake_api().await;
    let mut request = SandboxListRequest::default();
    request.state = Some(SandboxState::Live);
    request.cursor = Some("c1".into());
    request.limit = Some(20);

    let page = client.list_sandboxes(&request).await.unwrap();

    assert_eq!(
        last(&log).uri,
        "/v1/time-portal/sandbox?state=live&cursor=c1&limit=20"
    );
    assert_eq!(page.sandboxes[0].id, ID);
    assert_eq!(page.next_cursor.as_deref(), Some("c2"));

    client
        .list_sandboxes(&SandboxListRequest::default())
        .await
        .unwrap();
    assert_eq!(last(&log).uri, "/v1/time-portal/sandbox");

    request.state = Some(SandboxState::Unknown);
    let err = client.list_sandboxes(&request).await.unwrap_err();
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
}

#[test]
fn a_box_serializes_back_to_the_wire_shape() {
    let info: SandboxInfo = serde_json::from_str(SANDBOX_EXAMPLE).unwrap();
    let back = serde_json::to_value(&info).unwrap();
    assert_eq!(
        (&back["timeout"], &back["idle_timeout"]),
        (&json!("1h"), &json!("10m"))
    );
    assert_eq!(serde_json::from_value::<SandboxInfo>(back).unwrap(), info);
}

#[tokio::test]
async fn start_exec_sends_the_command_stdin_and_key() {
    let (client, log) = fake_api().await;
    let mut request = ExecStartRequest::new(["python3", "-c", "print(1)"]);
    request.cwd = Some("/work".into());
    request.env.insert("TOKEN".into(), "secret".into());
    request.stdin = b"hi".to_vec();
    request.idempotency_key = Some("job-42-step-1".into());

    let exec = client.start_exec(ID, &request).await.unwrap();

    assert_eq!(exec, EXEC);
    let seen = last(&log);
    assert_eq!(seen.uri, in_box("/execs"));
    assert_eq!(seen.header("idempotency-key"), Some("job-42-step-1"));
    assert_eq!(
        seen.json(),
        json!({"argv": ["python3", "-c", "print(1)"], "env": {"TOKEN": "secret"},
               "cwd": "/work", "stdin_b64": "aGk="})
    );
    assert!(
        !format!("{request:?}").contains("secret"),
        "env values are not logged"
    );
}

#[tokio::test]
async fn an_empty_argv_is_refused_before_any_request() {
    let (client, log) = fake_api().await;

    let err = client
        .start_exec(ID, &ExecStartRequest::new(Vec::<String>::new()))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn output_is_read_from_the_offsets_asked() {
    let (client, log) = fake_api().await;
    let mut request = ExecOutputRequest::default();
    request.stdout_offset = 4;
    request.wait = Some(Duration::from_secs(30));
    request.max_bytes = Some(65_536);

    let output = client.read_exec_output(ID, EXEC, &request).await.unwrap();

    assert_eq!(
        last(&log).uri,
        in_box(
            "/execs/exec_m3p8x2k7/output?stdout_offset=4&stderr_offset=0&wait=30s&max_bytes=65536"
        )
    );
    assert_eq!(output.frames[0].stream, Stream::Stdout);
    assert_eq!(output.frames[0].data, b"2026-03-01\n");
    assert_eq!(output.exit, Some(ExecExit::Exited { code: 0 }));
}

#[tokio::test]
async fn input_encodes_each_action_and_returns_the_next_seq() {
    let (client, log) = fake_api().await;

    let next = client
        .send_exec_input(ID, EXEC, 3, &ExecInput::Data(b"hi\n".to_vec()))
        .await
        .unwrap();
    assert_eq!(next, 4);
    assert_eq!(last(&log).json(), json!({"seq": 3, "data_b64": "aGkK"}));

    client
        .send_exec_input(ID, EXEC, 3, &ExecInput::Eof)
        .await
        .unwrap();
    assert_eq!(last(&log).json(), json!({"seq": 3, "eof": true}));

    client
        .send_exec_input(ID, EXEC, 3, &ExecInput::Signal(15))
        .await
        .unwrap();
    assert_eq!(last(&log).json(), json!({"seq": 3, "signal": 15}));
}

#[tokio::test]
async fn an_out_of_order_input_names_the_seq_expected() {
    let (client, _) = fake_api().await;

    let err = client
        .send_exec_input(ID, EXEC, 9, &ExecInput::Eof)
        .await
        .unwrap_err();

    assert!(
        matches!(&err, Error::Api { status: 409, kind, next_seq: Some(3), .. } if kind == "conflict"),
        "{err:?}"
    );
}

#[tokio::test]
async fn stat_and_list_read_file_metadata() {
    let (client, log) = fake_api().await;

    let stat = client.stat_file(ID, "/work/out/result.csv").await.unwrap();
    assert_eq!(
        last(&log).uri,
        in_box("/files/stat?path=%2Fwork%2Fout%2Fresult.csv")
    );
    assert_eq!(stat.file_type, FileType::File);
    assert_eq!((stat.size, stat.mode), (18432, 0o644));
    assert_eq!(stat.mtime.as_deref(), Some("2026-03-01T00:02:13Z"));

    let mut request = FileListRequest::new("/work/out");
    request.recursive = true;
    request.limit = Some(50);
    let page = client.list_files(ID, &request).await.unwrap();
    assert_eq!(
        last(&log).uri,
        in_box("/files/list?path=%2Fwork%2Fout&recursive=true&limit=50")
    );
    assert_eq!(page.entries, vec![stat]);
    assert_eq!(page.next_cursor, None);
}

#[tokio::test]
async fn read_file_says_where_the_bytes_start_and_the_size() {
    let (client, log) = fake_api().await;

    let range = client.read_file(ID, "/f", 4, Some(3)).await.unwrap();
    assert_eq!(last(&log).header("range"), Some("bytes=4-6"));
    assert_eq!(range.start, 4);
    assert_eq!(range.data, b"456");
    assert_eq!(range.total, Some(10));

    let rest = client.read_file(ID, "/f", 7, None).await.unwrap();
    assert_eq!(last(&log).header("range"), Some("bytes=7-"));
    assert_eq!((rest.start, rest.data.as_slice()), (7, &b"789"[..]));

    let past = client.read_file(ID, "/f", 10, Some(3)).await.unwrap();
    assert!(past.data.is_empty());
    assert_eq!(
        past.total,
        Some(10),
        "a read past the end still says the size"
    );
}

#[tokio::test]
async fn an_empty_file_reads_as_nothing_from_its_start() {
    let (client, _) = fake_api().await;

    let range = client.read_file(ID, "/empty", 0, None).await.unwrap();

    assert_eq!(range.start, 0);
    assert!(range.data.is_empty());
}

#[tokio::test]
async fn write_returns_the_file_after_and_remove_addresses_the_path() {
    let (client, log) = fake_api().await;

    let stat = client
        .write_file(ID, "/f", 8, b"tail".to_vec())
        .await
        .unwrap();
    let seen = last(&log);
    assert_eq!(
        (seen.method.as_str(), seen.uri.clone()),
        ("PUT", in_box("/files?path=%2Ff&offset=8"))
    );
    assert_eq!(&seen.body[..], b"tail");
    assert_eq!(
        seen.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(stat.size, 18432);

    client.remove_file(ID, "/d", true).await.unwrap();
    let seen = last(&log);
    assert_eq!(
        (seen.method.as_str(), seen.uri.clone()),
        ("DELETE", in_box("/files?path=%2Fd&recursive=true"))
    );
}

#[tokio::test]
async fn an_id_cannot_change_which_endpoint_is_called() {
    let (client, log) = fake_api().await;

    let _ = client.remove_file("sbx_victim?x=/../", "/f", false).await;

    let seen = last(&log);
    assert_eq!(seen.method.as_str(), "DELETE");
    assert_eq!(
        seen.uri,
        "/v1/time-portal/sandbox/sbx_victim%3Fx%3D%2F..%2F/files?path=%2Ff"
    );
}

#[tokio::test]
async fn an_id_that_a_url_would_drop_is_refused_before_any_request() {
    let (client, log) = fake_api().await;

    for id in ["", ".", ".."] {
        let refused = [
            client.get_sandbox(id, None).await.err(),
            client
                .send_exec_input(ID, id, 0, &ExecInput::Eof)
                .await
                .err(),
        ];
        for err in refused {
            assert!(
                matches!(err, Some(Error::InvalidArgument(_))),
                "{id:?}: {err:?}"
            );
        }
    }
    assert!(log.lock().unwrap().is_empty());
}
