//! What the box's harness is served from this machine on the relay's port: `/mcp` from [`Tools`],
//! and a model proxy for the rest. The box's model requests go out from this machine to the user's
//! endpoint with the user's key; responses come back scrubbed of the present.

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use epochal_mcp::Tools;
use futures_util::{StreamExt, stream};
use reqwest::{Body as UpstreamBody, Client, Response as UpstreamResponse, Url};

use crate::sampler::{self, Api};
use crate::session::scrub::{self, Scrub};

/// Request headers the endpoint gets from the harness; auth is replaced, the rest dropped.
const FORWARDED: [&str; 5] = [
    "content-type",
    "accept",
    "anthropic-version",
    "anthropic-beta",
    "openai-beta",
];
const API_KEY_HEADER: &str = "x-api-key";
const MCP: &str = "/mcp";
/// The version prefix every harness's request path carries itself.
const VERSION_PREFIX: &str = "/v1";
const MESSAGES: &str = "/v1/messages";
const JSON: &str = "application/json";
/// The only calls the user's key is lent to: a gateway's admin routes share its base.
const MODEL_CALLS: [&str; 3] = [MESSAGES, "/v1/messages/count_tokens", "/v1/responses"];
const NOT_A_MODEL_CALL: &str =
    r#"{"error":{"type":"not_found","message":"only model calls are forwarded"}}"#;

/// What the box sees of a failure: the error itself may carry the endpoint's URL.
const PROXY_ERROR: &str =
    r#"{"error":{"type":"proxy_error","message":"the model endpoint failed"}}"#;

/// The model endpoint and its key, read on this machine; neither enters the box.
#[derive(Clone)]
pub struct Upstream {
    base: Url,
    key: String,
}

impl Upstream {
    /// The endpoint the built-in harness would use, from the same variables.
    pub fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Upstream> {
        let (base, key) = sampler::endpoint(var)?;
        Ok(Upstream { base, key })
    }
}

#[derive(Clone)]
pub struct Endpoint {
    http: Client,
    upstream: Upstream,
    /// Every epoch timestamp in a response becomes this one.
    as_of_epoch: i64,
}

impl Endpoint {
    pub fn new(upstream: Upstream, as_of_epoch: i64) -> Result<Self> {
        Ok(Endpoint {
            http: sampler::http()?,
            upstream,
            as_of_epoch,
        })
    }
}

pub fn router(endpoint: Endpoint, tools: Tools) -> Router {
    Router::new()
        .nest_service(MCP, tools.http_router())
        .fallback(forward)
        .with_state(endpoint)
}

/// One model call from the box. A failure after the head has gone ends the stream with an error,
/// which the harness sees as a dropped response.
async fn forward(State(endpoint): State<Endpoint>, request: Request) -> Response {
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_owned();
    if request.method() != Method::POST || !MODEL_CALLS.contains(&request.uri().path()) {
        tracing::warn!(method = %request.method(), %path, "refused a non-model request from the box");
        return (
            StatusCode::NOT_FOUND,
            [(CONTENT_TYPE, JSON)],
            NOT_A_MODEL_CALL,
        )
            .into_response();
    }
    let resp = match endpoint.send(&path, request).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(%path, %e, "model request failed");
            return (StatusCode::BAD_GATEWAY, [(CONTENT_TYPE, JSON)], PROXY_ERROR).into_response();
        }
    };
    tracing::debug!(%path, status = resp.status().as_u16(), "model response");
    let mut out = Response::builder().status(resp.status().as_u16());
    for (n, v) in scrub::headers(
        resp.headers()
            .iter()
            .filter_map(|(n, v)| Some((n.as_str(), v.to_str().ok()?))),
    ) {
        out = out.header(n, v);
    }
    let start = (Some(resp.bytes_stream()), Scrub::new(endpoint.as_of_epoch));
    let scrubbed = stream::unfold(start, |(upstream, mut body)| async move {
        let mut upstream = upstream?;
        match upstream.next().await {
            Some(Ok(chunk)) => Some((Ok(body.push(&chunk)), (Some(upstream), body))),
            Some(Err(e)) => {
                tracing::warn!(%e, "model response failed midway");
                Some((Err(e), (None, body)))
            }
            None => Some((Ok(body.finish()), (None, body))),
        }
    });
    out.body(Body::from_stream(scrubbed))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

impl Endpoint {
    /// Where `path`, one of [`MODEL_CALLS`] and its query, goes, and the API it speaks: a
    /// harness's wire format is its own, so the path, not the model, says how to authenticate.
    fn url(&self, path: &str) -> Result<(Url, Api)> {
        let api = if path.starts_with(MESSAGES) {
            Api::Anthropic
        } else {
            Api::OpenAi
        };
        let rest = path.strip_prefix(VERSION_PREFIX).unwrap_or(path);
        Ok((
            Url::parse(&format!("{}{rest}", api.root(&self.upstream.base)))?,
            api,
        ))
    }

    async fn send(&self, path: &str, request: Request) -> Result<UpstreamResponse> {
        let (url, api) = self.url(path)?;
        let (head, body) = request.into_parts();
        let req = self.http.request(head.method, url);
        let key = &self.upstream.key;
        let mut req = match api {
            Api::Anthropic => req.header(API_KEY_HEADER, key),
            Api::OpenAi => req.bearer_auth(key),
        }
        .body(UpstreamBody::wrap_stream(body.into_data_stream()));
        for (n, v) in &head.headers {
            if FORWARDED.iter().any(|f| f.eq_ignore_ascii_case(n.as_str())) {
                req = req.header(n, v);
            }
        }
        Ok(req.send().await?)
    }
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
