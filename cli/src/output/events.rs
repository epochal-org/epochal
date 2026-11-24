//! A harness's JSON events: what each means ([`Event`]), drawn live as progress — what the agent
//! thinks, runs and finds — by [`Screen`], which keeps the agent's last message as its answer.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::output::progress::{Kind, Out};
use crate::session::Harness;

/// Bounds what one call or result may print; the terminal cuts it again to its width.
const CLIP: usize = 500;

/// What one harness event means, whichever harness sent it.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// Thinking is under way; the estimate of its size so far, if the harness gives one.
    Thinking(Option<u64>),
    /// A thinking block finished; its text, when the provider shares it.
    Thought(String),
    /// Text from the agent, and the id of the message it is part of.
    Message(String, String),
    ToolStart {
        id: String,
        name: String,
        input: Value,
    },
    ToolEnd {
        id: String,
        output: String,
        failed: bool,
    },
    Failed(String),
}

pub fn events(harness: Harness, raw: &[u8]) -> Vec<Event> {
    let Ok(e) = serde_json::from_slice::<Value>(raw) else {
        let text = String::from_utf8_lossy(raw).trim().to_owned();
        return if text.is_empty() {
            vec![]
        } else {
            vec![Event::Failed(text)]
        };
    };
    match harness {
        Harness::Claude => claude(&e, plain_result),
        // grok's `streaming-messages-json` is Claude Code's stream-json, its results grok's records.
        Harness::Grok => claude(&e, grok_result),
        Harness::Codex => codex(&e),
    }
}

