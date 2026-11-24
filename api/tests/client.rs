//! The client against an in-process server that replies with the API reference's examples.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use common::spawn;
use epochal_api::chrono::{DateTime, Utc};
use epochal_api::time_portal::web::{
    Category, FetchRequest, FetchResponse, PageStatus, SearchRequest, SearchResponse,
};
use epochal_api::{Client, Error};
use serde_json::{Value, json};

const AS_OF: &str = "2026-03-01T00:00:00Z";

fn as_of() -> DateTime<Utc> {
    AS_OF.parse().unwrap()
}

const SEARCH_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "results": [{"title": "Battery plant start pushed back to late 2027", "url": "https://news.example.com/energy/battery-plant-delay", "snippet": "The cell maker said commissioning would slip by three quarters…", "published_at": "2026-02-11T06:12:00Z", "domain": "example.com", "language": "en", "entity": null}], "entities": []}"#;

const FETCH_EXAMPLE: &str = r##"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "status": "success", "id": "6b81feebde1a4821a125fb307894e7ad", "url": "https://news.example.com/energy/battery-plant-delay", "final_url": "https://news.example.com/energy/battery-plant-delay/", "title": "Battery plant start pushed back to late 2027", "description": "The cell maker said commissioning would slip.", "author": "Jane Doe", "published_at": "2026-02-11T06:12:00Z", "language": "en", "content": "# Battery plant start pushed back to late 2027\n\nThe cell maker said…", "truncated": true, "entity": null}"##;
const UNREAD_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "status": "error", "page_error": {"http_status_code": 404, "message": "Page not found."}, "id": null, "url": "https://news.example.com/energy/missing", "final_url": "https://news.example.com/energy/missing", "title": "", "description": null, "author": null, "published_at": null, "language": null, "content": "", "truncated": false, "entity": null}"#;

/// A search naming a company: its world page leads, carrying the entity, its properties as sent.
const ENTITY_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "results": [{"title": "NVIDIA", "url": "https://world.epochal.com/organization/nvidia", "snippet": "NVIDIA designs GPUs.", "published_at": null, "domain": "world.epochal.com", "language": "en", "entity": {"id": "https://world.epochal.com/organization/nvidia", "type": "organization", "schema_version": 1, "updated_at": "2026-02-28T00:00:00Z", "properties": {"name": "NVIDIA"}}}, {"title": "NVIDIA earnings beat", "url": "https://news.example.com/nvidia-earnings", "snippet": "Revenue rose…", "published_at": "2026-02-26T21:00:00Z", "domain": "example.com", "language": "en", "entity": null}], "entities": []}"#;

/// A server older than `results[].entity`: the field is absent.
const OLDER_SERVER_EXAMPLE: &str = r#"{"as_of": "2026-03-01T00:00:00Z", "request_id": "req_9Zp4kL", "results": [{"title": "NVIDIA earnings beat", "url": "https://news.example.com/nvidia-earnings", "snippet": "Revenue rose…"}]}"#;

const PAGE_HTML: &str =
    r#"<html><head><title>Battery plant</title></head><body>{"not": "json"}</body></html>"#;

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    api_key: Option<String>,
    accept_datetime: Option<String>,
    body: Value,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn api_error(status: StatusCode, kind: &str, message: &str) -> Response {
    let body = json!({"error": {"type": kind, "message": message, "request_id": "req_9Zp4kL"}});
    (status, Json(body)).into_response()
}

