//! The router the box is served, on a local port as the relay would reach it, in front of a fake
//! model endpoint and a fake time-portal API; and where a model call goes.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::routing::post;
use epochal_api::Client;
use epochal_api::chrono::DateTime;
use epochal_mcp::{ToolCall, Tools};
use reqwest::Client as HttpClient;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

/// 2025-03-01T23:59:59Z.
const AS_OF: i64 = 1_740_873_599;
/// The `Host` a harness in the box sends to the relay's port.
const BOX_HOST: &str = "127.0.0.1:8787";

async fn spawn(app: Router) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// The user's model endpoint and Epochal's API, faked on one local port.
async fn upstream() -> SocketAddr {
    let app = Router::new()
        .route(
            "/v1/messages",
            post(|headers: HeaderMap, body: String| async move {
                let key = headers["x-api-key"].to_str().unwrap().to_owned();
                assert!(!headers.contains_key("authorization"), "{headers:?}");
                (
                    [
                        ("content-type", "text/event-stream"),
                        ("date", "Fri, 06 Nov 2026 12:00:00 GMT"),
                    ],
                    format!(
                        "data: {{\"created_at\":1794010015,\"key\":\"{key}\",\"echo\":{body}}}\n\n"
                    ),
                )
            }),
        )
        .route(
            "/v1/responses",
            post(|headers: HeaderMap| async move {
                let header = |name: &str| headers.get(name).map(|v| v.to_str().unwrap().to_owned());
                Json(json!({
                    "authorization": header("authorization"),
                    "x-api-key": header("x-api-key"),
                }))
            }),
        )
        .route(
            "/v1/time-portal/search",
            post(|headers: HeaderMap, Json(body): Json<Value>| async move {
                Json(json!({
                    "as_of": headers["accept-datetime"].to_str().unwrap(),
                    "request_id": "r",
                    "results": [{
                        "title": body["query"], "url": "https://a.example", "snippet": "s",
                        "published_at": "2025-02-26T00:00:00Z",
                    }],
                }))
            }),
        );
    spawn(app).await
}

fn endpoint(base: &str) -> Endpoint {
    let upstream = Upstream::from_env(|name| match name {
        "EPOCHAL_LLM_BASE_URL" => Some(base.to_owned()),
        "EPOCHAL_LLM_API_KEY" => Some("user-model-key".to_owned()),
        _ => None,
    });
    Endpoint::new(upstream.unwrap(), AS_OF).unwrap()
}

fn tools(api: &str, observer: impl Fn(ToolCall) + Send + Sync + 'static) -> Tools {
    let client = Client::builder()
        .with_api_key("user-epochal-key")
        .with_base_url(api)
        .build()
        .unwrap();
    Tools::time_portal(DateTime::from_timestamp(AS_OF, 0).unwrap())
        .with_client(client)
        .with_observer(observer)
        .build()
        .unwrap()
}

/// A raw HTTP/1.1 request as the box would send it, with no client to normalize the target.
async fn raw(addr: SocketAddr, head: &str, body: &str) -> String {
    let mut conn = TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "{head}\r\nhost: {BOX_HOST}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    );
    conn.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    conn.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn a_model_call_gets_the_users_key_and_only_the_as_of_comes_back() {
    let up = format!("http://{}", upstream().await);
    let addr = spawn(router(endpoint(&up), tools(&up, |_| {}))).await;

    let resp = HttpClient::new()
        .post(format!("http://{addr}/v1/messages"))
        .bearer_auth("placeholder")
        .body(r#"{"model":"m"}"#)
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    // The upstream's own headers stop here; the relay serves with no `Date` of its own.
    let dates: Vec<_> = resp.headers().get_all("date").iter().collect();
    assert!(
        !dates.iter().any(|d| *d == "Fri, 06 Nov 2026 12:00:00 GMT"),
        "{dates:?}"
    );
    assert_eq!(
        resp.text().await.unwrap(),
        format!(
            "data: {{\"created_at\":{AS_OF},\"key\":\"user-model-key\",\"echo\":{{\"model\":\"m\"}}}}\n\n"
        )
    );
}

#[tokio::test]
async fn a_responses_call_carries_the_same_key_as_only_a_bearer_token() {
    let up = format!("http://{}", upstream().await);
    let addr = spawn(router(endpoint(&up), tools(&up, |_| {}))).await;

    let seen: Value = HttpClient::new()
        .post(format!("http://{addr}/v1/responses"))
        .bearer_auth("placeholder")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        seen,
        json!({"authorization": "Bearer user-model-key", "x-api-key": null})
    );
}

#[tokio::test]
async fn the_web_tools_answer_the_boxs_host_and_every_call_is_observed() {
    let up = format!("http://{}", upstream().await);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let addr = spawn(router(
        endpoint(&up),
        tools(&up, move |call| seen.lock().unwrap().push(call)),
    ))
    .await;

    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    let head = "POST /mcp HTTP/1.1\r\ncontent-type: application/json\r\naccept: application/json, text/event-stream";
    let answer = raw(addr, head, init).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    let config = StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/mcp"));
    let transport = StreamableHttpClientTransport::with_client(HttpClient::new(), config);
    let mcp = ().serve(transport).await.unwrap();
    let mut args = Map::new();
    args.insert("query".into(), "nvidia".into());
    let found = mcp
        .call_tool(CallToolRequestParams::new("web_search").with_arguments(args))
        .await
        .unwrap();
    let text = serde_json::to_string(&found.content).unwrap();
    assert!(
        text.contains("nvidia") && !text.contains("2025-02-26"),
        "{text}"
    );

    let calls = calls.lock().unwrap();
    let [
        ToolCall::Search {
            request,
            response: Ok(found),
        },
    ] = calls.as_slice()
    else {
        panic!("{calls:?}");
    };
    assert_eq!(request.query, "nvidia");
    assert_eq!(found.as_of, "2025-03-01T23:59:59Z");
    assert_eq!(
        found.results[0].published_at.as_deref(),
        Some("2025-02-26T00:00:00Z")
    );
}

/// Every path the upstream is asked for, on a port that answers anything.
async fn recording_upstream() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = Router::new().fallback(move |req: Request| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(req.uri().path().to_owned());
            "ok"
        }
    });
    (spawn(app).await, seen)
}

