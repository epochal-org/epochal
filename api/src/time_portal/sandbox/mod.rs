//! `/v1/time-portal/sandbox`: boxes. Each method is one call; waiting for a
//! box to come up, and anything else that polls, is the caller's.

use std::collections::BTreeMap;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::client::{with_idempotency_key, with_json};
use crate::error::Error;
use crate::time_portal::TimePortal;
use crate::wire::{deserialize_duration, duration_param, segment, serialize_duration};

mod exec;
mod files;

pub use crate::client::idempotency_key;
pub use exec::{
    ExecExit, ExecFrame, ExecInput, ExecOutput, ExecOutputRequest, ExecStartRequest, Stream,
};
pub use files::{FileList, FileListRequest, FileRange, FileStat, FileType};

const SANDBOXES: &str = "/sandbox";

/// What a box may reach on the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Network {
    /// Nothing: names do not resolve and no connection leaves the box.
    None,
    /// HTTP and HTTPS to the web as it stood at the box's `as_of`, through the time portal, and
    /// package mirrors held to it. Each page read is billed.
    TimePortal,
    /// A mode from a newer server; only ever read, never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

/// `POST /sandbox`: a box to create, from `SandboxCreateRequest::default()` then its fields.
/// Anything not set takes the API's default: no network, 1 vCPU, 2 GiB, a 1 h lifetime. The box
/// boots the newest Ubuntu build dated on or before its `as_of`.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SandboxCreateRequest {
    /// What the box may reach.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<Network>,
    /// Virtual CPUs reserved for the box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpus: Option<u32>,
    /// Memory reserved for the box, in GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_gib: Option<u32>,
    /// The box ends this long after it boots; a positive whole number of seconds.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_duration"
    )]
    pub timeout: Option<Duration>,
    /// The box ends after this long with no command running; a positive whole number of seconds.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_duration"
    )]
    pub idle_timeout: Option<Duration>,
    /// Variables in the environment of every command in the box. Values are not logged.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Labels kept with the box and returned with it.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Apps mounted read-only at `/opt/epochal/bin`, by name. An app is a current build, not one as
    /// of `as_of`, so a box that uses one is not strictly held to its instant.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    /// Sent as `Idempotency-Key`: a create retried with the same key makes one box. Unset, each
    /// call sends a fresh key, so only a retry that sets the first call's key is covered.
    #[serde(skip)]
    pub idempotency_key: Option<String>,
}

/// A box's lifecycle state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SandboxState {
    /// Accepted and booting.
    Creating,
    /// Booted and accepting calls.
    Live,
    /// Terminated, timed out or idle; its files and command output are gone.
    Ended,
    /// Never booted; [`SandboxInfo::error`] says why.
    Failed,
    /// A state from a newer server; only ever read, never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

/// Why a box failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ApiFailure {
    /// A stable machine-readable code, such as `lifetime_expired`, `idle_expired`,
    /// `out_of_memory`, `sandbox_lost` or `spend_limit_reached`.
    #[serde(rename = "type")]
    pub kind: String,
    /// What went wrong, in one sentence.
    pub message: String,
}

/// A box as the API describes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxInfo {
    /// The box's id.
    pub id: String,
    /// Where it is in its life.
    pub state: SandboxState,
    /// The instant its clock and network are held to, RFC 3339.
    pub as_of: String,
    /// What it may reach.
    pub network: Network,
    /// Reserved virtual CPUs.
    pub vcpus: u32,
    /// Reserved memory in GiB.
    pub memory_gib: u32,
    /// Its lifetime from boot.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub timeout: Duration,
    /// How long it may sit idle.
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration"
    )]
    pub idle_timeout: Duration,
    /// When the create was accepted.
    pub created_at: String,
    /// When it booted; billing starts here.
    #[serde(default)]
    pub booted_at: Option<String>,
    /// When it will end at the latest: `booted_at + timeout`.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// When it ended.
    #[serde(default)]
    pub ended_at: Option<String>,
    /// Why it failed or ended, when the API says.
    #[serde(default)]
    pub error: Option<ApiFailure>,
    /// The labels it was created with.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// The apps it got.
    #[serde(default)]
    pub apps: Vec<SandboxApp>,
}

/// An app on a box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxApp {
    /// The app's name.
    pub name: String,
    /// The version the box got.
    pub version: String,
}

/// `GET /sandbox`: one page of boxes, from `SandboxListRequest::default()` then its fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SandboxListRequest {
    /// Only boxes in this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<SandboxState>,
    /// Where the last page ended: its [`SandboxList::next_cursor`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The most boxes on the page, 1 to 100.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// A page of boxes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SandboxList {
    /// The boxes.
    pub sandboxes: Vec<SandboxInfo>,
    /// Where the next page starts; `None` on the last page.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

impl TimePortal {
    /// Creates a box held to [`as_of`](Self::as_of). It is usually still
    /// [`SandboxState::Creating`]; wait for it with [`get_sandbox`](Self::get_sandbox).
    pub async fn create_sandbox(
        &self,
        request: &SandboxCreateRequest,
    ) -> Result<SandboxInfo, Error> {
        let key = request
            .idempotency_key
            .clone()
            .unwrap_or_else(idempotency_key);
        let http = with_idempotency_key(self.held(Method::POST, SANDBOXES), &key);
        self.client.json(with_json(http, request)?).await
    }

    /// A box as it is now, or, with `wait`, once it leaves [`SandboxState::Creating`] or `wait`
    /// passes, whichever is first. The API holds a wait for at most
    /// [`MAX_WAIT`](crate::MAX_WAIT).
    pub async fn get_sandbox(
        &self,
        id: &str,
        wait: Option<Duration>,
    ) -> Result<SandboxInfo, Error> {
        let mut http = self.request(Method::GET, &sandbox_path(id, "")?);
        if let Some(wait) = wait {
            http = http.query(&[("wait", duration_param(wait)?)]);
        }
        self.client.json(http).await
    }

    /// One page of the account's boxes.
    pub async fn list_sandboxes(&self, request: &SandboxListRequest) -> Result<SandboxList, Error> {
        if request.state == Some(SandboxState::Unknown) {
            return Err(Error::InvalidArgument(
                "SandboxState::Unknown is not a filter".into(),
            ));
        }
        let http = self.request(Method::GET, SANDBOXES).query(request);
        self.client.json(http).await
    }

    /// Ends a box now. Terminating an ended box succeeds.
    pub async fn terminate_sandbox(&self, id: &str) -> Result<(), Error> {
        self.client
            .send(self.request(Method::DELETE, &sandbox_path(id, "")?))
            .await?;
        Ok(())
    }
}

/// `rest` under the box's path, its id escaped as one segment.
fn sandbox_path(id: &str, rest: &str) -> Result<String, Error> {
    Ok(format!("{SANDBOXES}/{}{rest}", segment(id)?))
}
