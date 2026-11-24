use futures_util::stream;

use super::*;

/// Decodes `body`, as `api` streams it, split at `cuts`.
async fn decode(api: Api, body: &str, cuts: &[usize]) -> Result<Reply, Failure> {
    let mut pieces = Vec::new();
    let mut at = 0;
    for &cut in cuts.iter().chain([&body.len()]) {
        pieces.push(Ok(bytes::Bytes::copy_from_slice(&body.as_bytes()[at..cut])));
        at = cut;
    }
    let mut sse = Sse::new(stream::iter(pieces));
    match api {
        Api::Anthropic => anthropic(&mut sse).await,
        Api::OpenAi => openai(&mut sse).await,
    }
}

/// Every split of `body` in two decodes as the whole does.
async fn same_at_every_split(api: Api, body: &str) -> Reply {
    let whole = decode(api, body, &[]).await.unwrap();
    for cut in 1..body.len() {
        let split = decode(api, body, &[cut]).await.unwrap();
        assert_eq!(split.native, whole.native, "split at {cut}");
        assert_eq!(split.calls, whole.calls, "split at {cut}");
    }
    whole
}

/// Recorded from the Messages API's documented stream shape: signed thinking, text, a tool call
/// whose input streams in pieces, and a ping.
const ANTHROPIC_STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","model":"claude-x","content":[],"usage":{"input_tokens":10}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Look it "}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"up."}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig=="}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: ping
data: {"type": "ping"}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Searching é."}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"web_search","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"nv"}}

event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"idia\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":2}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn an_anthropic_reply_keeps_signed_thinking_and_assembles_its_tool_input() {
    let reply = same_at_every_split(Api::Anthropic, ANTHROPIC_STREAM).await;
    assert_eq!(reply.shown.len(), 2, "tool calls are shown as they run");
    assert_eq!(
        reply.native.unwrap(),
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Look it up.", "signature": "sig=="},
            {"type": "text", "text": "Searching é."},
            {"type": "tool_use", "id": "toolu_1", "name": "web_search", "input": {"query": "nvidia"}},
        ]})
    );
    assert_eq!(
        reply.calls,
        [Call {
            id: "toolu_1".into(),
            name: "web_search".into(),
            input: Ok(json!({"query": "nvidia"}))
        }]
    );
    assert!(!reply.cut);
}

#[tokio::test]
async fn anthropic_thinking_cut_before_its_signature_is_not_sent_back() {
    let body = r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}
data: {"type":"message_delta","delta":{"stop_reason":"max_tokens"}}
data: {"type":"message_stop"}
"#;
    let reply = decode(Api::Anthropic, body, &[]).await.unwrap();
    assert_eq!(
        reply.native, None,
        "an empty turn is refused, so none is sent back"
    );
    assert!(reply.cut);
}

#[tokio::test]
async fn a_stream_that_stops_early_or_is_overloaded_is_retried_but_a_bad_request_is_not() {
    let early = "data: {\"type\":\"message_start\",\"message\":{}}\n";
    assert!(decode(Api::Anthropic, early, &[]).await.unwrap_err().retry);
    let error =
        |kind: &str| format!("data: {{\"type\":\"error\",\"error\":{{\"type\":\"{kind}\"}}}}\n");
    let overloaded = decode(Api::Anthropic, &error("overloaded_error"), &[]).await;
    assert!(overloaded.unwrap_err().retry);
    let invalid = decode(Api::Anthropic, &error("invalid_request_error"), &[]).await;
    assert!(
        !invalid.unwrap_err().retry,
        "a context overflow fails the same way again"
    );
    let openai =
        "data: {\"error\":{\"message\":\"too long\",\"type\":\"invalid_request_error\"}}\n";
    assert!(!decode(Api::OpenAi, openai, &[]).await.unwrap_err().retry);
}

/// The Chat Completions stream shape vLLM, DeepSeek and OpenAI send: reasoning, text, and two
/// tool calls whose names and arguments arrive in pieces by index.
const OPENAI_STREAM: &str = r#"data: {"id":"c1","choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":"Need "}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"reasoning_content":"data."}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"content":"Let me check."}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"bash","arguments":""}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"read","arguments":"{\"path\":\"a\"}"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" \"ls\"}"}}]}}]}

