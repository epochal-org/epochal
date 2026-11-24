//! `epochal`: the Epochal command line. `epochal time-portal` runs an agent in a sandbox held to an
//! as-of, and searches or reads the web as it stood then. The agent's model calls and web tools are
//! made from this machine, so the model key never enters the box.

mod cli;
mod commands;
mod lines;
mod output;
mod sampler;
mod session;

use std::io;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use cli::OutputArgs;
use commands::{Command, Done, FetchArgs, RunArgs, SearchArgs};
use output::Sink;
use output::trace::Trace;

/// Progress has its own lines on stderr; anything quieter than a warning would break them up.
const DEFAULT_LOG: &str = "warn";

#[derive(Parser)]
#[command(name = "epochal", version, about = "The Epochal command line")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
    #[command(flatten)]
    output: OutputArgs,
}

#[derive(Subcommand)]
enum Cmd {
    /// Agents, search and fetch held to an as-of
    #[command(subcommand)]
    TimePortal(TimePortal),
}

#[derive(Subcommand)]
enum TimePortal {
    /// Run an agent on a prompt in a new box
    Run(Box<RunArgs>),
    /// Search the web as it stood at --as-of
    Search(SearchArgs),
    /// Read a page as it stood at --as-of
    Fetch(FetchArgs),
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .init();
    let cli = Cli::parse();
    let Cmd::TimePortal(cmd) = cli.cmd;
    let code = match cmd {
        TimePortal::Run(a) => report(*a, &cli.output).await?,
        TimePortal::Search(a) => report(a, &cli.output).await?,
        TimePortal::Fetch(a) => report(a, &cli.output).await?,
    };
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)))
}

/// Runs `cmd` and writes its report where and how `--output` and `--format` say.
async fn report<C: Command>(cmd: C, out: &OutputArgs) -> Result<i32> {
    let trace = out.trace.as_deref().map(Trace::open).transpose()?;
    let Done { report, code } = cmd.run(trace).await?;
    Sink::new(out.output.clone()).write(&report, out.format)?;
    Ok(code)
}
