//! The tools served over streamable HTTP in-process, called by an rmcp client, against a fake
//! time-portal API that replies with the API reference's examples.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use epochal_api::Client;
use epochal_mcp::{ToolCall, Tools, parse_as_of};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const AS_OF: &str = "2026-03-01T00:00:00Z";

const SEARCH_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "results": [{"title": "Battery plant start pushed back to late 2027", "url": "https://news.example.com/energy/battery-plant-delay", "snippet": "The cell maker said commissioning would slip by three quarters…", "published_at": "2026-02-11T06:12:00Z", "domain": "example.com", "language": "en"}, {"title": "Gigafactory permits stall", "url": "https://other.example.org/permits", "snippet": " Regional permits are still pending. ", "published_at": null}], "entities": []}"#;

const FETCH_EXAMPLE: &str = r##"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "status": "success", "id": "6b81feebde1a4821a125fb307894e7ad", "url": "https://news.example.com/energy/battery-plant-delay", "final_url": "https://news.example.com/energy/battery-plant-delay/", "title": "Battery plant start pushed back to late 2027", "description": "The cell maker said commissioning would slip.", "author": "Jane Doe", "published_at": "2026-02-11T06:12:00Z", "language": "en", "content": "# Battery plant start pushed back to late 2027\n\nThe cell maker said…", "truncated": true, "entity": null}"##;
const UNREAD_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "status": "error", "page_error": {"http_status_code": 404, "message": "Page not found."}, "url": "https://news.example.com/missing", "final_url": "https://news.example.com/missing", "title": "", "content": "", "truncated": false, "entity": null}"#;

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    accept_datetime: Option<String>,
    body: Value,
}

type Log = Arc<Mutex<Vec<Seen>>>;
type Calls = Arc<Mutex<Vec<ToolCall>>>;

async fn handle(
    State(log): State<Log>,
    Path(op): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let header = |name: &str| Some(headers.get(name)?.to_str().ok()?.to_owned());
    log.lock().unwrap().push(Seen {
        path: format!("/v1/time-portal/{op}"),
        accept_datetime: header("accept-datetime"),
        body: body.clone(),
    });
    if header("x-api-key").as_deref() != Some("good-key") {
        let error = json!({"error": {"type": "invalid_api_key", "message": "No key matches that token.", "request_id": "req_9Zp4kL"}});
        return (StatusCode::UNAUTHORIZED, Json(error)).into_response();
    }
    match (op.as_str(), body["query"].as_str()) {
        ("search", Some("nothing")) => {
            r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "r", "results": [], "entities": []}"#
                .into_response()
        }
        ("search", _) => SEARCH_EXAMPLE.into_response(),
        ("fetch", _) if body["url"] == "https://news.example.com/missing" => {
            UNREAD_EXAMPLE.into_response()
        }
        ("fetch", _) => FETCH_EXAMPLE.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Serves `app` on a free local port and returns its address.
async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// The tools, using `api_key` against the fake API; returns them, the fake API's request log and
/// every call the observer was given.
async fn tools(api_key: &str, allowed_hosts: &[&str]) -> (Tools, Log, Calls) {
    let log = Log::default();
    let api = Router::new()
        .route("/v1/time-portal/{op}", post(handle))
        .with_state(log.clone());
    let client = Client::builder()
        .with_api_key(api_key)
        .with_base_url(format!("http://{}", spawn(api).await))
        .build()
        .unwrap();
    let calls = Calls::default();
    let observed = calls.clone();
    let tools = Tools::time_portal(AS_OF.parse().unwrap())
        .with_client(client)
        .with_allowed_hosts(allowed_hosts.iter().copied())
        .with_observer(move |call| observed.lock().unwrap().push(call))
        .build()
        .unwrap();
    (tools, log, calls)
}

/// The tools mounted at `/mcp`; returns the MCP server's address, the log and the calls.
async fn serve(api_key: &str, allowed_hosts: &[&str]) -> (String, Log, Calls) {
    let (tools, log, calls) = tools(api_key, allowed_hosts).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mcp = listener.local_addr().unwrap().to_string();
    tokio::spawn(tools.serve_http(listener));
    (mcp, log, calls)
}

async fn connect_observed(api_key: &str) -> (RunningService<RoleClient, ()>, Log, Calls) {
    let (addr, log, calls) = serve(api_key, &[]).await;
    let port = addr.rsplit(':').next().unwrap();
    let transport = StreamableHttpClientTransport::from_uri(format!("http://localhost:{port}/mcp"));
    (().serve(transport).await.unwrap(), log, calls)
}

async fn connect(api_key: &str) -> (RunningService<RoleClient, ()>, Log) {
    let (mcp, log, _) = connect_observed(api_key).await;
    (mcp, log)
}

async fn call(
    mcp: &RunningService<RoleClient, ()>,
    tool: &'static str,
    args: Value,
) -> CallToolResult {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    mcp.call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await
        .unwrap()
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| Some(block.as_text()?.text.as_str()))
        .collect()
}