fn claude(e: &Value, tool_result: fn(&Value) -> (String, bool)) -> Vec<Event> {
    let blocks = e["message"]["content"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    match (e["type"].as_str(), e["subtype"].as_str()) {
        (Some("system"), Some("thinking_tokens")) => {
            vec![Event::Thinking(e["estimated_tokens"].as_u64())]
        }
        (Some("assistant"), _) => blocks
            .iter()
            .filter_map(|b| match b["type"].as_str()? {
                "thinking" => Some(Event::Thought(str_of(&b["thinking"]).to_owned())),
                "text" => Some(Event::Message(
                    str_of(&b["text"]).to_owned(),
                    str_of(&e["message"]["id"]).to_owned(),
                )),
                "tool_use" => Some(Event::ToolStart {
                    id: str_of(&b["id"]).to_owned(),
                    name: str_of(&b["name"]).to_owned(),
                    input: b["input"].clone(),
                }),
                _ => None,
            })
            .collect(),
        (Some("user"), _) => blocks
            .iter()
            .filter(|b| b["type"] == "tool_result")
            .map(|b| {
                let (output, failed) = tool_result(b);
                Event::ToolEnd {
                    id: str_of(&b["tool_use_id"]).to_owned(),
                    output,
                    failed,
                }
            })
            .collect(),
        (Some("result"), _) if e["is_error"] == true => {
            vec![Event::Failed(str_of(&e["result"]).to_owned())]
        }
        _ => Vec::new(),
    }
}

fn codex(e: &Value) -> Vec<Event> {
    let item = &e["item"];
    let id = || str_of(&item["id"]).to_owned();
    match (e["type"].as_str(), item["type"].as_str()) {
        (Some("turn.started"), _) => vec![Event::Thinking(None)],
        (Some("item.started"), Some("command_execution")) => vec![Event::ToolStart {
            id: id(),
            name: "shell".into(),
            input: json!({"command": unwrap_shell(str_of(&item["command"]))}),
        }],
        (Some("item.completed"), Some("command_execution")) => vec![Event::ToolEnd {
            id: id(),
            output: str_of(&item["aggregated_output"]).to_owned(),
            failed: item["exit_code"].as_i64().is_some_and(|c| c != 0),
        }],
        (Some("item.started"), Some("mcp_tool_call")) => vec![Event::ToolStart {
            id: id(),
            name: format!(
                "mcp__{}__{}",
                str_of(&item["server"]),
                str_of(&item["tool"])
            ),
            input: item["arguments"].clone(),
        }],
        (Some("item.completed"), Some("mcp_tool_call")) => vec![Event::ToolEnd {
            id: id(),
            output: match item["error"].is_null() {
                true => text_of(&item["result"]["content"]),
                false => text_of(&item["error"]["message"]),
            },
            failed: !item["error"].is_null(),
        }],
        (Some("item.completed"), Some("file_change")) => {
            let changes = item["changes"].as_array().cloned().unwrap_or_default();
            changes
                .iter()
                .flat_map(|c| {
                    let id = format!("{}:{}", id(), str_of(&c["path"]));
                    let input = json!({"file_path": c["path"], "kind": c["kind"]});
                    [
                        Event::ToolStart {
                            id: id.clone(),
                            name: "file_change".into(),
                            input,
                        },
                        Event::ToolEnd {
                            id,
                            output: String::new(),
                            failed: false,
                        },
                    ]
                })
                .collect()
        }
        (Some("item.completed"), Some("todo_list")) | (Some("item.updated"), Some("todo_list")) => {
            vec![
                Event::ToolStart {
                    id: id(),
                    name: "todo_list".into(),
                    input: json!({"items": item["items"]}),
                },
                Event::ToolEnd {
                    id: id(),
                    output: String::new(),
                    failed: false,
                },
            ]
        }
        (Some("item.completed"), Some("reasoning")) => {
            vec![Event::Thought(str_of(&item["text"]).to_owned())]
        }
        (Some("item.completed"), Some("agent_message")) => {
            vec![Event::Message(str_of(&item["text"]).to_owned(), id())]
        }
        (Some("item.completed"), Some("error")) => {
            vec![Event::Failed(str_of(&item["message"]).to_owned())]
        }
        (Some("turn.failed"), _) => vec![Event::Failed(str_of(&e["error"]["message"]).to_owned())],
        (Some("error"), _) => vec![Event::Failed(str_of(&e["message"]).to_owned())],
        _ => Vec::new(),
    }
}

/// How a tool call reads: what the spinner says while it runs, and its line once started.
struct Call {
    doing: &'static str,
    head: String,
    detail: String,
}

fn describe(name: &str, input: &Value) -> Call {
    let s = |k: &str| str_of(&input[k]);
    let call = |doing, head: &str, detail: String| Call {
        doing,
        head: head.into(),
        detail,
    };
    match name.rsplit("__").next().unwrap_or(name) {
        "web_search" | "WebSearch" => call("Searching", "Search", quote(s("query"))),
        "web_fetch" | "WebFetch" => call("Reading", "Fetch", clip(s("url"))),
        "Bash" | "shell" | "run_terminal_command" => call("Running", "Run", clip(s("command"))),
        "Read" => call("Reading", "Read", s("file_path").into()),
        "read_file" => call("Reading", "Read", s("target_file").into()),
        "list_dir" => call("Reading", "List", s("target_directory").into()),
        "Write" | "write" => call("Writing", "Write", s("file_path").into()),
        "Edit" | "MultiEdit" | "NotebookEdit" | "search_replace" => {
            call("Editing", "Edit", s("file_path").into())
        }
        "file_change" => match s("kind") {
            "add" => call("Writing", "Write", s("file_path").into()),
            "delete" => call("Deleting", "Delete", s("file_path").into()),
            _ => call("Editing", "Edit", s("file_path").into()),
        },
        "Grep" | "grep" => call(
            "Searching",
            "Grep",
            format!("{} {}", quote(s("pattern")), s("path")),
        ),
        "Glob" => call("Searching", "Glob", s("pattern").into()),
        "TodoWrite" | "todo_list" | "todo_write" => call("Planning", "Plan", String::new()),
        "Task" | "Agent" | "spawn_subagent" => call("Delegating", "Agent", clip(s("description"))),
        other => call("Running", other, clip(&input.to_string())),
    }
}

/// One line on how a tool call came out.
fn outcome(name: &str, output: &str, failed: bool) -> String {
    // Claude Code's stand-in for a command that printed nothing.
    let output = match output.trim() {
        "(Bash completed with no output)" => "",
        _ => output,
    };
    let first = output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if failed {
        return clip(first);
    }
    match name.rsplit("__").next().unwrap_or(name) {
        "web_search" => match results(output) {
            0 => "no results".into(),
            1 => "1 result".into(),
            n => format!("{n} results"),
        },
        "web_fetch" => clip(first),
        "Read" => {
            // Claude Code numbers the empty line after a file's final newline too.
            let numbered: Vec<&str> = output
                .lines()
                .filter_map(|l| Some(l.split_once('\t')?.1))
                .collect();
            let trailing = numbered.last().is_some_and(|l| l.is_empty());
            format!("{} lines", numbered.len() - usize::from(trailing))
        }
        "TodoWrite" | "todo_list" | "todo_write" | "Write" | "write" | "Edit" | "MultiEdit"
        | "search_replace" | "file_change" => String::new(),
        _ => match output.lines().filter(|l| !l.trim().is_empty()).count() {
            0 => String::new(),
            1 => clip(first),
            n => format!("{} (+{} lines)", clip(first), n - 1),
        },
    }
}

fn plan(name: &str, input: &Value) -> Vec<String> {
    let items = match name {
        "TodoWrite" | "todo_write" => &input["todos"],
        _ => &input["items"],
    };
    items
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            let done = t["status"] == "completed" || t["completed"] == true;
            let doing = t["status"] == "in_progress";
            let text = t["content"].as_str().or(t["text"].as_str()).unwrap_or("");
            format!(
                "{} {text}",
                if done {
                    "☒"
                } else if doing {
                    "◐"
                } else {
                    "☐"
                }
            )
        })
        .collect()
}

