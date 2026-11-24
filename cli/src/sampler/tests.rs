//! A whole run against a scripted model endpoint and a fake sandbox API whose commands run on this
//! host in a temp directory standing for the box's working directory.

use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use epochal_api::Client;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

use super::*;
use crate::output::events::Screen;
use crate::output::progress::{Kind, Out};
use crate::session::Harness;

const AS_OF: &str = "2025-03-12T23:59:59Z";
const CWD: &str = "/home/epochal";

async fn spawn(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

/// The sandbox and time-portal API: one live box whose commands run in `dir`, each answering
/// with all its output at once.
async fn api(dir: Arc<TempDir>) -> String {
    let outputs: Arc<Mutex<Vec<Value>>> = Arc::default();
    let info = json!({"id": "sbx_1", "state": "live", "as_of": AS_OF, "network": "none",
        "vcpus": 1, "memory_gib": 1, "timeout": "1h", "idle_timeout": "10m",
        "created_at": AS_OF});
    let exec = move |State(outputs): State<Arc<Mutex<Vec<Value>>>>, body: Bytes| {
        let dir = dir.clone();
        async move {
            let req: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                req["cwd"], CWD,
                "every command runs in the working directory"
            );
            let argv: Vec<String> = serde_json::from_value(req["argv"].clone()).unwrap();
            let stdin = STANDARD
                .decode(req["stdin_b64"].as_str().unwrap_or(""))
                .unwrap();
            let mut child = tokio::process::Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(dir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(&stdin).await.unwrap();
            let out = child.wait_with_output().await.unwrap();
            let frame = |stream, data: &[u8]| json!({"stream": stream, "offset": 0, "data_b64": STANDARD.encode(data)});
            let code = out.status.code().unwrap();
            let mut outputs = outputs.lock().unwrap();
            outputs.push(
                json!({"frames": [frame("stdout", &out.stdout), frame("stderr", &out.stderr)],
                "exit": {"reason": "exited", "code": code}}),
            );
            json!({"id": format!("e{}", outputs.len() - 1)}).to_string()
        }
    };
    let output =
        |State(outputs): State<Arc<Mutex<Vec<Value>>>>,
         axum::extract::Path((_, e)): axum::extract::Path<(String, String)>| async move {
            let i: usize = e[1..].parse().unwrap();
            outputs.lock().unwrap()[i].to_string()
        };
    let search = || async {
        json!({"as_of": AS_OF, "request_id": "r", "entities": [], "results": [
            {"title": "Nvidia revenue", "url": "https://n.example/q4", "snippet": "Record $39.3B quarter."}]})
        .to_string()
    };
    let fetch = || async {
        json!({"as_of": AS_OF, "request_id": "r", "status": "success", "url": "https://n.example/q4",
            "final_url": "https://n.example/q4", "title": "Q4", "content": "w".repeat(60_000),
            "truncated": false})
        .to_string()
    };
    let p = "/v1/time-portal/sandbox";
    let created = info.to_string();
    spawn(
        Router::new()
            .route(p, post(move || async move { created }))
            .route(
                &format!("{p}/{{id}}"),
                delete(|| async { StatusCode::NO_CONTENT }),
            )
            .route(&format!("{p}/{{id}}/execs"), post(exec))
            .route(&format!("{p}/{{id}}/execs/{{e}}/output"), get(output))
            .route("/v1/time-portal/search", post(search))
            .route("/v1/time-portal/fetch", post(fetch))
            .with_state(outputs),
    )
    .await
}

/// A model that first fails once, then asks to write a file, then to read it back with bash and
/// search, then answers with what it saw. Each request body is kept.
async fn model(api: model::Api, seen: Arc<Mutex<Vec<Value>>>) -> String {
    let count = Arc::new(AtomicUsize::new(0));
    let reply = move |body: Bytes| {
        let (seen, count) = (seen.clone(), count.clone());
        async move {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    [("retry-after", "0")],
                    "busy",
                )
                    .into_response();
            }
            seen.lock()
                .unwrap()
                .push(serde_json::from_slice(&body).unwrap());
            let calls: Vec<(&str, Value)> = match n {
                1 => vec![(
                    "Write",
                    json!({"file_path": "notes/a.txt", "content": "hello é\n"}),
                )],
                2 => vec![
                    (
                        "Bash",
                        json!({"command": "cat notes/a.txt; echo oops >&2; exit 3"}),
                    ),
                    ("web_search", json!({"query": "nvidia revenue"})),
                ],
                _ => vec![],
            };
            stream(api, n, &calls, "Done: hello é")
        }
    };
    spawn(Router::new().fallback(post(reply))).await
}