#[tokio::test]
async fn lists_search_and_fetch_with_their_arguments() {
    let (mcp, _) = connect("good-key").await;

    let mut tools = mcp.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));

    let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, ["web_fetch", "web_search"]);
    let properties = |i: usize| {
        let mut keys: Vec<_> = tools[i].input_schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(properties(0), ["max_characters", "url"]);
    assert_eq!(properties(1), ["domains", "num_results", "query"]);
    assert_eq!(tools[1].input_schema["required"], json!(["query"]));
    let range = |i: usize, arg: &str| {
        let arg = &tools[i].input_schema["properties"][arg];
        (arg["minimum"].clone(), arg["maximum"].clone())
    };
    assert_eq!(range(0, "max_characters"), (json!(1), json!(100_000)));
    assert_eq!(range(1, "num_results"), (json!(1), json!(10)));
}

/// Whether `text` shows any timestamp the fixtures carry: `as_of`, `published_at`.
fn has_date(text: &str) -> bool {
    ["2026-03-01", "2026-02-11", "06:12"]
        .iter()
        .any(|t| text.contains(t))
}

#[tokio::test]
async fn search_numbers_each_result_with_its_title_url_and_snippet() {
    let (mcp, log) = connect("good-key").await;

    let result = call(
        &mcp,
        "web_search",
        json!({"query": "battery gigafactory delays", "num_results": 500, "domains": ["example.com"]}),
    )
    .await;

    assert_ne!(result.is_error, Some(true));
    assert_eq!(
        text(&result),
        "1. Battery plant start pushed back to late 2027\n   https://news.example.com/energy/battery-plant-delay\n   The cell maker said commissioning would slip by three quarters…\n\n\
         2. Gigafactory permits stall\n   https://other.example.org/permits\n   Regional permits are still pending.\n\n"
    );
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.path, "/v1/time-portal/search");
    assert_eq!(seen.accept_datetime.as_deref(), Some(AS_OF));
    assert_eq!(
        seen.body,
        json!({"query": "battery gigafactory delays", "num_results": 10, "domains": ["example.com"]})
    );
}

#[tokio::test]
async fn search_output_carries_no_timestamps() {
    let (mcp, _) = connect("good-key").await;

    let shown = text(&call(&mcp, "web_search", json!({"query": "battery"})).await);

    assert!(!has_date(&shown), "{shown}");
}

#[test]
fn an_unreadable_instant_is_refused() {
    assert!(parse_as_of("last tuesday").is_err());
}

#[tokio::test]
async fn search_with_no_hits_says_so() {
    let (mcp, _) = connect("good-key").await;

    let result = call(&mcp, "web_search", json!({"query": "nothing"})).await;

    assert_eq!(text(&result), "No results.");
}

#[tokio::test]
async fn fetch_shows_the_page_and_no_timestamps() {
    let (mcp, log) = connect("good-key").await;

    let result = call(
        &mcp,
        "web_fetch",
        json!({"url": "https://news.example.com/energy/battery-plant-delay", "as_of": "2030-01-01T00:00:00Z"}),
    )
    .await;

    let shown = text(&result);
    assert_eq!(
        shown,
        "Battery plant start pushed back to late 2027\nhttps://news.example.com/energy/battery-plant-delay/\n\n# Battery plant start pushed back to late 2027\n\nThe cell maker said…\n[truncated]"
    );
    assert!(!has_date(&shown), "{shown}");
    let seen = log.lock().unwrap()[0].clone();
    assert_eq!(seen.path, "/v1/time-portal/fetch");
    assert_eq!(seen.accept_datetime.as_deref(), Some(AS_OF));
    assert_eq!(
        seen.body,
        json!({"url": "https://news.example.com/energy/battery-plant-delay", "max_characters": 20000})
    );
}

#[tokio::test]
async fn a_page_that_could_not_be_read_is_a_tool_error_with_the_sites_status() {
    let (mcp, _, calls) = connect_observed("good-key").await;

    let result = call(
        &mcp,
        "web_fetch",
        json!({"url": "https://news.example.com/missing"}),
    )
    .await;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        text(&result),
        "could not read https://news.example.com/missing: Page not found. (HTTP 404)"
    );
    let calls = calls.lock().unwrap();
    let [
        ToolCall::Fetch {
            response: Ok(page), ..
        },
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_eq!(
        page.page_error.as_ref().unwrap().http_status_code,
        Some(404)
    );
}