pub struct Screen<O: Out> {
    harness: Harness,
    out: O,
    /// The agent's latest message as (id, text), its parts joined: shown as progress once anything
    /// follows.
    held: Option<(String, String)>,
    /// The last message, as `held` but never shown: the run's answer.
    last: Option<(String, String)>,
    /// Tool calls under way, by id: their name, for reading their outcome, and their line.
    running: HashMap<String, (String, String)>,
    /// The call whose line was printed last: an outcome directly under it needs no label.
    last_call: Option<String>,
    thinking: Option<u64>,
}

impl<O: Out> Screen<O> {
    pub fn new(harness: Harness, out: O) -> Self {
        Screen {
            harness,
            out,
            held: None,
            last: None,
            running: HashMap::new(),
            last_call: None,
            thinking: None,
        }
    }

    /// Where progress goes, for lines that aren't harness events.
    pub fn out(&mut self) -> &mut O {
        &mut self.out
    }

    pub fn event(&mut self, raw: &[u8]) {
        for event in events(self.harness, raw) {
            self.apply(event);
        }
    }

    /// The agent's last message, its parts joined.
    pub fn answer(&self) -> &str {
        self.last.as_ref().map_or("", |(_, text)| text)
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Thinking(tokens) => {
                self.thinking = tokens.or(self.thinking);
                let note = self
                    .thinking
                    .map(|t| format!(" · ~{t} tokens"))
                    .unwrap_or_default();
                self.out.status("Thinking", &note);
            }
            Event::Thought(text) => {
                self.release();
                self.last_call = None;
                let size = self.thinking.take().map(|t| format!("(~{t} tokens)"));
                match text.trim() {
                    "" => self
                        .out
                        .line(Kind::Thought, "Thought", &size.unwrap_or_default()),
                    text => self.out.line(Kind::Thought, "Thought", &clip(text)),
                }
                self.out.status("Thinking", "");
            }
            Event::Message(text, id) => {
                if self.held.as_ref().is_none_or(|(of, _)| *of != id) {
                    self.release();
                    self.out.status("Writing", "");
                }
                join(&mut self.held, &id, &text);
                join(&mut self.last, &id, &text);
            }
            Event::ToolStart { id, name, input } => {
                self.release();
                let call = describe(&name, &input);
                self.out.line(Kind::Tool, &call.head, &call.detail);
                for item in plan(&name, &input) {
                    self.out.line(Kind::Plan, &item, "");
                }
                self.out.status(call.doing, &format!(" {}", call.detail));
                let label = format!("{} {}", call.head, clip(&call.detail));
                self.last_call = Some(id.clone());
                self.running.insert(id, (name, label));
            }
            Event::ToolEnd { id, output, failed } => {
                let (name, label) = self.running.remove(&id).unwrap_or_default();
                let mut summary = outcome(&name, &output, failed);
                let under_its_call = self.last_call.as_deref() == Some(id.as_str());
                if !under_its_call && (failed || !summary.is_empty()) {
                    summary = format!("{} · {summary}", label.trim_end());
                }
                match (failed, summary.is_empty()) {
                    (true, _) => self.out.line(Kind::Failed, &summary, ""),
                    (false, false) => self.out.line(Kind::Result, &summary, ""),
                    (false, true) => {}
                }
                if !summary.is_empty() {
                    self.last_call = None;
                }
                if self.running.is_empty() {
                    self.out.status("Thinking", "");
                }
            }
            Event::Failed(text) => {
                self.release();
                self.last_call = None;
                self.out.line(Kind::Failed, &clip(&text), "");
            }
        }
    }

    fn release(&mut self) {
        if let Some((_, earlier)) = self.held.take() {
            self.last_call = None;
            self.out.line(Kind::Message, &clip(&earlier), "");
        }
    }
}