async fn handle(
    State(log): State<Log>,
    Path(op): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let header = |name: &str| Some(headers.get(name)?.to_str().ok()?.to_owned());
    log.lock().unwrap().push(Seen {
        path: format!("/v1/time-portal/{op}"),
        api_key: header("x-api-key"),
        accept_datetime: header("accept-datetime"),
        body: body.clone(),
    });
    if header("x-api-key").as_deref() != Some("good-key") {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "No key matches that token.",
        );
    }
    match (op.as_str(), body["query"].as_str(), body["url"].as_str()) {
        ("search", Some("proxy down"), _) => {
            (StatusCode::BAD_GATEWAY, "<html>bad gateway</html>").into_response()
        }
        ("search", Some("garbled"), _) => r#"{"results": "not a list"}"#.into_response(),
        ("search", Some("too fast"), _) => {
            let mut resp = api_error(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_exceeded",
                "Over the organization's rate limit.",
            );
            resp.headers_mut()
                .insert("retry-after", "7".parse().unwrap());
            resp
        }
        ("search", Some("nvidia"), _) => ENTITY_EXAMPLE.into_response(),
        ("search", ..) => SEARCH_EXAMPLE.into_response(),
        ("fetch", _, Some("https://x.example/home")) => api_error(
            StatusCode::FORBIDDEN,
            "url_not_allowed",
            "This URL can't be fetched.",
        ),
        ("fetch", _, Some("https://news.example.com/missing")) if body["format"] == "page" => [
            ("x-fetch-status-code", "404"),
            ("x-fetch-status-message", "Page not found."),
        ]
        .into_response(),
        ("fetch", _, Some("https://news.example.com/never")) if body["format"] == "page" => [
            ("x-fetch-status-code", "0"),
            (
                "x-fetch-status-message",
                "No copy at or before Accept-Datetime.",
            ),
        ]
        .into_response(),
        ("fetch", ..) if body["format"] == "page" => (
            [
                ("content-type", "text/html; charset=utf-8"),
                ("memento-datetime", AS_OF),
                ("x-fetch-status-code", "200"),
                ("x-fetch-status-message", "OK."),
            ],
            PAGE_HTML,
        )
            .into_response(),
        ("fetch", _, Some("https://news.example.com/energy/missing")) => {
            UNREAD_EXAMPLE.into_response()
        }
        ("fetch", ..) => FETCH_EXAMPLE.into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The fake API's base URL, with a trailing slash the client must tolerate, and its request log.
async fn fake_api() -> (String, Log) {
    let log = Log::default();
    let app = Router::new()
        .route("/v1/time-portal/{op}", post(handle))
        .with_state(log.clone());
    (spawn(app).await + "/", log)
}

fn first(log: &Log) -> Seen {
    log.lock().unwrap()[0].clone()
}

#[tokio::test]
async fn search_sends_the_instant_and_only_the_fields_set() {
    let (url, log) = fake_api().await;
    let mut req = SearchRequest::new("european battery gigafactory delays");
    req.num_results = Some(5);

    let found = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&req)
        .await
        .unwrap();

    let seen = first(&log);
    assert_eq!(seen.path, "/v1/time-portal/search");
    assert_eq!(seen.api_key.as_deref(), Some("good-key"));
    assert_eq!(seen.accept_datetime.as_deref(), Some(AS_OF));
    assert_eq!(
        seen.body,
        json!({"query": "european battery gigafactory delays", "num_results": 5})
    );
    let hit = &found.results[0];
    assert_eq!(
        hit.url,
        "https://news.example.com/energy/battery-plant-delay"
    );
    assert_eq!(hit.published_at.as_deref(), Some("2026-02-11T06:12:00Z"));
    assert_eq!(hit.text, None);
    assert_eq!(hit.domain.as_deref(), Some("example.com"));
    assert_eq!(hit.entity, None);
    assert_eq!(found.request_id, "req_9Zp4kL");
    assert!(found.entities.is_empty());
}

#[tokio::test]
async fn published_after_in_whole_utc_seconds_and_categories_and_extra_fields_by_name() {
    let (url, log) = fake_api().await;
    let mut req = SearchRequest::new("q");
    req.categories = vec![Category::News, Category::Other("patents".into())];
    req.extra.insert("rerank".into(), json!(true));
    req.published_after = Some(
        "2026-02-01T09:00:00.75+09:00"
            .parse::<DateTime<chrono::FixedOffset>>()
            .unwrap()
            .with_timezone(&Utc),
    );

    Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&req)
        .await
        .unwrap();

    assert_eq!(
        first(&log).body,
        json!({
            "query": "q",
            "published_after": "2026-02-01T00:00:00Z",
            "categories": ["news", "patents"],
            "rerank": true,
        })
    );
}