#[tokio::test]
async fn a_request_outside_the_endpoint_never_reaches_the_upstream() {
    let (up, seen) = recording_upstream().await;
    let addr = spawn(router(
        endpoint(&format!("http://{up}/llm")),
        tools("http://127.0.0.1:9", |_| {}),
    ))
    .await;

    for escape in [
        "POST /../llm-other/v1/messages",
        "POST /../v1/messages",
        "GET /key/list",
        "GET /user/info",
        "POST /key/generate",
        "GET /v1/messages",
        "POST /v1/messages/../../key/list",
        "POST /v1/%6Dessages",
    ] {
        let resp = raw(addr, &format!("{escape} HTTP/1.1"), "").await;
        assert!(resp.starts_with("HTTP/1.1 404"), "{escape}: {resp}");
        assert!(
            !resp.contains(&up.to_string()) && !resp.contains("llm"),
            "{escape} leaked the endpoint: {resp}"
        );
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "escapes reached the upstream: {:?}",
        seen.lock().unwrap()
    );

    let control = raw(addr, "POST /v1/messages HTTP/1.1", "").await;
    assert!(control.starts_with("HTTP/1.1 200"), "{control}");
    assert_eq!(*seen.lock().unwrap(), ["/llm/v1/messages"]);
}

fn from_vars(vars: &[(&str, &str)]) -> Result<Upstream> {
    Upstream::from_env(|name| {
        vars.iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_owned())
    })
}

fn url(base: &str, path: &str) -> Result<String> {
    let upstream = from_vars(&[("EPOCHAL_LLM_BASE_URL", base), ("EPOCHAL_LLM_API_KEY", "k")])?;
    Ok(Endpoint::new(upstream, 0)?.url(path)?.0.to_string())
}

#[test]
fn a_base_is_taken_as_the_built_in_harness_takes_it() {
    let url = |base, path| url(base, path).unwrap();
    for base in ["https://api.anthropic.com", "https://api.anthropic.com/v1/"] {
        assert_eq!(
            url(base, "/v1/messages?beta=true"),
            "https://api.anthropic.com/v1/messages?beta=true"
        );
    }
    for base in ["https://api.openai.com", "https://api.openai.com/v1"] {
        assert_eq!(
            url(base, "/v1/responses"),
            "https://api.openai.com/v1/responses"
        );
    }
    assert_eq!(
        url("https://openrouter.ai/api/v1", "/v1/responses"),
        "https://openrouter.ai/api/v1/responses"
    );
}

#[test]
fn a_missing_variable_is_named_and_a_non_http_base_refused() {
    let err = |vars: &[(&str, &str)]| from_vars(vars).err().unwrap().to_string();
    assert_eq!(
        err(&[("EPOCHAL_LLM_BASE_URL", "https://h.example")]),
        "set EPOCHAL_LLM_API_KEY to the model endpoint's key"
    );
    assert_eq!(
        err(&[
            ("EPOCHAL_LLM_API_KEY", "k"),
            ("ANTHROPIC_BASE_URL", "https://h.example")
        ]),
        "set EPOCHAL_LLM_BASE_URL to the model endpoint"
    );
    assert!(
        err(&[
            ("EPOCHAL_LLM_BASE_URL", "ftp://h.example"),
            ("EPOCHAL_LLM_API_KEY", "k")
        ])
        .contains("not an http or https URL")
    );
    assert!(
        err(&[
            ("EPOCHAL_LLM_BASE_URL", "https://user:pw@h.example"),
            ("EPOCHAL_LLM_API_KEY", "k")
        ])
        .contains("must not hold a user or password")
    );
}
