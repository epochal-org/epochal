//! `epochal-mcp`: Epochal's web tools over MCP on stdio or streamable HTTP. Its one mode is the
//! time portal: web search and web fetch held to a past instant.

use std::error::Error;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use epochal_api::chrono::{DateTime, Utc};
use epochal_mcp::{Tools, parse_as_of};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

/// Serves `web_search` and `web_fetch` over MCP, held to the `--time-portal` instant. Reads the
/// API key from EPOCHAL_API_KEY and the API's address from EPOCHAL_BASE_URL.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Serve the web through the time portal as it stood at this instant: RFC 3339, an HTTP-date
    /// or YYYY-MM-DD (the end of that UTC day).
    #[arg(long, value_name = "INSTANT", value_parser = parse_as_of)]
    time_portal: DateTime<Utc>,
    /// How to serve: `stdio`, or `http:[HOST:]PORT` for streamable HTTP under /mcp. A bare port
    /// listens on 127.0.0.1 only.
    #[arg(long, value_name = "TRANSPORT", default_value = "stdio", value_parser = parse_serve)]
    serve: Serve,
    /// A Host the HTTP endpoint answers besides the loopback names; repeatable.
    #[arg(long, value_name = "HOST")]
    allow_host: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum Serve {
    Stdio,
    Http(SocketAddr),
}

fn parse_serve(text: &str) -> Result<Serve, String> {
    if text == "stdio" {
        return Ok(Serve::Stdio);
    }
    let Some(addr) = text.strip_prefix("http:") else {
        return Err("expected `stdio` or `http:[HOST:]PORT`".to_owned());
    };
    if let Ok(port) = addr.parse::<u16>() {
        return Ok(Serve::Http((Ipv4Addr::LOCALHOST, port).into()));
    }
    addr.parse()
        .map(Serve::Http)
        .map_err(|_| format!("`{addr}` is not PORT or HOST:PORT (an IPv6 host goes in brackets)"))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();
    let args = Args::parse();
    if args.serve == Serve::Stdio && !args.allow_host.is_empty() {
        Args::command()
            .error(
                ErrorKind::ArgumentConflict,
                "--allow-host needs --serve http:…",
            )
            .exit();
    }
    let tools = Tools::time_portal(args.time_portal)
        .with_allowed_hosts(args.allow_host)
        .build()?;
    let Serve::Http(addr) = args.serve else {
        return Ok(tools.serve_stdio().await?);
    };
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "serving MCP at /mcp");
    Ok(tools.serve_http(listener).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_port_listens_on_loopback_only() {
        assert_eq!(
            parse_serve("http:8080"),
            Ok(Serve::Http("127.0.0.1:8080".parse().unwrap()))
        );
    }

    #[test]
    fn an_explicit_host_is_kept() {
        for addr in ["0.0.0.0:8080", "[::1]:9000"] {
            assert_eq!(
                parse_serve(&format!("http:{addr}")),
                Ok(Serve::Http(addr.parse().unwrap()))
            );
        }
        assert_eq!(parse_serve("stdio"), Ok(Serve::Stdio));
    }

    #[test]
    fn anything_else_is_refused() {
        for bad in [
            "",
            "http",
            "http:",
            "sse:8080",
            "http:localhost:8080",
            "http:99999",
        ] {
            assert!(parse_serve(bad).is_err(), "{bad:?}");
        }
    }
}
