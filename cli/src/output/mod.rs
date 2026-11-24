//! Everything a command shows or writes. Progress goes to stderr ([`progress`]); a command's result
//! is a [`Report`], written in the chosen [`Format`] to a [`Sink`]; a run's trace is JSONL
//! ([`trace`]).

pub mod events;
pub mod progress;
pub mod setup;
pub mod trace;
pub mod watcher;

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::ValueEnum;
use epochal_api::chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use tempfile::NamedTempFile;

/// A command's result: plain text for a reader, or its JSON.
pub trait Report: Serialize {
    fn text(&self) -> String;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    #[default]
    Text,
    Json,
}

/// `at` as reports show an instant: RFC 3339 in whole seconds, UTC.
pub fn instant(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Where a result goes: stdout, or a file written whole or not at all.
pub enum Sink {
    Stdout,
    File(PathBuf),
}

impl Sink {
    pub fn new(path: Option<PathBuf>) -> Self {
        path.map_or(Sink::Stdout, Sink::File)
    }

    pub fn write(&self, report: &impl Report, format: Format) -> Result<()> {
        match self {
            Sink::Stdout => {
                let mut out = io::stdout().lock();
                out.write_all(render(report, format)?.as_bytes())?;
                Ok(out.flush()?)
            }
            Sink::File(path) => write_atomic(path, render(report, format)?.as_bytes()),
        }
    }
}

/// `report` as `format` writes it, ending in one newline.
fn render(report: &impl Report, format: Format) -> Result<String> {
    let body = match format {
        Format::Text => report.text(),
        Format::Json => serde_json::to_string_pretty(report)?,
    };
    Ok(format!("{}\n", body.trim_end()))
}

/// `data` at `path` whole or not at all, so an interrupted write never leaves half a file. The
/// temporary file is a fresh one beside `path`, made for this write alone: no file already there is
/// touched, and concurrent writers never share one.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let mut tmp =
        NamedTempFile::new_in(dir).with_context(|| format!("writing beside {}", path.display()))?;
    tmp.write_all(data)
        .with_context(|| format!("writing {}", tmp.path().display()))?;
    tmp.persist(path)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::*;

    #[derive(Serialize)]
    struct Greeting {
        to: String,
    }

    impl Report for Greeting {
        fn text(&self) -> String {
            format!("hello {}\n\n", self.to)
        }
    }

    fn hi() -> Greeting {
        Greeting { to: "Ann".into() }
    }

    #[test]
    fn each_format_ends_in_one_newline() {
        assert_eq!(render(&hi(), Format::Text).unwrap(), "hello Ann\n");
        let json = render(&hi(), Format::Json).unwrap();
        assert!(json.ends_with("}\n"), "{json:?}");
        let json: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(json, json!({"to": "Ann"}));
    }

    #[test]
    fn a_write_never_touches_a_file_named_like_its_temporary() {
        let dir = tempdir().unwrap();
        let dir = dir.path();
        let path = dir.join("answer.md");
        let bystander = dir.join("answer.md.tmp");
        fs::write(&bystander, "someone else's").unwrap();
        write_atomic(&path, b"mine").unwrap();
        write_atomic(&path, b"mine again").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine again");
        assert_eq!(fs::read_to_string(&bystander).unwrap(), "someone else's");
        assert_eq!(fs::read_dir(dir).unwrap().count(), 2);
    }

    #[test]
    fn a_file_sink_writes_plain_text_and_leaves_no_temporary() {
        let dir = tempdir().unwrap();
        let dir = dir.path();
        let path = dir.join("report.txt");
        Sink::new(Some(path.clone()))
            .write(&hi(), Format::Text)
            .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello Ann\n");
        let left: Vec<_> = fs::read_dir(dir).unwrap().collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }
}