/// `text` added to the message in `slot` if it has the same `id`, else in its place.
fn join(slot: &mut Option<(String, String)>, id: &str, text: &str) {
    match slot {
        Some((of, held)) if of == id => {
            held.push_str("\n\n");
            held.push_str(text);
        }
        _ => *slot = Some((id.to_owned(), text.to_owned())),
    }
}

/// `v` as a string, or empty.
pub fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// A tool result's text, whether a string or a list of content blocks.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A tool result's text and whether it failed, as Claude Code sends it.
fn plain_result(b: &Value) -> (String, bool) {
    (text_of(&b["content"]), b["is_error"] == true)
}

/// grok returns a command's or an MCP tool's result as a JSON record: a command's `output` is its
/// bytes, an MCP tool's is `OkayOutput` or `Error`.
fn grok_result(b: &Value) -> (String, bool) {
    let (text, failed) = plain_result(b);
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return (text, failed);
    };
    let out = &record["output"];
    match record["type"].as_str() {
        Some("Bash") if out.is_array() => {
            let bytes: Vec<u8> = (out.as_array().into_iter().flatten())
                .filter_map(|b| u8::try_from(b.as_u64()?).ok())
                .collect();
            let code = record["exit_code"].as_i64();
            let output = String::from_utf8_lossy(&bytes).into_owned();
            (output, failed || code.is_some_and(|c| c != 0))
        }
        Some("MCP") if out["OkayOutput"].is_string() => (str_of(&out["OkayOutput"]).into(), failed),
        Some("MCP") if out["Error"].is_string() => (str_of(&out["Error"]).into(), true),
        _ => (text, failed),
    }
}

/// Codex wraps each command as `/bin/bash -lc '…'`; the agent wrote only the inside.
fn unwrap_shell(cmd: &str) -> String {
    let Some(rest) = cmd.strip_prefix("/bin/bash -lc ") else {
        return cmd.to_owned();
    };
    let quoted = |q: char| rest.strip_prefix(q).and_then(|r| r.strip_suffix(q));
    if let Some(inner) = quoted('\'') {
        // Single quotes keep everything literal; a quote inside is written '\''.
        return inner.replace(r"'\''", "'");
    }
    let Some(inner) = quoted('"') else {
        return rest.to_owned();
    };
    // Inside double quotes a backslash escapes only $ ` " \ and a newline.
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some('$' | '`' | '"' | '\\' | '\n')) => out.extend(chars.next()),
            _ => out.push(c),
        }
    }
    out
}

/// Results in `web_search`'s text: each is `N. title` with its URL on the next line.
fn results(text: &str) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    lines
        .windows(2)
        .filter(|w| {
            let digits = w[0].bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && w[0][digits..].starts_with(". ") && w[1].trim_start().starts_with("http")
        })
        .count()
}

fn quote(s: &str) -> String {
    format!("\"{}\"", clip(s))
}

fn clip(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(CLIP) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