#[tokio::test]
async fn a_search_naming_an_entity_returns_it_on_its_world_page_result() {
    let (url, _) = fake_api().await;

    let found = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("nvidia"))
        .await
        .unwrap();

    let [page, news] = found.results.as_slice() else {
        panic!("{:?}", found.results);
    };
    let entity = page.entity.as_ref().unwrap();
    assert_eq!(entity.id, page.url);
    assert_eq!(entity.kind, "organization");
    assert_eq!(entity.schema_version, 1);
    assert_eq!(entity.properties, json!({"name": "NVIDIA"}));
    assert_eq!(news.entity, None);
    assert!(found.entities.is_empty());
}

#[test]
fn a_response_from_an_older_server_reads_without_entities() {
    let found: SearchResponse = serde_json::from_str(OLDER_SERVER_EXAMPLE).unwrap();

    assert_eq!(found.results[0].entity, None);
    assert!(found.entities.is_empty());
}

#[test]
fn responses_write_back_as_the_api_sent_them() {
    fn round_trip<T: serde::Serialize + serde::de::DeserializeOwned>(sent: &str) {
        let sent: Value = serde_json::from_str(sent).unwrap();
        let read: T = serde_json::from_value(sent.clone()).unwrap();
        assert_eq!(serde_json::to_value(read).unwrap(), sent);
    }
    round_trip::<SearchResponse>(SEARCH_EXAMPLE);
    round_trip::<SearchResponse>(ENTITY_EXAMPLE);
    round_trip::<FetchResponse>(FETCH_EXAMPLE);
    round_trip::<FetchResponse>(UNREAD_EXAMPLE);
}

#[tokio::test]
async fn fetch_reads_the_page_and_its_dates() {
    let (url, log) = fake_api().await;
    let mut req = FetchRequest::new("https://news.example.com/energy/battery-plant-delay");
    req.max_characters = Some(2000);

    let page = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .fetch(&req)
        .await
        .unwrap();

    let seen = first(&log);
    assert_eq!(seen.path, "/v1/time-portal/fetch");
    assert_eq!(
        seen.body,
        json!({"url": "https://news.example.com/energy/battery-plant-delay", "max_characters": 2000})
    );
    assert_eq!(
        page.final_url,
        "https://news.example.com/energy/battery-plant-delay/"
    );
    assert_eq!(page.author.as_deref(), Some("Jane Doe"));
    assert_eq!(page.status, PageStatus::Success);
    assert_eq!(page.page_error, None);
    assert_eq!(page.entity, None);
    assert!(page.truncated);
    assert!(page.content.starts_with("# Battery plant"));
}

#[tokio::test]
async fn a_page_that_could_not_be_read_is_an_answer_with_the_sites_status() {
    let (url, _) = fake_api().await;

    let page = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .fetch(&FetchRequest::new(
            "https://news.example.com/energy/missing",
        ))
        .await
        .unwrap();

    assert_eq!(page.status, PageStatus::Error);
    let error = page.page_error.unwrap();
    assert_eq!(error.http_status_code, Some(404));
    assert_eq!(error.to_string(), "Page not found. (HTTP 404)");
    assert!(page.content.is_empty());
}