data: {"id":"c1","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]

"#;

#[tokio::test]
async fn an_openai_reply_keeps_its_reasoning_field_and_joins_calls_by_index() {
    let reply = same_at_every_split(Api::OpenAi, OPENAI_STREAM).await;
    assert_eq!(
        reply.native.unwrap(),
        json!({"role": "assistant", "content": "Let me check.", "reasoning_content": "Need data.",
        "tool_calls": [
            {"id": "call_a", "type": "function", "function": {"name": "bash", "arguments": "{\"command\": \"ls\"}"}},
            {"id": "call_b", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}},
        ]})
    );
    let names: Vec<_> = reply.calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["bash", "read"]);
    assert_eq!(reply.calls[0].input, Ok(json!({"command": "ls"})));
}

#[tokio::test]
async fn an_openai_call_with_broken_arguments_is_reported_not_run() {
    let body = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c","function":{"name":"bash","arguments":"{\"comm"}}]},"finish_reason":"length"}]}
data: [DONE]
"#;
    let reply = decode(Api::OpenAi, body, &[]).await.unwrap();
    assert!(reply.cut);
    assert!(
        reply.calls[0]
            .input
            .as_ref()
            .unwrap_err()
            .contains("not valid JSON")
    );
    let native = reply.native.unwrap();
    assert_eq!(native["tool_calls"][0]["function"]["arguments"], "{}");
    assert_eq!(
        native["content"],
        Value::Null,
        "content may be null beside tool calls"
    );
}

#[tokio::test]
async fn an_openai_reply_cut_in_its_reasoning_goes_back_with_content() {
    let body = r#"data: {"choices":[{"delta":{"reasoning_content":"hmm","reasoning":"hmm"}}]}
data: {"choices":[{"delta":{},"finish_reason":"length"}]}
data: [DONE]
"#;
    let reply = decode(Api::OpenAi, body, &[]).await.unwrap();
    assert!(reply.cut && reply.calls.is_empty());
    assert_eq!(
        reply.native.unwrap(),
        json!({"role": "assistant", "content": "", "reasoning_content": "hmm"}),
        "one reasoning field, and content a string without tool calls"
    );
}

#[tokio::test]
async fn openai_call_fragments_without_an_index_continue_the_call_they_follow() {
    let body = r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c1","function":{"name":"Bash","arguments":"{\"comm"}}]}}]}
data: {"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"and\": \"ls\"}"}}]}}]}
data: {"choices":[{"delta":{"tool_calls":[{"id":"c2","function":{"name":"Read","arguments":"{}"}}]}}]}
data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}
"#;
    let reply = decode(Api::OpenAi, body, &[]).await.unwrap();
    let calls: Vec<_> = reply
        .calls
        .iter()
        .map(|c| (c.id.as_str(), c.input.clone()))
        .collect();
    assert_eq!(
        calls,
        [("c1", Ok(json!({"command": "ls"}))), ("c2", Ok(json!({})))]
    );
}

#[tokio::test]
async fn an_openai_stream_without_a_finish_is_retried() {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n";
    assert!(decode(Api::OpenAi, body, &[]).await.unwrap_err().retry);
}

fn env(base: &str, api: Option<&str>) -> impl Fn(&str) -> Option<String> {
    let (base, api) = (base.to_owned(), api.map(str::to_owned));
    move |name: &str| match name {
        BASE_ENV => Some(base.clone()),
        KEY_ENV => Some("k".to_owned()),
        API_ENV => api.clone(),
        _ => None,
    }
}

