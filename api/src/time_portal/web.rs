//! `POST /v1/time-portal/{search,fetch}`: the web as it read at a past instant — search results,
//! a page as markdown, or the archived document itself.

use std::fmt;
use std::ops::Not;

use chrono::{DateTime, Utc};
use reqwest::Method;
use reqwest::header::{CONTENT_TYPE, HeaderMap};
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{self, with_json};
use crate::error::{Error, transport};
use crate::time_portal::TimePortal;
use crate::time_portal::entities::TypedEntity;

const SEARCH: &str = "/search";
const FETCH: &str = "/fetch";
const MEMENTO_DATETIME: &str = "memento-datetime";
const FETCH_STATUS_CODE: &str = "x-fetch-status-code";
const FETCH_STATUS_MESSAGE: &str = "x-fetch-status-message";
/// The site's code when no site answered: unreachable, or no copy at the as-of instant.
const NO_SITE: u16 = 0;
/// The message when a server names no reason for an unread page.
const UNREAD_MESSAGE: &str = "The page could not be read.";
const PAGE_FORMAT: &str = "page";

/// `POST /v1/time-portal/search`: web search as it would have answered at the as-of instant.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct SearchRequest {
    /// What to search the web for.
    pub query: String,
    /// The most results to return, 1 to 10. The server defaults to 10.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_results: Option<u32>,
    /// Only return pages from these sites, subdomains included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub domains: Vec<String>,
    /// Never return pages from these sites, subdomains included.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,
    /// Preferred result languages as BCP 47 tags, most preferred first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
    /// Only return pages published at or after this instant.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "optional_instant"
    )]
    pub published_after: Option<DateTime<Utc>>,
    /// Only return pages in these categories; empty searches everything.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<Category>,
    /// Also return each page's full text in [`SearchResult::text`].
    #[serde(skip_serializing_if = "Not::not")]
    pub include_text: bool,
    /// Fields this version doesn't model, sent in the body as given; a key must not name a
    /// modeled field.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl SearchRequest {
    /// A search for `query` with the server's defaults for everything else.
    pub fn new(query: impl Into<String>) -> Self {
        SearchRequest {
            query: query.into(),
            num_results: None,
            domains: Vec::new(),
            exclude_domains: Vec::new(),
            languages: Vec::new(),
            published_after: None,
            categories: Vec::new(),
            include_text: false,
            extra: serde_json::Map::new(),
        }
    }
}

/// A kind of page a [`SearchRequest`] can be narrowed to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Category {
    /// News articles.
    News,
    /// SEC filings.
    Finance,
    /// Court opinions, statutes, regulations and sanctions.
    Legal,
    /// Academic articles.
    Publications,
    /// Company profiles.
    Companies,
    /// People profiles.
    People,
    /// Places.
    Maps,
    /// A category this version doesn't name, sent as given.
    #[serde(untagged)]
    Other(String),
}

fn optional_instant<S: Serializer>(at: &Option<DateTime<Utc>>, s: S) -> Result<S::Ok, S::Error> {
    match at {
        Some(at) => s.serialize_str(&client::instant(*at)),
        None => s.serialize_none(),
    }
}

/// The answer to a [`SearchRequest`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchResponse {
    /// The instant the search was held to.
    pub as_of: String,
    /// Identifies the request in Epochal's logs; quote it when reporting a problem.
    pub request_id: String,
    /// Ranked results, best first.
    pub results: Vec<SearchResult>,
    /// Reserved; entities come on [`SearchResult::entity`].
    #[serde(default)]
    pub entities: Vec<Entity>,
}

/// One search result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SearchResult {
    /// The page's title.
    pub title: String,
    /// The page's canonical URL.
    pub url: String,
    /// A short passage from the page that matches the query.
    pub snippet: String,
    /// The page's extracted text, when [`SearchRequest::include_text`] was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// When the page was published, if known.
    #[serde(default)]
    pub published_at: Option<String>,
    /// The page's registrable domain, if known.
    #[serde(default)]
    pub domain: Option<String>,
    /// The page's language as a BCP 47 tag, if detected.
    #[serde(default)]
    pub language: Option<String>,
    /// The entity this `https://world.epochal.com/` result is the page of, as fetch returns it for
    /// `url`; `None` for every other result.
    #[serde(default)]
    pub entity: Option<Entity>,
}

