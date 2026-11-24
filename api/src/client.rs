use std::env;
use std::fmt;
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};

use reqwest::header::{CONTENT_TYPE, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Method, RequestBuilder, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, failure, transport};

const DEFAULT_BASE_URL: &str = "https://api.epochal.com";
pub(crate) const API_KEY_ENV: &str = "EPOCHAL_API_KEY";
const BASE_URL_ENV: &str = "EPOCHAL_BASE_URL";

const AGENT: &str = concat!("epochal-api/", env!("CARGO_PKG_VERSION"));
const API_KEY_HEADER: &str = "x-api-key";
const ACCEPT_DATETIME: &str = "accept-datetime";
const IDEMPOTENCY_KEY: &str = "idempotency-key";
const JSON: &str = "application/json";

/// The longest the API holds a call that waits, such as
/// [`TimePortal::get_sandbox`](crate::time_portal::TimePortal::get_sandbox) with a `wait`.
pub const MAX_WAIT: Duration = Duration::from_secs(55);

/// The longest the API takes to answer any call: a file write's deadline, past [`MAX_WAIT`] plus
/// the server's own slack on a wait.
const LONGEST_ANSWER: Duration = Duration::from_secs(80);

/// An Epochal API client, from [`Client::builder`]. Each API module is a handle from it, such as
/// [`time_portal`](Self::time_portal). Cheap to clone; clones share one connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

/// Configures a [`Client`]. Anything not set takes its default when [`build`](Self::build) runs.
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
}

impl ClientBuilder {
    /// The API key. Defaults to `EPOCHAL_API_KEY`.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Where requests go. Defaults to `EPOCHAL_BASE_URL`, then `https://api.epochal.com`.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// The client, with defaults for anything not set.
    pub fn build(self) -> Result<Client, Error> {
        self.build_with(|name| env::var(name).ok())
    }

    fn build_with(self, var: impl Fn(&str) -> Option<String>) -> Result<Client, Error> {
        let or_env =
            |value: Option<String>, name| value.or_else(|| var(name)).filter(|v| !v.is_empty());
        let api_key = or_env(self.api_key, API_KEY_ENV).ok_or(Error::MissingApiKey)?;
        let base_url =
            or_env(self.base_url, BASE_URL_ENV).unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(LONGEST_ANSWER + Duration::from_secs(5))
            .build()
            .map_err(transport)?;
        Ok(Client {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            api_key,
        })
    }
}

impl Client {
    /// A builder for a client; `Client::builder().build()` takes every default. The client never
    /// follows redirects, which would carry the key elsewhere, gives up connecting after 10 s, and
    /// gives up on a response that sends nothing for 85 s, 5 s past the longest the API takes to
    /// answer (a file write).
    pub fn builder() -> ClientBuilder {
        ClientBuilder {
            api_key: None,
            base_url: None,
        }
    }

    /// A request to `path` on the API, carrying the key.
    pub(crate) fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .header(API_KEY_HEADER, &self.api_key)
            .header(USER_AGENT, AGENT)
    }

    /// The response, whatever its status.
    pub(crate) async fn send_any(&self, request: RequestBuilder) -> Result<Response, Error> {
        request.send().await.map_err(transport)
    }

    /// The response if it succeeded, its [`Error::Api`] if not.
    pub(crate) async fn send(&self, request: RequestBuilder) -> Result<Response, Error> {
        let resp = self.send_any(request).await?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(failure(resp).await)
    }

    /// A successful response's body, decoded.
    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<T, Error> {
        let body = self.send(request).await?.bytes().await.map_err(transport)?;
        Ok(serde_json::from_slice(&body)?)
    }
}

/// An instant as the API reads one: RFC 3339 in whole seconds, UTC.
pub(crate) fn instant(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Holds `request` to `as_of` through `Accept-Datetime`.
pub(crate) fn with_as_of(request: RequestBuilder, as_of: DateTime<Utc>) -> RequestBuilder {
    request.header(ACCEPT_DATETIME, instant(as_of))
}

/// Marks `request` with `key`, so a retry the API already applied is applied once.
pub(crate) fn with_idempotency_key(request: RequestBuilder, key: &str) -> RequestBuilder {
    request.header(IDEMPOTENCY_KEY, key)
}

/// A fresh random idempotency key, which says nothing about the time it was made. A call that
/// sets none sends one of these; set your own to send the same one again on a retry.
pub fn idempotency_key() -> String {
    let random = || RandomState::new().hash_one(0u8);
    format!("{:016x}{:016x}", random(), random())
}

/// `request` with `body` as JSON; a value the API can't take is the caller's error.
pub(crate) fn with_json(
    request: RequestBuilder,
    body: &impl Serialize,
) -> Result<RequestBuilder, Error> {
    let body = serde_json::to_vec(body).map_err(|e| Error::InvalidArgument(e.to_string()))?;
    Ok(request.header(CONTENT_TYPE, JSON).body(body))
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn debug_output_never_shows_the_key() {
        let builder = Client::builder().with_api_key("sk-secret-123");
        assert!(!format!("{builder:?}").contains("sk-secret-123"));
        let shown = format!("{:?}", builder.build_with(env(&[])).unwrap());
        assert!(!shown.contains("sk-secret-123"), "{shown}");
        assert!(shown.contains("https://api.epochal.com"), "{shown}");
    }

    #[test]
    fn a_key_is_required_from_the_builder_or_the_environment() {
        let no_key = Client::builder().build_with(env(&[]));
        assert!(matches!(no_key, Err(Error::MissingApiKey)));
        let empty = Client::builder().build_with(env(&[(API_KEY_ENV, "")]));
        assert!(matches!(empty, Err(Error::MissingApiKey)));
        let from_env = Client::builder()
            .build_with(env(&[(API_KEY_ENV, "k")]))
            .unwrap();
        assert_eq!(from_env.api_key, "k");
    }

    #[test]
    fn defaults_come_from_the_environment_then_production() {
        let c = Client::builder()
            .build_with(env(&[(API_KEY_ENV, "k")]))
            .unwrap();
        assert_eq!(c.base_url, "https://api.epochal.com");
        let vars = [(API_KEY_ENV, "k"), (BASE_URL_ENV, "http://localhost:8080/")];
        let c = Client::builder().build_with(env(&vars)).unwrap();
        assert_eq!(c.base_url, "http://localhost:8080");
    }

    #[test]
    fn what_the_builder_sets_wins_over_the_environment() {
        let vars = [
            (API_KEY_ENV, "env-key"),
            (BASE_URL_ENV, "http://env.example"),
        ];
        let c = Client::builder()
            .with_api_key("set-key")
            .with_base_url("http://set.example/")
            .build_with(env(&vars))
            .unwrap();
        assert_eq!(
            (c.api_key.as_str(), c.base_url.as_str()),
            ("set-key", "http://set.example")
        );
    }
}