fn stream(api: model::Api, n: usize, calls: &[(&str, Value)], text: &str) -> Response {
    let mut out = String::new();
    let mut event = |v: Value| out.push_str(&format!("data: {v}\n\n"));
    match api {
        model::Api::Anthropic => {
            if calls.is_empty() {
                event(
                    json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
                );
                event(
                    json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}),
                );
            }
            for (i, (name, input)) in calls.iter().enumerate() {
                event(json!({"type": "content_block_start", "index": i,
                    "content_block": {"type": "tool_use", "id": format!("t{n}{i}"), "name": name, "input": {}}}));
                event(json!({"type": "content_block_delta", "index": i,
                    "delta": {"type": "input_json_delta", "partial_json": input.to_string()}}));
            }
            event(
                json!({"type": "message_delta", "delta": {"stop_reason": if calls.is_empty() {"end_turn"} else {"tool_use"}}}),
            );
            event(json!({"type": "message_stop"}));
        }
        model::Api::OpenAi => {
            if calls.is_empty() {
                event(json!({"choices": [{"delta": {"content": text}}]}));
            }
            for (i, (name, input)) in calls.iter().enumerate() {
                event(
                    json!({"choices": [{"delta": {"tool_calls": [{"index": i, "id": format!("t{n}{i}"),
                    "function": {"name": name, "arguments": input.to_string()}}]}}]}),
                );
            }
            event(
                json!({"choices": [{"delta": {}, "finish_reason": if calls.is_empty() {"stop"} else {"tool_calls"}}]}),
            );
            out.push_str("data: [DONE]\n\n");
        }
    }
    ([("content-type", "text/event-stream")], out).into_response()
}

#[derive(Default)]
struct Events(Vec<Vec<u8>>);

impl Listener for Events {
    fn event(&mut self, raw: Vec<u8>) {
        self.0.push(raw);
    }
    fn thinking(&mut self) {}
}

struct Quiet;

impl Out for Quiet {
    fn line(&mut self, _: Kind, _: &str, _: &str) {}
    fn status(&mut self, _: &str, _: &str) {}
}

/// The answer the screen takes from `events`.
fn answer(events: &Events) -> String {
    let mut screen = Screen::new(Harness::Claude, Quiet);
    for raw in &events.0 {
        screen.event(raw);
    }
    screen.answer().to_owned()
}

/// Runs the built-in harness on `api` to its answer: the events it reported, the requests the
/// model saw, and the box's working directory.
async fn run(api: model::Api, max_turns: Option<u32>) -> (Events, Vec<Value>, Arc<TempDir>) {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    let client = Client::builder()
        .with_api_key("k")
        .with_base_url(self::api(dir.clone()).await)
        .build()
        .unwrap();
    let seen = Arc::default();
    let base = model(api, Arc::clone(&seen)).await;
    let name = match api {
        model::Api::Anthropic => "anthropic",
        model::Api::OpenAi => "openai",
    };
    let llm = Llm::from_env("m", |var| match var {
        model::BASE_ENV => Some(base.clone()),
        model::KEY_ENV => Some("key".into()),
        model::API_ENV => Some(name.into()),
        _ => None,
    })
    .unwrap();
    let as_of = AS_OF.parse().unwrap();
    let sandbox = Sandbox::time_portal(as_of)
        .with_client(client.clone())
        .build()
        .await
        .unwrap();
    let web = Web::time_portal(as_of).with_client(client).build().unwrap();
    let run = Run {
        llm: &llm,
        prompt: "What did you write?",
        sandbox: &sandbox,
        web,
        cwd: CWD,
        max_turns,
    };
    let mut events = Events::default();
    run.drive(&mut events).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    (events, seen, dir)
}

#[tokio::test]
async fn an_anthropic_run_works_in_the_box_and_answers() {
    let (events, seen, dir) = run(model::Api::Anthropic, None).await;
    assert_eq!(answer(&events), "Done: hello é");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes/a.txt")).unwrap(),
        "hello é\n"
    );
    assert_eq!(seen.len(), 3, "the 503 was asked again, then three turns");
    let system = seen[0]["system"][0]["text"].as_str().unwrap();
    assert!(
        system.contains("2025-03-12") && system.contains(CWD),
        "{system}"
    );
    let names: Vec<_> = seen[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].clone())
        .collect();
    assert_eq!(
        names,
        ["Bash", "Read", "Write", "Edit", "web_fetch", "web_search"]
    );
    let results = &seen[2]["messages"][4]["content"];
    assert_eq!(results[0]["tool_use_id"], "t20");
    let bash = results[0]["content"].as_str().unwrap();
    assert_eq!(bash, "hello é\noops\n[exit code: 3]");
    assert_eq!(results[0]["is_error"], true);
    assert!(
        results[1]["content"]
            .as_str()
            .unwrap()
            .contains("Record $39.3B quarter.")
    );
    assert_eq!(
        seen[2]["messages"][3]["content"][0]["type"], "tool_use",
        "the reply went back as sent"
    );
}