#[tokio::test]
async fn a_refused_call_is_a_tool_error_with_the_api_message() {
    let (mcp, _) = connect("bad-key").await;

    let result = call(&mcp, "web_search", json!({"query": "q"})).await;

    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        text(&result),
        "web_search failed: No key matches that token."
    );
}

/// The status line of a bare initialize POST to `/mcp` at `addr`, sent with `Host: host`.
async fn status_for_host(addr: &str, host: &str) -> String {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\nhost: {host}\r\ncontent-type: application/json\r\naccept: application/json, text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn http_answers_loopback_and_allowed_hosts_only() {
    let (addr, _, _) = serve("good-key", &["tools.internal"]).await;

    assert!(status_for_host(&addr, "localhost").await.contains("200"));
    assert!(
        status_for_host(&addr, "tools.internal:8080")
            .await
            .contains("200")
    );
    let refused = status_for_host(&addr, "attacker.example").await;
    assert!(refused.contains("403"), "{refused}");
}

#[tokio::test]
async fn the_observer_gets_each_call_with_its_timestamps() {
    let (mcp, _, calls) = connect_observed("good-key").await;

    call(&mcp, "web_search", json!({"query": "battery"})).await;
    call(
        &mcp,
        "web_fetch",
        json!({"url": "https://news.example.com/energy/battery-plant-delay"}),
    )
    .await;

    let calls = calls.lock().unwrap();
    let [first, second] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    let ToolCall::Search {
        request,
        response: Ok(found),
    } = first
    else {
        panic!("{first:?}");
    };
    assert_eq!(request.query, "battery");
    assert_eq!(request.num_results, Some(10));
    assert_eq!(
        found.results[0].published_at.as_deref(),
        Some("2026-02-11T06:12:00Z")
    );
    let ToolCall::Fetch {
        request,
        response: Ok(page),
    } = second
    else {
        panic!("{second:?}");
    };
    assert_eq!(
        request.url,
        "https://news.example.com/energy/battery-plant-delay"
    );
    assert_eq!(page.published_at.as_deref(), Some("2026-02-11T06:12:00Z"));
    let written = serde_json::to_value(second).unwrap();
    assert_eq!(
        written["response"]["published_at"], "2026-02-11T06:12:00Z",
        "{written}"
    );
    assert!(written.get("error").is_none(), "{written}");
}

#[tokio::test]
async fn the_observer_gets_a_refused_call_as_its_error() {
    let (mcp, _, calls) = connect_observed("bad-key").await;

    call(&mcp, "web_fetch", json!({"url": "https://a.example"})).await;

    let calls = calls.lock().unwrap();
    let [call] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    let written = serde_json::to_value(call).unwrap();
    assert_eq!(written["tool"], "fetch");
    assert_eq!(written["request"]["url"], "https://a.example");
    assert!(written.get("response").is_none(), "{written}");
    assert!(
        written["error"]
            .as_str()
            .is_some_and(|e| e.contains("No key matches")),
        "{written}"
    );
}

#[tokio::test]
async fn http_responses_carry_no_date_so_the_client_is_not_told_today() {
    let (addr, _, _) = serve("good-key", &[]).await;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("accept", "application/json, text/event-stream")
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
        .send()
        .await
        .unwrap();

    assert!(response.status().is_success(), "{}", response.status());
    assert!(
        response.headers().get("date").is_none(),
        "{:?}",
        response.headers()
    );
}

#[tokio::test]
async fn an_in_process_call_answers_as_the_mcp_call_does() {
    let (mcp, _) = connect("good-key").await;
    let (tools, _, calls) = tools("good-key", &[]).await;
    let listed: Vec<_> = tools.definitions().into_iter().map(|t| t.name).collect();
    assert_eq!(listed, ["web_fetch", "web_search"]);
    for (tool, args) in [
        ("web_search", json!({"query": "battery"})),
        (
            "web_fetch",
            json!({"url": "https://news.example.com/missing"}),
        ),
    ] {
        let served = call(&mcp, tool, args.clone()).await;
        let direct = tools.call(tool, args).await;
        assert_eq!(
            (text(&direct), direct.is_error),
            (text(&served), served.is_error)
        );
    }
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "each in-process call is observed"
    );

    let unknown = tools.call("web_crawl", json!({})).await;
    assert_eq!(unknown.is_error, Some(true));
    let missing = tools.call("web_search", json!({"num_results": 3})).await;
    assert_eq!(missing.is_error, Some(true));
    assert!(text(&missing).contains("query"), "{}", text(&missing));
}
