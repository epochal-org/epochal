//! A command's trace as JSON Lines in a file: every harness event and web call as it happens, and
//! a command's own API answer whole, so what a command saw can be read back.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use epochal_mcp::ToolCall;
use serde::Serialize;
use serde_json::Value;

/// One line of a trace.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Entry<'a, A: Serialize> {
    /// A harness event, as the harness sent it; a line that isn't JSON is kept as a string.
    Event { event: Value },
    /// A web tool call, as the API answered it.
    Call { call: &'a ToolCall },
    /// The API's whole answer to a command's own call.
    Answer { op: &'a str, answer: &'a A },
}

pub struct Trace {
    out: BufWriter<File>,
}

/// A trace written from more than one task: a session's observer and its tool server.
pub type Shared = Arc<Mutex<Trace>>;

impl Trace {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
        Ok(Trace {
            out: BufWriter::new(file),
        })
    }

    pub fn event(&mut self, raw: &[u8]) {
        let event = serde_json::from_slice(raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(raw).into_owned()));
        let entry: Entry<'_, ()> = Entry::Event { event };
        self.write(&entry);
    }

    pub fn call(&mut self, call: &ToolCall) {
        let entry: Entry<'_, ()> = Entry::Call { call };
        self.write(&entry);
    }

    pub fn answer(&mut self, op: &str, answer: &impl Serialize) {
        self.write(&Entry::Answer { op, answer });
    }

    /// A trace that can't be written is reported once a line; the run goes on.
    fn write(&mut self, entry: &Entry<'_, impl Serialize>) {
        let written = serde_json::to_writer(&mut self.out, entry)
            .map_err(io::Error::from)
            .and_then(|()| self.out.write_all(b"\n"))
            .and_then(|()| self.out.flush());
        if let Err(e) = written {
            tracing::warn!(%e, "writing the trace");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use epochal_api::time_portal::web::SearchRequest;
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn each_event_and_call_is_one_json_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut trace = Trace::open(&path).unwrap();
        trace.event(br#"{"type":"assistant"}"#);
        trace.event(b"not json");
        trace.call(&ToolCall::Search {
            request: SearchRequest::new("q"),
            response: Err("search is unavailable right now".into()),
        });
        trace.answer("fetch", &json!({"fetched_at": "2026-11-06T12:00:00Z"}));
        drop(trace);
        let lines: Vec<Value> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                json!({"kind": "event", "event": {"type": "assistant"}}),
                json!({"kind": "event", "event": "not json"}),
                json!({"kind": "call", "call": {"tool": "search", "request": {"query": "q"},
                    "error": "search is unavailable right now"}}),
                json!({"kind": "answer", "op": "fetch", "answer": {"fetched_at": "2026-11-06T12:00:00Z"}}),
            ]
        );
    }
}
