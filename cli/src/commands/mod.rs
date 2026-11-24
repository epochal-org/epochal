//! The subcommands, one a file. Each turns its arguments into a [`Report`]; `main` writes it in the
//! chosen format to stdout or `--output`, so every command gets text and JSON alike.

mod fetch;
mod run;
mod search;

use anyhow::Result;

pub use fetch::Args as FetchArgs;
pub use run::Args as RunArgs;
pub use search::Args as SearchArgs;

use crate::output::Report;
use crate::output::trace::Trace;

/// A subcommand, and the result it reports. What it does along the way goes to `trace` when
/// `--trace` is given.
pub trait Command {
    type Report: Report;

    async fn run(self, trace: Option<Trace>) -> Result<Done<Self::Report>>;
}

/// A command's report, and the exit code it ends with.
pub struct Done<R> {
    pub report: R,
    pub code: i32,
}

impl<R> Done<R> {
    pub fn ok(report: R) -> Self {
        Done { report, code: 0 }
    }
}