/// `POST /v1/time-portal/fetch`: one page as it read at the as-of instant, as markdown with its
/// metadata. For the document itself, use [`TimePortal::page`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct FetchRequest {
    /// The page to read: a full `http` or `https` URL.
    pub url: String,
    /// The most characters of content to return. The server defaults to 100,000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_characters: Option<u32>,
    /// Fields this version doesn't model, sent in the body as given; a key must not name a
    /// modeled field.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl FetchRequest {
    /// A fetch of `url` with the server's defaults for everything else.
    pub fn new(url: impl Into<String>) -> Self {
        FetchRequest {
            url: url.into(),
            max_characters: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// The answer to a [`FetchRequest`]. A page that could not be read is still an answer, and is
/// billed: [`status`](Self::status) is [`PageStatus::Error`], [`page_error`](Self::page_error)
/// says why, and `content` is empty.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FetchResponse {
    /// The instant the read was held to.
    pub as_of: String,
    /// Identifies the request in Epochal's logs; quote it when reporting a problem.
    pub request_id: String,
    /// Whether the page was read.
    #[serde(default)]
    pub status: PageStatus,
    /// Why the page could not be read; `None` when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_error: Option<PageError>,
    /// Identifies the exact stored version served, if known.
    #[serde(default)]
    pub id: Option<String>,
    /// The URL as requested.
    pub url: String,
    /// The URL the content was read from, after redirects and normalization.
    pub final_url: String,
    /// The page's title.
    pub title: String,
    /// The page's own summary, from its metadata.
    #[serde(default)]
    pub description: Option<String>,
    /// The page's authors, comma-separated.
    #[serde(default)]
    pub author: Option<String>,
    /// When the page says it was first published.
    #[serde(default)]
    pub published_at: Option<String>,
    /// The page's language as a BCP 47 tag, if detected.
    #[serde(default)]
    pub language: Option<String>,
    /// The page's extracted text as markdown.
    pub content: String,
    /// Whether `max_characters` cut `content` short.
    pub truncated: bool,
    /// The entity this `https://world.epochal.com/` URL is the page of; `None` for ordinary pages.
    #[serde(default)]
    pub entity: Option<Entity>,
}

/// Whether a [`FetchResponse`]'s page was read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PageStatus {
    /// Read; `content` holds it.
    #[default]
    Success,
    /// Not read; [`FetchResponse::page_error`] says why.
    Error,
    /// A status from a newer server.
    #[serde(other)]
    Unknown,
}

/// Why a page could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PageError {
    /// The HTTP status the site answered with, the one reading the page yourself would get.
    /// `None` when no site answered: the host could not be reached, or no copy is archived at the
    /// as-of instant.
    #[serde(default)]
    pub http_status_code: Option<u16>,
    /// A short reason, for people; branch on `http_status_code`.
    pub message: String,
}

/// The message, then the site's status when one answered: `Page not found. (HTTP 404)`.
impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        match self.http_status_code {
            Some(code) => write!(f, " (HTTP {code})"),
            None => Ok(()),
        }
    }
}

/// A company, person, place, crypto asset or city's weather, with the properties the API holds for
/// it. [`typed`](Self::typed) reads `properties` into the model of its type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Entity {
    /// Its stable URL, `https://world.epochal.com/<type>/<key>`.
    pub id: String,
    /// What it is, such as `organization`, `person`, `place`, `crypto` or `weather`; newer servers
    /// may name more.
    #[serde(rename = "type")]
    pub kind: String,
    /// The version of `properties`' layout for this `kind`.
    pub schema_version: u32,
    /// When `properties` were last refreshed.
    pub updated_at: String,
    /// Its properties, laid out as the API reference describes for `kind` and `schema_version`.
    pub properties: serde_json::Value,
}

