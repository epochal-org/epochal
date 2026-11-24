//! The built-in harness: an agent loop on this machine whose computer is the box. Each turn asks
//! the model endpoint for a reply, runs its tool calls in order (shell and files in the box, web
//! search and fetch at the as-of), and feeds the results back, until a reply calls no tool. The
//! box holds no agent binary and the prompt is ours, dated at the as-of, so nothing in the prompt,
//! the tools or the box postdates it. Progress is written as Claude Code's stream-json, which the
//! screen and trace already read.

mod model;
mod prompt;
mod tools;

use std::time::Duration;

use epochal_mcp::Tools as Web;
use epochal_sandbox::{Network, Sandbox};
use serde_json::{Value, json};

pub use model::{Api, Llm, endpoint, http};
use model::{Conversation, Failure, Reply, ToolResult};
use tools::{Computer, Def};

/// Tries for one reply, the first included.
const ATTEMPTS: u32 = 6;
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const CUT_OFF: &str = "Your reply was cut off at the output limit. Continue, in shorter steps.";
const LAST_TURN: &str =
    "That was your last tool call: reply now with your final answer from what you have.";

/// One run of the built-in harness.
pub struct Run<'a> {
    pub llm: &'a Llm,
    pub prompt: &'a str,
    pub sandbox: &'a Sandbox,
    pub web: Web,
    pub cwd: &'a str,
    pub max_turns: Option<u32>,
}

/// Where a run reports: each event as the screen and trace read it, and the statuses between.
pub trait Listener {
    fn event(&mut self, raw: Vec<u8>);
    fn thinking(&mut self);
}

impl Run<'_> {
    /// Drives the agent to its final answer, or to a model failure that retries didn't cure.
    pub async fn drive(self, out: &mut impl Listener) -> Result<(), String> {
        let online = self.sandbox.network() == Network::TimePortal;
        let system = prompt::system(self.sandbox.as_of(), self.cwd, online);
        let computer = Computer {
            sandbox: self.sandbox,
            web: self.web,
            cwd: self.cwd,
        };
        let defs = computer.defs();
        let mut convo = Conversation::new(self.llm.api(), system, self.prompt);
        for turn in 1.. {
            let last = self.max_turns.is_some_and(|max| turn >= max);
            out.thinking();
            let reply = sample(self.llm, &convo, &defs, last).await?;
            convo.push_reply(&reply);
            if !reply.shown.is_empty() {
                out.event(assistant(turn, reply.shown.clone()));
            }
            if last || reply.calls.is_empty() && !reply.cut {
                return Ok(());
            }
            let mut results = Vec::new();
            for call in reply.calls {
                // Each call is shown as it starts, so its result reads under it.
                let shown = json!({"type": "tool_use", "id": call.id, "name": call.name,
                    "input": call.input.clone().unwrap_or_else(|_| json!({}))});
                out.event(assistant(turn, vec![shown]));
                let (text, failed) = match call.input {
                    Ok(input) => computer.call(&call.name, input).await,
                    Err(why) => (format!("{} was not run: {why}", call.name), true),
                };
                let done = ToolResult {
                    id: call.id,
                    text,
                    failed,
                };
                let block = model::tool_result(&done);
                out.event(
                    json!({"type": "user", "message": {"content": [block]}})
                        .to_string()
                        .into_bytes(),
                );
                results.push(done);
            }
            let note = match self.max_turns {
                Some(max) if turn + 1 == max => Some(LAST_TURN),
                _ if results.is_empty() => Some(CUT_OFF),
                _ => None,
            };
            convo.push_results(&results, note);
        }
        unreachable!("turns are unbounded")
    }
}

/// A reply, asked again after a failure that may pass, with backoff or the wait the endpoint asks.
async fn sample(
    llm: &Llm,
    convo: &Conversation,
    defs: &[Def],
    last: bool,
) -> Result<Reply, String> {
    let mut backoff = FIRST_BACKOFF;
    for attempt in 1..=ATTEMPTS {
        match llm.sample(convo, defs, last).await {
            Ok(reply) => return Ok(reply),
            Err(Failure { message, retry, .. }) if !retry || attempt == ATTEMPTS => {
                return Err(message);
            }
            Err(Failure { message, after, .. }) => {
                tracing::warn!(attempt, %message, "asking the model again");
                tokio::time::sleep(after.unwrap_or(backoff).min(MAX_BACKOFF)).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
    unreachable!("the last attempt returns")
}

fn assistant(turn: u32, content: Vec<Value>) -> Vec<u8> {
    let event =
        json!({"type": "assistant", "message": {"id": format!("turn-{turn}"), "content": content}});
    event.to_string().into_bytes()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
