//! The model endpoint: Anthropic Messages or an OpenAI-compatible Chat Completions API, streamed.
//! The conversation is kept in the endpoint's own wire format, so what a model sent, its thinking
//! and signatures included, goes back to it unchanged.

use std::collections::VecDeque;
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{Stream, StreamExt};
use reqwest::header::RETRY_AFTER;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode, Url};
use serde_json::{Map, Value, json};
use tokio::time::timeout;

use super::tools::Def;
use crate::lines::Lines;
use crate::output::events::str_of;

pub const BASE_ENV: &str = "EPOCHAL_LLM_BASE_URL";
pub const KEY_ENV: &str = "EPOCHAL_LLM_API_KEY";
pub const API_ENV: &str = "EPOCHAL_LLM_API";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic requires a cap on each reply; every current Claude model takes this one.
const MAX_TOKENS: u32 = 32_000;
/// How long a stream may go without a byte: reasoning models can think silently for minutes.
const STALL: Duration = Duration::from_secs(300);
const CONNECT: Duration = Duration::from_secs(30);
/// The most of an error body kept for the message.
const ERROR_BODY: usize = 2_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Anthropic,
    OpenAi,
}

/// The endpoint, its key and the model asked there, read on this machine.
pub struct Llm {
    http: Client,
    api: Api,
    url: String,
    key: String,
    model: String,
}

/// Why a request failed, and whether sending it again may succeed.
#[derive(Debug)]
pub struct Failure {
    pub message: String,
    pub retry: bool,
    pub after: Option<Duration>,
}

impl Failure {
    fn retry(message: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
            retry: true,
            after: None,
        }
    }
}

/// One tool call, its input as the model wrote it, or why it couldn't be read.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub input: Result<Value, String>,
}

/// One tool call's answer for the model, and whether it failed.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub text: String,
    pub failed: bool,
}

/// A model reply: as it goes back to the endpoint (none when nothing in it can), its text and
/// thinking as Claude Code's content blocks for the screen and trace, and its tool calls.
#[derive(Debug)]
pub struct Reply {
    native: Option<Value>,
    pub shown: Vec<Value>,
    pub calls: Vec<Call>,
    /// Stopped at the output limit.
    pub cut: bool,
}

/// The conversation so far, in the endpoint's format.
pub struct Conversation {
    api: Api,
    system: String,
    messages: Vec<Value>,
}

