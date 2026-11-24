#![doc = include_str!("../README.md")]

mod exec;
mod files;
mod retry;
mod sandbox;
mod tunnel;

use std::io;

use epochal_api::Error as ApiError;

pub use epochal_api::time_portal::sandbox::{
    ExecExit, ExecFrame, FileStat, FileType, Network, Stream,
};
pub use epochal_api::{Client, chrono};
pub use exec::{Command, Exec, Output};
pub use files::Files;
pub use sandbox::{Sandbox, SandboxBuilder};
pub use tunnel::Tunnel;

/// A failed call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An API call failed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The box ended or failed before it was live.
    #[error("sandbox {id} did not come up: {}", reason.as_deref().unwrap_or("no reason given"))]
    NotLive {
        /// The box's id.
        id: String,
        /// Why, as the API put it.
        reason: Option<String>,
    },
    /// A command this crate ran in the box, to move a directory, failed.
    #[error("{step} failed: {stderr}")]
    CommandFailed {
        /// What the command was for.
        step: String,
        /// Its stderr.
        stderr: String,
    },
    /// A local file could not be read or written.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The box answered in a way this crate can't continue from.
    #[error("unexpected: {0}")]
    Unexpected(String),
    /// The relay behind [`Sandbox::serve_http`] failed.
    #[error("tunnel: {0}")]
    Tunnel(String),
}

/// An argument this crate refuses before any call, as the API crate refuses its own.
fn invalid(why: String) -> Error {
    Error::Api(ApiError::InvalidArgument(why))
}
