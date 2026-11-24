use std::fmt;
use std::time::Duration;

use epochal_api::chrono::{DateTime, Utc};
use epochal_api::time_portal::TimePortal;
use epochal_api::time_portal::sandbox::{
    Network, SandboxCreateRequest, SandboxInfo, SandboxState, idempotency_key,
};
use epochal_api::{Client, MAX_WAIT};

use crate::Error;
use crate::files::Files;
use crate::retry::retry;
use crate::tunnel::RELAY_APP;

/// A live box, from [`Sandbox::time_portal`]. Call [`terminate`](Self::terminate) when done; a box
/// left running ends at its timeout and is billed until then.
#[derive(Clone, Debug)]
pub struct Sandbox {
    pub(crate) portal: TimePortal,
    as_of: DateTime<Utc>,
    info: SandboxInfo,
}

/// The box to get, from [`Sandbox::time_portal`]. Anything not set takes the API's default: no
/// network, 1 vCPU, 2 GiB, a 1 h lifetime.
#[must_use]
pub struct SandboxBuilder {
    as_of: DateTime<Utc>,
    client: Option<Client>,
    request: SandboxCreateRequest,
}

impl fmt::Debug for SandboxBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxBuilder")
            .field("as_of", &self.as_of)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl SandboxBuilder {
    /// The API client to use. Defaults to `Client::builder().build()`, which reads
    /// `EPOCHAL_API_KEY` and `EPOCHAL_BASE_URL`.
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// An app mounted read-only at `/opt/epochal/bin`, by name. It is a current build, not one as
    /// of `as_of`, so a box that uses it is not strictly held to its instant.
    pub fn with_app(mut self, name: impl Into<String>) -> Self {
        self.request.apps.push(name.into());
        self
    }

    /// The relay, so [`Sandbox::serve_http`] can serve the box's ports from here.
    pub fn with_relay(self) -> Self {
        self.with_app(RELAY_APP)
    }

    /// What the box may reach; [`Network::None`] when unset.
    pub fn with_network(mut self, network: Network) -> Self {
        self.request.network = Some(network);
        self
    }

    /// Virtual CPUs reserved for the box.
    pub fn with_vcpus(mut self, vcpus: u32) -> Self {
        self.request.vcpus = Some(vcpus);
        self
    }

    /// Memory reserved for the box, in GiB.
    pub fn with_memory_gib(mut self, memory_gib: u32) -> Self {
        self.request.memory_gib = Some(memory_gib);
        self
    }

    /// The box ends this long after it boots, in whole seconds.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request.timeout = Some(timeout);
        self
    }

    /// The box ends after this long with no command running, in whole seconds.
    pub fn with_idle_timeout(mut self, idle_timeout: Duration) -> Self {
        self.request.idle_timeout = Some(idle_timeout);
        self
    }

    /// A variable in the environment of every command in the box. Values are not logged.
    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.request.env.insert(name.into(), value.into());
        self
    }

    /// A label kept with the box.
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.request.metadata.insert(key.into(), value.into());
        self
    }

    /// The create's `Idempotency-Key`, so that a build retried by the caller gets the box the first
    /// one made, not a second billed one. Unset, each build uses a fresh key of its own, which
    /// covers the retries the build makes itself.
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.request.idempotency_key = Some(key.into());
        self
    }

    /// Creates the box and waits until it is live. A box that was made but did not come up is
    /// terminated before the error is returned.
    pub async fn build(mut self) -> Result<Sandbox, Error> {
        let client = match self.client {
            Some(client) => client,
            None => Client::builder().build()?,
        };
        let portal = client.time_portal(self.as_of);
        let request = &mut self.request;
        request.idempotency_key.get_or_insert_with(idempotency_key);
        let mut info = retry(|| portal.create_sandbox(request)).await?;
        while info.state == SandboxState::Creating {
            match retry(|| portal.get_sandbox(&info.id, Some(MAX_WAIT))).await {
                Ok(next) => info = next,
                Err(e) => {
                    let _ = retry(|| portal.terminate_sandbox(&info.id)).await;
                    return Err(e.into());
                }
            }
        }
        if info.state != SandboxState::Live {
            let _ = retry(|| portal.terminate_sandbox(&info.id)).await;
            return Err(Error::NotLive {
                id: info.id,
                reason: info.error.map(|e| e.message),
            });
        }
        let Ok(as_of) = info.as_of.parse() else {
            let _ = retry(|| portal.terminate_sandbox(&info.id)).await;
            return Err(Error::Unexpected(format!(
                "box {} is held to {:?}",
                info.id, info.as_of
            )));
        };
        Ok(Sandbox {
            portal: client.time_portal(as_of),
            as_of,
            info,
        })
    }
}

impl Sandbox {
    /// A time-portal box held to `as_of`: its clock starts there and its network sees nothing
    /// published after it.
    pub fn time_portal(as_of: DateTime<Utc>) -> SandboxBuilder {
        SandboxBuilder {
            as_of,
            client: None,
            request: SandboxCreateRequest::default(),
        }
    }

    /// The box's id.
    pub fn id(&self) -> &str {
        &self.info.id
    }

    /// The instant the box is held to.
    pub fn as_of(&self) -> DateTime<Utc> {
        self.as_of
    }

    /// What it may reach.
    pub fn network(&self) -> Network {
        self.info.network
    }

    /// Reserved virtual CPUs.
    pub fn vcpus(&self) -> u32 {
        self.info.vcpus
    }

    /// Reserved memory in GiB.
    pub fn memory_gib(&self) -> u32 {
        self.info.memory_gib
    }

    /// Its lifetime from boot.
    pub fn timeout(&self) -> Duration {
        self.info.timeout
    }

    /// The apps it got, by name.
    pub fn apps(&self) -> impl Iterator<Item = &str> {
        self.info.apps.iter().map(|app| app.name.as_str())
    }

    /// The box's filesystem.
    pub fn files(&self) -> Files<'_> {
        Files { sandbox: self }
    }

    /// Ends the box now. Ending an ended box succeeds.
    pub async fn terminate(&self) -> Result<(), Error> {
        Ok(retry(|| self.portal.terminate_sandbox(self.id())).await?)
    }
}