/// The model endpoint and its key from `var`, read on this machine; the error names what is unset.
pub fn endpoint(var: impl Fn(&str) -> Option<String>) -> Result<(Url, String)> {
    let set = |name: &str| var(name).filter(|v| !v.trim().is_empty());
    let (base, key) = match (set(BASE_ENV), set(KEY_ENV)) {
        (Some(base), Some(key)) => (base, key),
        (None, Some(_)) => bail!("set {BASE_ENV} to the model endpoint"),
        (Some(_), None) => bail!("set {KEY_ENV} to the model endpoint's key"),
        (None, None) => bail!("set {BASE_ENV} and {KEY_ENV} to the model endpoint and its key"),
    };
    let url = Url::parse(&base).with_context(|| format!("{BASE_ENV}={base:?}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("{BASE_ENV}={base:?} is not an http or https URL");
    }
    // The key goes only as its own header; a user or password in the URL would go too.
    if !url.username().is_empty() || url.password().is_some() {
        bail!("{BASE_ENV} must not hold a user or password; the key goes in {KEY_ENV}");
    }
    Ok((url, key))
}

impl Api {
    /// What paths like `/messages` go under at `base`, taken as each provider's own SDK takes it:
    /// Anthropic's without `/v1`, which is added; OpenAI's with it, added only to a bare host.
    pub fn root(self, base: &Url) -> String {
        let path = base.path().trim_end_matches('/');
        let base = base.as_str().trim_end_matches('/');
        match self {
            _ if path.ends_with("/v1") => base.to_owned(),
            Api::OpenAi if !path.is_empty() => base.to_owned(),
            _ => format!("{base}/v1"),
        }
    }
}

/// A client for model calls. It follows no redirect, which would carry the key: reqwest strips only
/// `authorization`. A connection or a response that stalls fails.
pub fn http() -> reqwest::Result<Client> {
    Client::builder()
        .redirect(Policy::none())
        .connect_timeout(CONNECT)
        .read_timeout(STALL)
        .build()
}

impl Llm {
    /// The endpoint from `var`; the API is `EPOCHAL_LLM_API`, or Anthropic for a model whose id
    /// starts with `claude` or `anthropic`.
    pub fn from_env(model: &str, var: impl Fn(&str) -> Option<String>) -> Result<Llm> {
        let (base, key) = endpoint(&var)?;
        let named = var(API_ENV).map(|v| v.trim().to_ascii_lowercase());
        let model_id = model.to_ascii_lowercase();
        let api = match named.as_deref().filter(|v| !v.is_empty()) {
            Some("anthropic") => Api::Anthropic,
            Some("openai") => Api::OpenAi,
            Some(other) => bail!("{API_ENV}={other:?}: use anthropic or openai"),
            None if model_id.starts_with("claude") || model_id.starts_with("anthropic") => {
                Api::Anthropic
            }
            None => Api::OpenAi,
        };
        let path = match api {
            Api::Anthropic => "messages",
            Api::OpenAi => "chat/completions",
        };
        Ok(Llm {
            http: http()?,
            url: format!("{}/{path}", api.root(&base)),
            api,
            key,
            model: model.to_owned(),
        })
    }

    pub fn api(&self) -> Api {
        self.api
    }

    /// One reply to `convo`; `last` forbids tool calls.
    pub async fn sample(
        &self,
        convo: &Conversation,
        tools: &[Def],
        last: bool,
    ) -> Result<Reply, Failure> {
        let body = convo.request(&self.model, tools, last);
        let request = self.http.post(&self.url).json(&body);
        let request = match self.api {
            Api::Anthropic => request
                .header("x-api-key", &self.key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            Api::OpenAi => request.bearer_auth(&self.key),
        };
        let response = timeout(STALL, request.send())
            .await
            .map_err(|_| Failure::retry("the model endpoint did not answer"))?
            .map_err(|e| Failure::retry(format!("calling the model endpoint: {e}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(refused(status, response).await);
        }
        let mut sse = Sse::new(response.bytes_stream());
        match self.api {
            Api::Anthropic => anthropic(&mut sse).await,
            Api::OpenAi => openai(&mut sse).await,
        }
    }
}

async fn refused(status: StatusCode, response: reqwest::Response) -> Failure {
    let after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok()?.trim().parse().ok())
        .map(Duration::from_secs);
    let body = timeout(STALL, response.text()).await;
    let body = body.ok().and_then(Result::ok).unwrap_or_default();
    let body: String = body.trim().chars().take(ERROR_BODY).collect();
    let code = status.as_u16();
    Failure {
        message: format!("the model endpoint answered {code}: {body}"),
        retry: matches!(code, 408 | 409 | 425 | 429) || status.is_server_error() || code == 529,
        after,
    }
}

impl Conversation {
    pub fn new(api: Api, system: String, prompt: &str) -> Self {
        let first = match api {
            Api::Anthropic => {
                json!({"role": "user", "content": [{"type": "text", "text": prompt}]})
            }
            Api::OpenAi => json!({"role": "user", "content": prompt}),
        };
        Conversation {
            api,
            system,
            messages: vec![first],
        }
    }

    pub fn push_reply(&mut self, reply: &Reply) {
        self.messages.extend(reply.native.clone());
    }

    /// Each call's result, in order, then `note` for the model, if any.
    pub fn push_results(&mut self, results: &[ToolResult], note: Option<&str>) {
        match self.api {
            Api::Anthropic => {
                let mut content: Vec<Value> = results.iter().map(tool_result).collect();
                if let Some(note) = note {
                    content.push(json!({"type": "text", "text": note}));
                }
                self.messages
                    .push(json!({"role": "user", "content": content}));
            }
            Api::OpenAi => {
                for done in results {
                    self.messages.push(
                        json!({"role": "tool", "tool_call_id": done.id, "content": done.text}),
                    );
                }
                if let Some(note) = note {
                    self.messages.push(json!({"role": "user", "content": note}));
                }
            }
        }
    }

    fn request(&self, model: &str, tools: &[Def], last: bool) -> Value {
        match self.api {
            Api::Anthropic => {
                // Caching the prefix up to the newest message makes each turn read the last one's.
                let mut messages = self.messages.clone();
                if let Some(block) = messages
                    .last_mut()
                    .and_then(|m| m["content"].as_array_mut()?.last_mut())
                {
                    block["cache_control"] = json!({"type": "ephemeral"});
                }
                let tools: Vec<Value> = tools
                    .iter()
                    .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}))
                    .collect();
                let mut body = json!({
                    "model": model,
                    "max_tokens": MAX_TOKENS,
                    "system": [{"type": "text", "text": self.system, "cache_control": {"type": "ephemeral"}}],
                    "messages": messages,
                    "tools": tools,
                    "stream": true,
                });
                if last {
                    body["tool_choice"] = json!({"type": "none"});
                }
                body
            }
            Api::OpenAi => {
                let mut messages = vec![json!({"role": "system", "content": self.system})];
                messages.extend(self.messages.iter().cloned());
                let tools: Vec<Value> = tools
                    .iter()
                    .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
                    .collect();
                let mut body =
                    json!({"model": model, "messages": messages, "tools": tools, "stream": true});
                if last {
                    body["tool_choice"] = json!("none");
                }
                body
            }
        }
    }
}