#[tokio::test]
async fn each_call_is_shown_as_it_runs_with_its_result_under_it() {
    let (events, _, _dir) = run(model::Api::OpenAi, None).await;
    let kinds: Vec<String> = events
        .0
        .iter()
        .map(|raw| {
            let e: Value = serde_json::from_slice(raw).unwrap();
            let block = &e["message"]["content"][0];
            format!(
                "{} {}",
                block["type"].as_str().unwrap(),
                block["name"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "tool_use Write",
            "tool_result ",
            "tool_use Bash",
            "tool_result ",
            "tool_use web_search",
            "tool_result ",
            "text ",
        ]
    );
}

#[tokio::test]
async fn a_read_of_one_huge_line_says_it_was_cut_and_how_to_read_on() {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    std::fs::write(dir.path().join("one.txt"), "z".repeat(100_000)).unwrap();
    let client = Client::builder()
        .with_api_key("k")
        .with_base_url(api(dir.clone()).await)
        .build()
        .unwrap();
    let as_of = AS_OF.parse().unwrap();
    let sandbox = Sandbox::time_portal(as_of)
        .with_client(client.clone())
        .build()
        .await
        .unwrap();
    let computer = Computer {
        sandbox: &sandbox,
        web: Web::time_portal(as_of).with_client(client).build().unwrap(),
        cwd: CWD,
    };
    let (text, failed) = computer.call("Read", json!({"file_path": "one.txt"})).await;
    assert!(!failed && text.len() < 30_000, "{}", text.len());
    assert!(text.starts_with("     1\tzzz"));
    assert!(
        text.ends_with("read the rest with Bash, e.g. `cut -c 29721- one.txt`]\n"),
        "{}",
        &text[text.len() - 200..]
    );
    assert!(
        !text.contains("[long line"),
        "the script's note is replaced"
    );
    let shown = text.lines().next().unwrap().len() - 7;
    assert_eq!(
        shown, 29_720,
        "the hint's column follows the last one shown"
    );
}

#[tokio::test]
async fn a_fetched_page_reaches_the_model_whole_past_the_command_cap() {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    let client = Client::builder()
        .with_api_key("k")
        .with_base_url(api(dir.clone()).await)
        .build()
        .unwrap();
    let as_of = AS_OF.parse().unwrap();
    let sandbox = Sandbox::time_portal(as_of)
        .with_client(client.clone())
        .build()
        .await
        .unwrap();
    let computer = Computer {
        sandbox: &sandbox,
        web: Web::time_portal(as_of).with_client(client).build().unwrap(),
        cwd: CWD,
    };
    let args = json!({"url": "https://n.example/q4", "max_characters": 100_000});
    let (text, failed) = computer.call("web_fetch", args).await;
    assert!(!failed, "{text}");
    assert!(text.ends_with(&"w".repeat(60_000)) && !text.contains("bytes cut"));
}

#[tokio::test]
async fn a_file_that_quotes_the_long_line_note_reads_whole() {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    std::fs::write(dir.path().join("q.txt"), "a\n[long line 1]\nb\n").unwrap();
    let client = Client::builder()
        .with_api_key("k")
        .with_base_url(api(dir.clone()).await)
        .build()
        .unwrap();
    let as_of = AS_OF.parse().unwrap();
    let sandbox = Sandbox::time_portal(as_of)
        .with_client(client.clone())
        .build()
        .await
        .unwrap();
    let computer = Computer {
        sandbox: &sandbox,
        web: Web::time_portal(as_of).with_client(client).build().unwrap(),
        cwd: CWD,
    };
    let (text, failed) = computer.call("Read", json!({"file_path": "q.txt"})).await;
    assert!(!failed);
    assert_eq!(text, "     1\ta\n     2\t[long line 1]\n     3\tb\n");
}

#[tokio::test]
async fn an_openai_run_sends_results_as_tool_messages() {
    let (events, seen, _dir) = run(model::Api::OpenAi, None).await;
    assert_eq!(answer(&events), "Done: hello é");
    let messages = seen[2]["messages"].as_array().unwrap();
    let roles: Vec<_> = messages
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        [
            "system",
            "user",
            "assistant",
            "tool",
            "assistant",
            "tool",
            "tool"
        ]
    );
    assert_eq!(messages[5]["tool_call_id"], "t20");
    assert!(
        messages[6]["content"]
            .as_str()
            .unwrap()
            .contains("Nvidia revenue")
    );
}

#[tokio::test]
async fn the_last_turn_is_told_and_forbidden_tools() {
    let (_, seen, _dir) = run(model::Api::Anthropic, Some(2)).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["tool_choice"], json!({"type": "none"}));
    let note = &seen[1]["messages"][2]["content"][1]["text"];
    assert_eq!(note, LAST_TURN);
}
