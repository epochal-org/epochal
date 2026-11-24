#![doc = include_str!("../README.md")]

mod as_of;
pub mod text;

pub use as_of::parse as parse_as_of;

use std::fmt;
use std::io;
use std::sync::Arc;

use axum::Router;
use chrono::{DateTime, Utc};
use epochal_api::time_portal::TimePortal;
use epochal_api::time_portal::web::{FetchRequest, FetchResponse, SearchRequest, SearchResponse};
use epochal_api::{Client, Error as ApiError};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::stdio;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use tokio::net::TcpListener;

const SERVER_NAME: &str = "epochal";
const WEB_SEARCH: &str = "web_search";
const WEB_FETCH: &str = "web_fetch";

const DEFAULT_RESULTS: u32 = 10;
const MAX_RESULTS: u32 = 10;
const DEFAULT_CHARACTERS: u32 = 20_000;
const MAX_CHARACTERS: u32 = 100_000;
const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// One web call the tools made, as the API answered it: before any timestamp is hidden from the
/// agent. The response is `Err` when the call failed; it serializes beside the request as
/// `"response"`, or as `"error"` and the message.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "one per web call")]
pub enum ToolCall {
    /// A `web_search` call.
    Search {
        /// What was sent.
        request: SearchRequest,
        /// What came back.
        #[serde(flatten, serialize_with = "outcome")]
        response: Result<SearchResponse, String>,
    },
    /// A `web_fetch` call.
    Fetch {
        /// What was sent.
        request: FetchRequest,
        /// What came back.
        #[serde(flatten, serialize_with = "outcome")]
        response: Result<FetchResponse, String>,
    },
}

fn outcome<T: Serialize, S: Serializer>(
    response: &Result<T, String>,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut map = s.serialize_map(Some(1))?;
    match response {
        Ok(response) => map.serialize_entry("response", response)?,
        Err(error) => map.serialize_entry("error", error)?,
    }
    map.end()
}

#[derive(Clone)]
struct Observer(Arc<dyn Fn(ToolCall) + Send + Sync>);

impl fmt::Debug for Observer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Observer")
    }
}

/// `web_search` and `web_fetch` held to one instant, from [`Tools::time_portal`]. Implements rmcp's
/// [`ServerHandler`]; serve it with [`serve_stdio`](Self::serve_stdio) or mount
/// [`http_router`](Self::http_router).
#[derive(Clone, Debug)]
pub struct Tools {
    portal: TimePortal,
    allowed_hosts: Vec<String>,
    observer: Option<Observer>,
    tool_router: ToolRouter<Self>,
}

/// Configures [`Tools`]. Anything not set takes its default when [`build`](Self::build) runs.
#[derive(Debug)]
pub struct ToolsBuilder {
    as_of: DateTime<Utc>,
    client: Option<Client>,
    allowed_hosts: Vec<String>,
    observer: Option<Observer>,
}

impl ToolsBuilder {
    /// The client every call goes through. Defaults to `Client::builder().build()`, which reads
    /// `EPOCHAL_API_KEY` and `EPOCHAL_BASE_URL`.
    pub fn with_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    /// `Host` names, or `host:port` authorities, the HTTP endpoint answers besides the loopback
    /// names `localhost`, `127.0.0.1` and `::1`. Only loopback by default, which keeps web pages
    /// from reaching it by DNS rebinding.
    pub fn with_allowed_hosts(
        mut self,
        hosts: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.allowed_hosts.extend(hosts.into_iter().map(Into::into));
        self
    }

    /// Called with each web call once it is answered, before the agent sees the answer.
    pub fn with_observer(mut self, observer: impl Fn(ToolCall) + Send + Sync + 'static) -> Self {
        self.observer = Some(Observer(Arc::new(observer)));
        self
    }