type Bytes = Pin<Box<dyn Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

/// The `data:` payloads of a server-sent event stream, whatever its chunking.
struct Sse {
    body: Bytes,
    lines: Lines,
    ready: VecDeque<Vec<u8>>,
    ended: bool,
}

impl Sse {
    fn new(body: impl Stream<Item = reqwest::Result<bytes::Bytes>> + Send + 'static) -> Self {
        Sse {
            body: Box::pin(body),
            lines: Lines::default(),
            ready: VecDeque::new(),
            ended: false,
        }
    }

    async fn next(&mut self) -> Result<Option<String>, Failure> {
        loop {
            while let Some(line) = self.ready.pop_front() {
                if let Some(data) = data(&line) {
                    return Ok(Some(data));
                }
            }
            if self.ended {
                return Ok(self.lines.finish().and_then(|rest| data(&rest)));
            }
            match timeout(STALL, self.body.next()).await {
                Err(_) => return Err(Failure::retry("the model's reply stalled")),
                Ok(Some(Ok(chunk))) => match self.lines.push(&chunk) {
                    Ok(lines) => self.ready.extend(lines),
                    Err(e) => {
                        return Err(Failure {
                            message: format!("the model's reply: {e}"),
                            retry: false,
                            after: None,
                        });
                    }
                },
                Ok(Some(Err(e))) => {
                    return Err(Failure::retry(format!("the model's reply broke off: {e}")));
                }
                Ok(None) => self.ended = true,
            }
        }
    }
}

fn data(line: &[u8]) -> Option<String> {
    let line = String::from_utf8_lossy(line);
    let data = line.trim_end_matches(['\r', '\n']).strip_prefix("data:")?;
    Some(data.strip_prefix(' ').unwrap_or(data).to_owned())
}

fn parse(data: &str) -> Result<Value, Failure> {
    serde_json::from_str(data)
        .map_err(|e| Failure::retry(format!("an unreadable event from the model: {e}")))
}

/// The result block Anthropic takes back, which the screen and trace read too.
pub fn tool_result(done: &ToolResult) -> Value {
    json!({"type": "tool_result", "tool_use_id": done.id, "content": done.text, "is_error": done.failed})
}

/// An error sent in the stream; only an overload, a rate limit or a server fault may pass.
fn in_band(error: &Value) -> Failure {
    let kind = format!("{} {}", error["type"], error["code"]).to_ascii_lowercase();
    let passing = [
        "overloaded",
        "rate_limit",
        "api_error",
        "server_error",
        "internalservererror",
        "timeout",
    ];
    Failure {
        message: format!("the model endpoint failed mid-reply: {error}"),
        retry: passing.iter().any(|p| kind.contains(p)),
        after: None,
    }
}

/// The input of a call as streamed: empty is no arguments; anything but an object is unreadable.
fn input(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str(text) {
        Ok(input @ Value::Object(_)) => Ok(input),
        Ok(_) => Err("the arguments are not a JSON object".into()),
        Err(e) => Err(format!(
            "the arguments are not valid JSON ({e}); the reply may have hit the output limit"
        )),
    }
}