impl Entity {
    /// `properties` read into the model of [`kind`](Self::kind) at layout
    /// [`schema_version`](Self::schema_version), or [`TypedEntity::Unknown`] for a type or
    /// layout this version doesn't model. Errs when they don't match the documented layout;
    /// Python's `Entity.typed` raises `DecodeError` instead.
    ///
    /// The result holds `properties` only: `id`, `kind`, `schema_version` and `updated_at` stay on
    /// `self`. Fields this version doesn't model are left out of it; `properties` keeps them.
    pub fn typed(&self) -> Result<TypedEntity, serde_json::Error> {
        TypedEntity::read(&self.kind, self.schema_version, &self.properties)
    }
}

/// A page as the document itself, from [`TimePortal::page`]. A page that could not be read is still
/// a document, and is billed: [`status`](Self::status) is [`PageStatus::Error`],
/// [`page_error`](Self::page_error) says why, and `body` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Document {
    /// Whether the page was read, from `x-fetch-status-code`.
    pub status: PageStatus,
    /// Why the page could not be read, from `x-fetch-status-code` and `x-fetch-status-message`;
    /// `None` when it was.
    pub page_error: Option<PageError>,
    /// The document's media type, such as `text/html; charset=utf-8`, when the archive knows it.
    pub content_type: Option<String>,
    /// The instant the document was held to, from `Memento-Datetime`.
    pub memento_datetime: Option<String>,
    /// The document's bytes.
    pub body: Vec<u8>,
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    Some(headers.get(name)?.to_str().ok()?.to_owned())
}

/// The page's outcome from its `x-fetch-status-*` headers. A server that sends none read it.
fn page_outcome(headers: &HeaderMap) -> (PageStatus, Option<PageError>) {
    let Some(code) = header(headers, FETCH_STATUS_CODE) else {
        return (PageStatus::Success, None);
    };
    let Ok(code) = code.trim().parse::<u16>() else {
        return (PageStatus::Unknown, None);
    };
    if (200..300).contains(&code) {
        return (PageStatus::Success, None);
    }
    let error = PageError {
        http_status_code: (code != NO_SITE).then_some(code),
        message: header(headers, FETCH_STATUS_MESSAGE).unwrap_or_else(|| UNREAD_MESSAGE.into()),
    };
    (PageStatus::Error, Some(error))
}

#[derive(Serialize)]
struct PageRequest<'a> {
    url: &'a str,
    format: &'static str,
}

impl TimePortal {
    /// Web search as it would have answered at [`as_of`](Self::as_of).
    pub async fn search(&self, request: &SearchRequest) -> Result<SearchResponse, Error> {
        let http = with_json(self.held(Method::POST, SEARCH), request)?;
        self.client.json(http).await
    }

    /// One page as it read at [`as_of`](Self::as_of), as markdown with its metadata. A page that
    /// could not be read is `Ok`, with [`FetchResponse::status`] [`PageStatus::Error`].
    pub async fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse, Error> {
        let http = with_json(self.held(Method::POST, FETCH), request)?;
        self.client.json(http).await
    }

    /// `url` as the document itself at `as_of` (`fetch` with `format: page`): the archived bytes,
    /// with nothing learned after `as_of` left in them. A page that could not be read is `Ok`,
    /// with [`Document::status`] [`PageStatus::Error`].
    pub async fn page(&self, url: &str) -> Result<Document, Error> {
        let body = PageRequest {
            url,
            format: PAGE_FORMAT,
        };
        let http = with_json(self.held(Method::POST, FETCH), &body)?;
        let resp = self.client.send(http).await?;
        let (status, page_error) = page_outcome(resp.headers());
        Ok(Document {
            status,
            page_error,
            content_type: header(resp.headers(), CONTENT_TYPE.as_str()),
            memento_datetime: header(resp.headers(), MEMENTO_DATETIME),
            body: resp.bytes().await.map_err(transport)?.into(),
        })
    }
}
