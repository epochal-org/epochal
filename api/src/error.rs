use std::error::Error as StdError;
use std::time::Duration;

use reqwest::header::RETRY_AFTER;
use reqwest::{Response, StatusCode};
use serde::Deserialize;

use crate::client::API_KEY_ENV;

/// The `kind` of an error body that is not the API's error envelope, such as a proxy's.
const UNKNOWN_KIND: &str = "unknown";

/// A failed request.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No API key was given and `EPOCHAL_API_KEY` is unset or empty.
    #[error("no API key: call with_api_key or set {API_KEY_ENV}")]
    MissingApiKey,
    /// An argument the API can't take, refused before any request.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The HTTP client did not build, or the request got no complete response: connect,
    /// timeout or transport failure.
    #[error("request failed: {0}")]
    Transport(#[source] Box<dyn StdError + Send + Sync>),
    /// The API, or a proxy in front of it, refused the request.
    #[error("{message} (HTTP {status})")]
    #[non_exhaustive]
    Api {
        /// The HTTP status.
        status: u16,
        /// A stable machine-readable code such as `invalid_api_key` or `conflict`; `unknown`
        /// when the body is not the API's error envelope.
        kind: String,
        /// What went wrong, in one sentence.
        message: String,
        /// Identifies the request in Epochal's logs, if the API assigned one.
        request_id: Option<String>,
        /// How long to wait before retrying, from `Retry-After`, when the API says.
        retry_after: Option<Duration>,
        /// For an out-of-order [`TimePortal::send_exec_input`](crate::time_portal::TimePortal::send_exec_input),
        /// the `seq` the command expects next.
        next_seq: Option<u64>,
    },
    /// A successful response whose body did not match the documented shape.
    #[error("unreadable response: {0}")]
    Decode(#[from] serde_json::Error),
}

/// A request that could not be built, such as a malformed base URL or header, is the caller's;
/// anything else is the connection's.
pub(crate) fn transport(e: reqwest::Error) -> Error {
    if e.is_builder() {
        return Error::InvalidArgument(e.to_string());
    }
    Error::Transport(Box::new(e))
}

/// A non-2xx response as [`Error::Api`].
pub(crate) async fn failure(resp: Response) -> Error {
    let status = resp.status();
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok()?.trim().parse().ok())
        .map(Duration::from_secs);
    api_error(status, retry_after, &resp.bytes().await.unwrap_or_default())
}

fn api_error(status: StatusCode, retry_after: Option<Duration>, body: &[u8]) -> Error {
    #[derive(Deserialize)]
    struct Envelope {
        error: Body,
    }
    #[derive(Deserialize)]
    struct Body {
        #[serde(rename = "type")]
        kind: String,
        message: String,
        #[serde(default)]
        request_id: Option<String>,
        #[serde(default)]
        next_seq: Option<u64>,
    }
    let (kind, message, request_id, next_seq) = match serde_json::from_slice::<Envelope>(body) {
        Ok(Envelope { error }) => (error.kind, error.message, error.request_id, error.next_seq),
        Err(_) => {
            let reason = status.canonical_reason().unwrap_or("request failed");
            (UNKNOWN_KIND.to_owned(), reason.to_owned(), None, None)
        }
    };
    Error::Api {
        status: status.as_u16(),
        kind,
        message,
        request_id,
        retry_after,
        next_seq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_envelope_names_the_kind_and_next_seq() {
        let body = br#"{"error":{"type":"conflict","message":"out of order","request_id":"r","next_seq":7}}"#;
        let err = api_error(StatusCode::CONFLICT, None, body);
        assert!(
            matches!(&err, Error::Api { status: 409, kind, next_seq: Some(7), .. } if kind == "conflict"),
            "{err:?}"
        );
    }

    #[test]
    fn a_body_off_the_envelope_keeps_its_status() {
        let err = api_error(
            StatusCode::BAD_GATEWAY,
            Some(Duration::from_secs(3)),
            b"<html>",
        );
        assert!(
            matches!(&err, Error::Api { status: 502, kind, retry_after: Some(d), .. }
                if kind == "unknown" && *d == Duration::from_secs(3)),
            "{err:?}"
        );
    }
}