async fn anthropic(sse: &mut Sse) -> Result<Reply, Failure> {
    let (mut blocks, mut inputs, mut stop) =
        (Vec::<Value>::new(), Vec::<String>::new(), String::new());
    loop {
        let Some(data) = sse.next().await? else {
            return Err(Failure::retry("the model's reply ended early"));
        };
        let event = parse(&data)?;
        let index = event["index"].as_u64().unwrap_or(u64::MAX) as usize;
        match event["type"].as_str().unwrap_or_default() {
            "content_block_start" => {
                blocks.push(event["content_block"].clone());
                inputs.push(String::new());
            }
            "content_block_delta" => {
                let (Some(block), delta) = (blocks.get_mut(index), &event["delta"]) else {
                    return Err(Failure::retry("a reply block out of order"));
                };
                let text = |key: &str| delta[key].as_str().unwrap_or_default();
                match text("type") {
                    "text_delta" => append(block, "text", text("text")),
                    "thinking_delta" => append(block, "thinking", text("thinking")),
                    "signature_delta" => append(block, "signature", text("signature")),
                    "input_json_delta" => inputs[index].push_str(text("partial_json")),
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(reason) = event["delta"]["stop_reason"].as_str() {
                    stop = reason.to_owned();
                }
            }
            "message_stop" => break,
            "error" => return Err(in_band(&event["error"])),
            _ => {}
        }
    }
    let mut calls = Vec::new();
    for (block, streamed) in blocks.iter_mut().zip(&inputs) {
        if block["type"] == "tool_use" {
            // A block may open with its whole input and stream none of it.
            let read = match (streamed.is_empty(), &block["input"]) {
                (true, opened @ Value::Object(_)) => Ok(opened.clone()),
                _ => input(streamed),
            };
            block["input"] = read.clone().unwrap_or_else(|_| json!({}));
            calls.push(Call {
                id: str_of(&block["id"]).to_owned(),
                name: str_of(&block["name"]).to_owned(),
                input: read,
            });
        }
    }
    // The endpoint refuses empty text, thinking cut off before its signature, and an empty turn.
    blocks.retain(|b| match b["type"].as_str() {
        Some("text") => !str_of(&b["text"]).is_empty(),
        Some("thinking") => !str_of(&b["signature"]).is_empty(),
        _ => true,
    });
    let shown = blocks.iter().filter(|b| b["type"] != "tool_use").cloned();
    Ok(Reply {
        shown: shown.collect(),
        native: (!blocks.is_empty()).then(|| json!({"role": "assistant", "content": blocks})),
        calls,
        cut: stop == "max_tokens",
    })
}

/// A Chat Completions tool call as it streams in.
#[derive(Default)]
struct Part {
    id: String,
    name: String,
    arguments: String,
}

async fn openai(sse: &mut Sse) -> Result<Reply, Failure> {
    let (mut text, mut reasoning, mut field) = (String::new(), String::new(), None);
    let mut parts: Vec<Part> = Vec::new();
    let mut finish = None;
    while let Some(data) = sse.next().await? {
        if data.trim() == "[DONE]" {
            break;
        }
        let chunk = parse(&data)?;
        if chunk["error"].is_object() {
            return Err(in_band(&chunk["error"]));
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        text.push_str(delta["content"].as_str().unwrap_or_default());
        // Servers mid-rename send both names with the same text; one is read.
        let named = ["reasoning_content", "reasoning"]
            .into_iter()
            .find_map(|key| Some((key, delta[key].as_str().filter(|s| !s.is_empty())?)));
        if let Some((key, more)) = named {
            field.get_or_insert(key);
            reasoning.push_str(more);
        }
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let id = call["id"].as_str().unwrap_or_default();
            // A fragment without an index opens a call if it has an id, else continues the last.
            let index = match call["index"].as_u64() {
                Some(index) => index as usize,
                None if !id.is_empty() || parts.is_empty() => parts.len(),
                None => parts.len() - 1,
            };
            if parts.len() <= index {
                parts.resize_with(index + 1, Part::default);
            }
            let part = &mut parts[index];
            part.id.push_str(id);
            part.name
                .push_str(call["function"]["name"].as_str().unwrap_or_default());
            part.arguments
                .push_str(call["function"]["arguments"].as_str().unwrap_or_default());
        }
        if let Some(reason) = choice["finish_reason"].as_str() {
            finish = Some(reason.to_owned());
        }
    }
    let Some(finish) = finish else {
        return Err(Failure::retry("the model's reply ended early"));
    };
    let mut native = Map::new();
    let mut shown = Vec::new();
    native.insert("role".into(), json!("assistant"));
    if let Some(field) = field {
        native.insert(field.into(), json!(reasoning));
        shown.push(json!({"type": "thinking", "thinking": reasoning}));
    }
    if !text.is_empty() {
        shown.push(json!({"type": "text", "text": text}));
    }
    let (mut calls, mut sent) = (Vec::new(), Vec::new());
    for (i, part) in parts.into_iter().enumerate() {
        let id = match part.id.is_empty() {
            true => format!("call_{i}"),
            false => part.id,
        };
        let read = input(&part.arguments);
        let echoed = if read.is_ok() {
            part.arguments
        } else {
            "{}".to_owned()
        };
        sent.push(json!({"id": id, "type": "function", "function": {"name": part.name, "arguments": echoed}}));
        calls.push(Call {
            id,
            name: part.name,
            input: read,
        });
    }
    // Content may be null only beside tool calls.
    let content = match (text.is_empty(), sent.is_empty()) {
        (true, false) => Value::Null,
        _ => json!(text),
    };
    native.insert("content".into(), content);
    if !sent.is_empty() {
        native.insert("tool_calls".into(), Value::Array(sent));
    }
    Ok(Reply {
        native: Some(Value::Object(native)),
        shown,
        calls,
        cut: finish == "length",
    })
}

fn append(block: &mut Value, key: &str, more: &str) {
    let joined = format!("{}{more}", block[key].as_str().unwrap_or_default());
    block[key] = json!(joined);
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
