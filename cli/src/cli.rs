//! Argument groups every command that needs them shares, so a flag means the same everywhere.

use std::path::PathBuf;

use clap::Args;
use epochal_api::chrono::{DateTime, Utc};
use epochal_mcp::parse_as_of;

use crate::output::Format;

/// Where and how a command writes its result.
#[derive(Args, Clone, Debug)]
pub struct OutputArgs {
    #[arg(long, value_enum, global = true, default_value_t)]
    pub format: Format,
    /// Write the result here instead of stdout
    #[arg(short, long, global = true)]
    pub output: Option<PathBuf>,
    /// Also write everything the command saw here, as JSON Lines: each harness event and web
    /// call of a run, or the command's own call
    #[arg(long, global = true)]
    pub trace: Option<PathBuf>,
}

/// The instant everything is held to.
#[derive(Args, Clone, Debug)]
pub struct AsOfArgs {
    /// RFC 3339, an HTTP-date, or YYYY-MM-DD meaning the end of that UTC day
    #[arg(long, value_parser = parse_as_of)]
    pub as_of: DateTime<Utc>,
}
