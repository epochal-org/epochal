//! `/v1/time-portal`: the web, and Linux sandboxes, held to a past instant. Every call is a method
//! of [`TimePortal`], from [`Client::time_portal`].

use chrono::{DateTime, Utc};
use reqwest::{Method, RequestBuilder};

use crate::client::{Client, with_as_of};

pub mod entities;
pub mod sandbox;
pub mod web;

const PREFIX: &str = "/v1/time-portal";

/// The time portal held to one instant, from [`Client::time_portal`]: search, fetch and new boxes
/// are held to [`as_of`](Self::as_of). Calls on an existing box take its id, and the box keeps the
/// instant it was made with. Cheap to clone; it shares the client's connection pool.
#[derive(Clone, Debug)]
pub struct TimePortal {
    client: Client,
    as_of: DateTime<Utc>,
}

impl Client {
    /// The time portal held to `as_of`: web search, web fetch and sandboxes as they stood then.
    pub fn time_portal(&self, as_of: DateTime<Utc>) -> TimePortal {
        TimePortal {
            client: self.clone(),
            as_of,
        }
    }
}

impl TimePortal {
    /// The client its calls go through.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The instant search, fetch and new boxes are held to.
    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client.request(method, &format!("{PREFIX}{path}"))
    }

    /// A request held to [`as_of`](Self::as_of) by `Accept-Datetime`.
    fn held(&self, method: Method, path: &str) -> RequestBuilder {
        with_as_of(self.request(method, path), self.as_of)
    }
}