#[tokio::test]
async fn page_returns_the_document_itself() {
    let (url, log) = fake_api().await;

    let doc = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .page("https://news.example.com/energy/battery-plant-delay")
        .await
        .unwrap();

    let seen = first(&log);
    assert_eq!(seen.path, "/v1/time-portal/fetch");
    assert_eq!(
        seen.body,
        json!({"url": "https://news.example.com/energy/battery-plant-delay", "format": "page"})
    );
    assert_eq!(doc.status, PageStatus::Success);
    assert_eq!(doc.page_error, None);
    assert_eq!(doc.body, PAGE_HTML.as_bytes());
    assert_eq!(
        doc.content_type.as_deref(),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(doc.memento_datetime.as_deref(), Some(AS_OF));
}

#[tokio::test]
async fn a_refusal_carries_the_api_error() {
    let (url, _) = fake_api().await;

    let err = Client::builder()
        .with_api_key("bad-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("q"))
        .await
        .unwrap_err();

    let Error::Api {
        status,
        kind,
        request_id,
        ..
    } = err
    else {
        panic!("expected an API error, got {err:?}");
    };
    assert_eq!((status, kind.as_str()), (401, "invalid_api_key"));
    assert_eq!(request_id.as_deref(), Some("req_9Zp4kL"));
}

#[tokio::test]
async fn a_page_the_site_failed_is_an_empty_document_with_the_sites_status() {
    let (url, _) = fake_api().await;

    let doc = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .page("https://news.example.com/missing")
        .await
        .unwrap();

    assert_eq!(doc.status, PageStatus::Error);
    let error = doc.page_error.unwrap();
    assert_eq!(error.http_status_code, Some(404));
    assert_eq!(error.to_string(), "Page not found. (HTTP 404)");
    assert!(doc.body.is_empty());
    assert_eq!(doc.content_type, None);
}

#[tokio::test]
async fn a_page_no_site_answered_has_no_status_code() {
    let (url, _) = fake_api().await;

    let doc = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .page("https://news.example.com/never")
        .await
        .unwrap();

    assert_eq!(doc.status, PageStatus::Error);
    let error = doc.page_error.unwrap();
    assert_eq!(error.http_status_code, None);
    assert_eq!(error.message, "No copy at or before Accept-Datetime.");
}

#[tokio::test]
async fn a_url_never_fetched_is_an_api_error_not_retried() {
    let (url, log) = fake_api().await;

    let err = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .fetch(&FetchRequest::new("https://x.example/home"))
        .await
        .unwrap_err();

    assert!(
        matches!(&err, Error::Api { status: 403, kind, .. } if kind == "url_not_allowed"),
        "{err:?}"
    );
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_proxy_failure_keeps_its_status() {
    let (url, _) = fake_api().await;

    let err = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("proxy down"))
        .await
        .unwrap_err();

    assert!(
        matches!(&err, Error::Api { status: 502, kind, request_id: None, .. } if kind == "unknown"),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_success_off_the_documented_shape_is_a_decode_error() {
    let (url, _) = fake_api().await;

    let err = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("garbled"))
        .await
        .unwrap_err();

    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}

#[tokio::test]
async fn a_redirect_to_another_origin_never_carries_the_key() {
    let (elsewhere, elsewhere_log) = fake_api().await;
    let target = format!("{elsewhere}v1/time-portal/search");
    let redirecting = Router::new().route(
        "/v1/time-portal/{op}",
        post(move || async move { (StatusCode::TEMPORARY_REDIRECT, [("location", target)]) }),
    );

    let result = Client::builder()
        .with_api_key("good-key")
        .with_base_url(spawn(redirecting).await)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("q"))
        .await;

    assert!(
        matches!(&result, Err(Error::Api { status: 307, .. })),
        "{result:?}"
    );
    assert!(
        elsewhere_log.lock().unwrap().is_empty(),
        "the redirect was followed"
    );
}

#[tokio::test]
async fn a_rate_limit_says_when_to_retry() {
    let (url, _) = fake_api().await;

    let err = Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(as_of())
        .search(&SearchRequest::new("too fast"))
        .await
        .unwrap_err();

    assert!(
        matches!(&err, Error::Api { status: 429, retry_after: Some(d), .. } if *d == Duration::from_secs(7)),
        "{err:?}"
    );
}

#[tokio::test]
async fn the_instant_goes_out_as_utc_in_whole_seconds() {
    let (url, log) = fake_api().await;
    let instant: DateTime<Utc> = "2026-03-01T05:30:00.250+05:30".parse().unwrap();

    Client::builder()
        .with_api_key("good-key")
        .with_base_url(url)
        .build()
        .unwrap()
        .time_portal(instant)
        .search(&SearchRequest::new("q"))
        .await
        .unwrap();

    assert_eq!(
        first(&log).accept_datetime.as_deref(),
        Some("2026-03-01T00:00:00Z")
    );
}