#[test]
fn each_api_takes_its_base_url_as_its_own_sdk_does() {
    let url = |model, base| Llm::from_env(model, env(base, None)).unwrap().url;
    assert_eq!(
        url("claude-x", "https://api.anthropic.com"),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(
        url("claude-x", "https://gw.example/v1/"),
        "https://gw.example/v1/messages"
    );
    assert_eq!(
        url("gpt-x", "https://api.openai.com/v1"),
        "https://api.openai.com/v1/chat/completions"
    );
    assert_eq!(
        url("gpt-x", "https://h.example"),
        "https://h.example/v1/chat/completions"
    );
    assert_eq!(
        url("gpt-x", "https://openrouter.ai/api/v1"),
        "https://openrouter.ai/api/v1/chat/completions"
    );
    assert_eq!(
        url("gpt-x", "https://gw.example/openai"),
        "https://gw.example/openai/chat/completions"
    );
    assert!(Llm::from_env("m", env("ftp://x", None)).is_err());
}

#[test]
fn the_api_is_named_or_follows_the_model() {
    let api = |model, named| {
        Llm::from_env(model, env("https://h.example", named))
            .unwrap()
            .api
    };
    assert_eq!(api("claude-sonnet-5-5", None), Api::Anthropic);
    assert_eq!(api("anthropic/claude-x", None), Api::Anthropic);
    assert_eq!(api("Anthropic.claude-x", None), Api::Anthropic);
    assert_eq!(api("gpt-6-sol", None), Api::OpenAi);
    assert_eq!(api("openrouter/anthropic/claude-x", None), Api::OpenAi);
    assert_eq!(api("claude-x", Some("OpenAI")), Api::OpenAi);
    assert_eq!(api("gpt-x", Some("anthropic")), Api::Anthropic);
    assert!(Llm::from_env("m", env("https://h.example", Some("gemini"))).is_err());
    let missing = Llm::from_env("m", |_| None).err().unwrap().to_string();
    assert!(
        missing.contains(BASE_ENV) && missing.contains(KEY_ENV),
        "{missing}"
    );
    let no_key = |name: &str| (name == BASE_ENV).then(|| "https://h.example".to_owned());
    let no_key = Llm::from_env("m", no_key).err().unwrap().to_string();
    assert_eq!(no_key, format!("set {KEY_ENV} to the model endpoint's key"));
}

#[test]
fn results_go_back_in_each_apis_shape() {
    let results = [ToolResult {
        id: "t1".into(),
        text: "ok".into(),
        failed: false,
    }];
    let mut a = Conversation::new(Api::Anthropic, "sys".into(), "hi");
    a.push_results(&results, Some("note"));
    let body = a.request("m", &[], true);
    assert_eq!(body["tool_choice"], json!({"type": "none"}));
    assert_eq!(body["messages"][1]["content"][0]["tool_use_id"], "t1");
    assert_eq!(body["messages"][1]["content"][1]["text"], "note");
    assert!(body["messages"][1]["content"][1]["cache_control"].is_object());

    let mut o = Conversation::new(Api::OpenAi, "sys".into(), "hi");
    o.push_results(&results, Some("note"));
    let body = o.request("m", &[], false);
    let roles: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].clone())
        .collect();
    assert_eq!(roles, ["system", "user", "tool", "user"]);
    assert!(body.get("tool_choice").is_none());
}

#[tokio::test]
async fn a_redirect_is_refused_so_the_key_never_reaches_another_origin() {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::http::HeaderMap;
    use axum::response::Redirect;
    use axum::routing::post;
    use tokio::net::TcpListener;

    async fn spawn(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }
    let seen: Arc<Mutex<Vec<HeaderMap>>> = Arc::default();
    let kept = seen.clone();
    let other = spawn(Router::new().fallback(post(move |headers: HeaderMap| {
        kept.lock().unwrap().push(headers);
        async { "" }
    })))
    .await;
    let to = format!("{other}/v1/messages");
    let first =
        spawn(Router::new().fallback(post(move || async move { Redirect::temporary(&to) }))).await;
    let env = |name: &str| match name {
        BASE_ENV => Some(first.clone()),
        KEY_ENV => Some("secret-key".to_owned()),
        _ => None,
    };
    let llm = Llm::from_env("claude-x", env).unwrap();
    let convo = Conversation::new(Api::Anthropic, "sys".into(), "hi");
    let failed = llm.sample(&convo, &[], false).await.unwrap_err();
    assert!(
        failed.message.contains("307") && !failed.retry,
        "{failed:?}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the other origin was never called"
    );
}