    /// The tools, with defaults for anything not set.
    pub fn build(self) -> Result<Tools, ApiError> {
        let client = match self.client {
            Some(client) => client,
            None => Client::builder().build()?,
        };
        Ok(Tools {
            portal: client.time_portal(self.as_of),
            allowed_hosts: LOOPBACK
                .map(String::from)
                .into_iter()
                .chain(self.allowed_hosts)
                .collect(),
            observer: self.observer,
            tool_router: Tools::tool_router(),
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchArgs {
    /// What to search the web for.
    query: String,
    /// Most results to return, 1-10 (default 10).
    #[schemars(range(min = 1, max = MAX_RESULTS))]
    num_results: Option<u32>,
    /// Only return pages from these sites, e.g. ["reuters.com"].
    domains: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FetchArgs {
    /// The page to read: a full http or https URL.
    url: String,
    /// Most characters of the page to return, 1-100000 (default 20000).
    #[schemars(range(min = 1, max = MAX_CHARACTERS))]
    max_characters: Option<u32>,
}

impl Tools {
    /// A builder for tools over the time portal, held to `as_of`. Answers are shown to the agent
    /// with no timestamps.
    pub fn time_portal(as_of: DateTime<Utc>) -> ToolsBuilder {
        ToolsBuilder {
            as_of,
            client: None,
            allowed_hosts: Vec::new(),
            observer: None,
        }
    }

    /// The tools' names, descriptions and input schemas, as MCP lists them.
    pub fn definitions(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    /// Calls the tool `name` in-process, as an MCP client's call would; an unknown name or
    /// arguments that don't fit its schema are a tool error.
    pub async fn call(&self, name: &str, arguments: Value) -> CallToolResult {
        let invalid = |e: serde_json::Error| {
            CallToolResult::error(vec![ContentBlock::text(format!("invalid arguments: {e}"))])
        };
        match name {
            WEB_SEARCH => match serde_json::from_value(arguments) {
                Ok(args) => self.web_search(Parameters(args)).await,
                Err(e) => invalid(e),
            },
            WEB_FETCH => match serde_json::from_value(arguments) {
                Ok(args) => self.web_fetch(Parameters(args)).await,
                Err(e) => invalid(e),
            },
            _ => CallToolResult::error(vec![ContentBlock::text(format!("no tool named {name}"))]),
        }
    }

    /// Serves the tools over stdin and stdout until the client hangs up.
    pub async fn serve_stdio(self) -> io::Result<()> {
        let running = self.serve(stdio()).await.map_err(io::Error::other)?;
        running.waiting().await.map_err(io::Error::other)?;
        Ok(())
    }

    /// Serves the tools at `/mcp` on `listener` until it fails. Responses carry no `Date` header,
    /// so a client held to a past instant isn't told today's date.
    pub async fn serve_http(self, listener: TcpListener) -> io::Result<()> {
        let app = Router::new().nest_service("/mcp", self.http_router());
        loop {
            let (conn, _) = listener.accept().await?;
            let service = TowerToHyperService::new(app.clone());
            tokio::spawn(async move {
                let conn = TokioIo::new(conn);
                let served = http1::Builder::new()
                    .auto_date_header(false)
                    .serve_connection(conn, service)
                    .await;
                if let Err(error) = served {
                    tracing::debug!(%error, "MCP connection ended");
                }
            });
        }
    }

    /// The tools as a stateless streamable-HTTP endpoint, to mount with
    /// `Router::nest_service("/mcp", tools.http_router())`. It answers only the hosts
    /// [`with_allowed_hosts`](ToolsBuilder::with_allowed_hosts) allows.
    pub fn http_router(self) -> Router {
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_allowed_hosts(self.allowed_hosts.clone());
        let service = StreamableHttpService::new(
            move || Ok(self.clone()),
            Arc::new(NeverSessionManager::default()),
            config,
        );
        Router::new().fallback_service(service)
    }
}

#[tool_router]
impl Tools {
    #[tool(
        name = "web_search",
        description = "Search the web as it stood today. Returns each result's title, URL and snippet; web_fetch a URL to read it."
    )]
    async fn web_search(&self, Parameters(args): Parameters<SearchArgs>) -> CallToolResult {
        let mut request = SearchRequest::new(args.query);
        request.num_results = Some(
            args.num_results
                .unwrap_or(DEFAULT_RESULTS)
                .clamp(1, MAX_RESULTS),
        );
        request.domains = args.domains.unwrap_or_default();
        let response = self.portal.search(&request).await;
        let (result, response) = settle(WEB_SEARCH, response, |r| Ok(text::search(r, false)));
        self.observe(ToolCall::Search { request, response });
        result
    }

    #[tool(
        name = "web_fetch",
        description = "Read a web page as it stood today, as text, up to 100000 characters. Returns its title, URL and content."
    )]
    async fn web_fetch(&self, Parameters(args): Parameters<FetchArgs>) -> CallToolResult {
        let mut request = FetchRequest::new(args.url);
        request.max_characters = Some(
            args.max_characters
                .unwrap_or(DEFAULT_CHARACTERS)
                .clamp(1, MAX_CHARACTERS),
        );
        let response = self.portal.fetch(&request).await;
        let (result, response) = settle(WEB_FETCH, response, |p| text::fetch(p, false));
        self.observe(ToolCall::Fetch { request, response });
        result
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
    }
}

/// What the agent is shown of `response`, as `text` shows it, and the response as the observer
/// gets it. An answer `text` refuses, such as a page that could not be read, is a tool error.
fn settle<T>(
    tool: &str,
    response: Result<T, ApiError>,
    text: impl Fn(&T) -> Result<String, String>,
) -> (CallToolResult, Result<T, String>) {
    match response {
        Ok(reply) => {
            let shown = match text(&reply) {
                Ok(shown) => CallToolResult::success(vec![ContentBlock::text(shown)]),
                Err(why) => CallToolResult::error(vec![ContentBlock::text(why)]),
            };
            (shown, Ok(reply))
        }
        Err(e) => {
            let why = e.to_string();
            (failure(tool, e), Err(why))
        }
    }
}

impl Tools {
    fn observe(&self, call: ToolCall) {
        if let Some(Observer(observer)) = &self.observer {
            observer(call);
        }
    }
}

/// The API's own message for a refusal. Anything else is logged and told to the agent only as a
/// failure, since transport errors name the endpoint.
fn failure(tool: &str, error: ApiError) -> CallToolResult {
    let message = match error {
        ApiError::Api { message, .. } => format!("{tool} failed: {message}"),
        error => {
            tracing::warn!(tool, %error, "Epochal API call failed");
            format!("{tool} is unavailable right now")
        }
    };
    CallToolResult::error(vec![ContentBlock::text(message)])
}
