//! An in-process sandbox API. A box is a temp directory: a box path `/x` is `<root>/x`, in file
//! calls and in every argument and `cwd` of a command, which runs on the host. Commands named
//! [`RELAY`] run the fake relay instead. Output comes back in frames of at most [`FRAME`] bytes,
//! each poll repeating the byte before the asked offset, as a server answering a retried poll may.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::header::{CONTENT_RANGE, RANGE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc};

use crate::relay;

pub const KEY: &str = "good-key";
pub const RELAY: &str = "/opt/epochal/bin/epochal-relay";
/// An as-of whose box never boots.
pub const NO_BOOT: &str = "2001-01-01T00:00:00Z";
pub const FRAME: usize = 7;
/// The most bytes one ranged read returns, so a file comes back over several.
pub const RANGE_CAP: u64 = 4096;
const PREFIX: &str = "/v1/time-portal/sandbox";
const SIGKILL: i32 = 9;

/// A request the fake saw.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

pub struct Fake {
    pub url: String,
    state: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    boxes: Mutex<HashMap<String, FakeBox>>,
    log: Mutex<Vec<Seen>>,
    /// Each `Idempotency-Key` answered, and its answer, replayed to a retry as the API does.
    answered: Mutex<HashMap<String, (StatusCode, Bytes)>>,
    /// How many more calls to carry out but answer with a 503, as a reply lost on the way back.
    flaky: Mutex<u32>,
    /// How long each file read or write takes.
    file_delay: Mutex<Duration>,
    /// How long each exec input waits before it is applied.
    input_delay: Mutex<Duration>,
    /// The most bytes a range read past the first byte returns, when set.
    short_ranges: Mutex<Option<u64>>,
    file_calls: AtomicU64,
    peak_file_calls: AtomicU64,
    /// The offset of each write, in the order they started.
    writes: Mutex<Vec<u64>>,
    write_started: Notify,
    /// A write offset to hold until the write at the second one starts.
    hold: Mutex<Option<(u64, u64)>>,
    /// Whether that write started while the held one was held.
    passed_held: Mutex<Option<bool>>,
}

struct FakeBox {
    root: TempDir,
    info: Value,
    execs: HashMap<String, Arc<Exec>>,
}

impl Fake {
    pub async fn start() -> Fake {
        let state = Arc::new(Shared::default());
        let app = Router::new()
            .route(PREFIX, post(create))
            .route(&format!("{PREFIX}/{{id}}"), get(read).delete(terminate))
            .route(&format!("{PREFIX}/{{id}}/execs"), post(start_exec))
            .route(
                &format!("{PREFIX}/{{id}}/execs/{{exec}}/output"),
                get(output),
            )
            .route(
                &format!("{PREFIX}/{{id}}/execs/{{exec}}/input"),
                post(input),
            )
            .route(
                &format!("{PREFIX}/{{id}}/files"),
                get(read_file).put(write_file).delete(remove_file),
            )
            .route(&format!("{PREFIX}/{{id}}/files/stat"), get(stat))
            .layer(middleware::from_fn_with_state(state.clone(), gate))
            .layer(DefaultBodyLimit::max(8 << 20))
            .with_state(state.clone());
        Fake {
            url: serve(app).await,
            state,
        }
    }

    pub fn log(&self) -> Vec<Seen> {
        self.state.log.lock().unwrap().clone()
    }

    /// Carries out each of the next `calls` calls but answers it with a 503.
    pub fn lose_replies(&self, calls: u32) {
        *self.state.flaky.lock().unwrap() = calls;
    }

    /// Makes each file read or write take `delay`, so calls that overlap show in
    /// [`peak_file_calls`](Self::peak_file_calls).
    pub fn delay_files(&self, delay: Duration) {
        *self.state.file_delay.lock().unwrap() = delay;
    }

    /// Makes each exec input wait `delay` before it is applied; one whose caller gave up first
    /// is never applied.
    pub fn delay_inputs(&self, delay: Duration) {
        *self.state.input_delay.lock().unwrap() = delay;
    }

    /// Caps each range read that doesn't start at byte 0 at `bytes`, while still naming the
    /// whole size.
    pub fn short_ranges(&self, bytes: u64) {
        *self.state.short_ranges.lock().unwrap() = Some(bytes);
    }

    /// Holds the write at `offset` until the write at `until` has started, or 10 s pass.
    pub fn hold_write(&self, offset: u64, until: u64) {
        *self.state.hold.lock().unwrap() = Some((offset, until));
    }

    /// Whether the write at `until` started while the held one was held.
    pub fn passed_held_write(&self) -> Option<bool> {
        *self.state.passed_held.lock().unwrap()
    }

    /// The most file reads and writes that were in flight at once.
    pub fn peak_file_calls(&self) -> u64 {
        self.state.peak_file_calls.load(Ordering::SeqCst)
    }

    /// The host directory standing for box `id`'s `/`.
    pub fn root(&self, id: &str) -> PathBuf {
        fs::canonicalize(self.state.boxes.lock().unwrap()[id].root.path()).unwrap()
    }

    pub fn box_state(&self, id: &str) -> String {
        let boxes = self.state.boxes.lock().unwrap();
        boxes[id].info["state"].as_str().unwrap().to_owned()
    }

    /// Each command run in box `id`: its argv and how it ended, if it has.
    pub fn execs(&self, id: &str) -> Vec<(Vec<String>, Option<Value>)> {
        let boxes = self.state.boxes.lock().unwrap();
        let mut execs: Vec<_> = boxes[id].execs.iter().collect();
        execs.sort_by_key(|(id, _)| id.len().to_string() + id);
        execs
            .into_iter()
            .map(|(_, e)| (e.argv.clone(), e.out.lock().unwrap().exit.clone()))
            .collect()
    }
}

/// Serves `app` on a free local port and returns its base URL.
pub async fn serve(app: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

pub fn api_error(status: StatusCode, kind: &str, message: &str) -> Response {
    let body = json!({"error": {"type": kind, "message": message, "request_id": "req_1"}});
    (status, Json(body)).into_response()
}

/// An error reply out of a helper, boxed to keep its `Result` small.
type Refused = Box<Response>;

fn refused() -> Refused {
    Box::new(not_found())
}

fn not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not_found", "No such object.")
}

/// Logs every request and refuses a wrong key, as the API's edge does.
async fn gate(State(state): State<Arc<Shared>>, request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    state.log.lock().unwrap().push(Seen {
        method: parts.method.to_string(),
        path: parts.uri.to_string(),
        headers: parts.headers.clone(),
        body: body.clone(),
    });
    if parts.headers.get("x-api-key").is_none_or(|k| k != KEY) {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "No key matches that token.",
        );
    }
    let key = parts.headers.get("idempotency-key").map(|k| {
        format!(
            "{} {} {}",
            parts.method,
            parts.uri.path(),
            k.to_str().unwrap()
        )
    });
    let earlier = key
        .as_ref()
        .and_then(|k| state.answered.lock().unwrap().get(k).cloned());
    let response = match earlier {
        Some((status, reply)) => (
            status,
            Json(serde_json::from_slice::<Value>(&reply).unwrap()),
        )
            .into_response(),
        None => {
            let response = next.run(Request::from_parts(parts, body.into())).await;
            let Some(key) = key.filter(|_| response.status().is_success()) else {
                return lose(&state, response);
            };
            let (head, body) = response.into_parts();
            let reply = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            state
                .answered
                .lock()
                .unwrap()
                .insert(key, (head.status, reply.clone()));
            Response::from_parts(head, reply.into())
        }
    };
    lose(&state, response)
}

/// `response`, or a 503 in its place while replies are being lost.
fn lose(state: &Shared, response: Response) -> Response {
    let mut flaky = state.flaky.lock().unwrap();
    if *flaky == 0 {
        return response;
    }
    *flaky -= 1;
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

async fn create(State(state): State<Arc<Shared>>, headers: HeaderMap, body: Bytes) -> Response {
    let as_of = headers["accept-datetime"].to_str().unwrap().to_owned();
    let body: Value = serde_json::from_slice(&body).unwrap();
    let apps: Vec<Value> = body["apps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|name| json!({"name": name, "version": "1.4.2"}))
        .collect();
    let mut boxes = state.boxes.lock().unwrap();
    let id = format!("sbx_{}", boxes.len() + 1);
    let info = json!({
        "id": id,
        "state": "creating",
        "as_of": as_of,
        "network": body.get("network").cloned().unwrap_or(json!("none")),
        "vcpus": body.get("vcpus").cloned().unwrap_or(json!(1)),
        "memory_gib": body.get("memory_gib").cloned().unwrap_or(json!(2)),
        "timeout": body.get("timeout").cloned().unwrap_or(json!("1h")),
        "idle_timeout": body.get("idle_timeout").cloned().unwrap_or(json!("10m")),
        "created_at": "2026-11-09T10:00:00Z",
        "apps": apps,
    });
    let root = TempDir::new().unwrap();
    for dir in ["tmp", "work"] {
        fs::create_dir(root.path().join(dir)).unwrap();
    }
    let reply = info.clone();
    boxes.insert(
        id,
        FakeBox {
            root,
            info,
            execs: HashMap::new(),
        },
    );
    (StatusCode::ACCEPTED, Json(reply)).into_response()
}

/// A box answers `creating` once, then boots, unless its as-of is [`NO_BOOT`].
async fn read(State(state): State<Arc<Shared>>, UrlPath(id): UrlPath<String>) -> Response {
    let mut boxes = state.boxes.lock().unwrap();
    let Some(b) = boxes.get_mut(&id) else {
        return not_found();
    };
    if b.info["state"] == "creating" {
        if b.info["as_of"] == NO_BOOT {
            b.info["state"] = json!("failed");
            b.info["error"] = json!({"type": "sandbox_lost", "message": "The host went away."});
        } else {
            b.info["state"] = json!("live");
        }
    }
    Json(b.info.clone()).into_response()
}

async fn terminate(State(state): State<Arc<Shared>>, UrlPath(id): UrlPath<String>) -> Response {
    let mut boxes = state.boxes.lock().unwrap();
    let Some(b) = boxes.get_mut(&id) else {
        return not_found();
    };
    b.info["state"] = json!("ended");
    for exec in b.execs.values() {
        let _ = exec.signals.send(SIGKILL);
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Box `id`'s root, if it is live.
fn live_root(state: &Shared, id: &str) -> Result<PathBuf, Refused> {
    let boxes = state.boxes.lock().unwrap();
    let b = boxes.get(id).ok_or_else(refused)?;
    if b.info["state"] != "live" {
        let conflict = api_error(StatusCode::CONFLICT, "conflict", "The sandbox is not live.");
        return Err(Box::new(conflict));
    }
    Ok(b.root.path().to_owned())
}

fn reroot(root: &Path, arg: &str) -> String {
    match arg.starts_with('/') {
        true => format!("{}{arg}", root.display()),
        false => arg.to_owned(),
    }
}

pub struct Exec {
    argv: Vec<String>,
    pub out: Mutex<Buffers>,
    pub changed: Notify,
    stdin: AsyncMutex<(u64, Option<Box<dyn AsyncWrite + Send + Unpin>>)>,
    signals: mpsc::UnboundedSender<i32>,
}

#[derive(Default)]
pub struct Buffers {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: Option<Value>,
}

impl Exec {
    pub fn append(&self, stderr: bool, data: &[u8]) {
        let mut out = self.out.lock().unwrap();
        match stderr {
            true => out.stderr.extend_from_slice(data),
            false => out.stdout.extend_from_slice(data),
        }
        drop(out);
        self.changed.notify_waiters();
    }

    fn end(&self, exit: Value) {
        self.out.lock().unwrap().exit.get_or_insert(exit);
        self.changed.notify_waiters();
    }
}

#[derive(Deserialize)]
struct CreateExec {
    argv: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    cwd: Option<String>,
    #[serde(default)]
    stdin_open: bool,
    stdin_b64: Option<String>,
}

async fn start_exec(
    State(state): State<Arc<Shared>>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<CreateExec>,
) -> Response {
    let root = match live_root(&state, &id) {
        Ok(root) => root,
        Err(refused) => return *refused,
    };
    let (signals, mut signalled) = mpsc::unbounded_channel();
    let argv0 = body.argv[0].clone();
    let exec = if argv0 == RELAY {
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let (mut from_relay, to_relay) = tokio::io::split(ours);
        let exec = new_exec(&body, Some(Box::new(to_relay)), signals);
        let port: u16 = body.argv[1].parse().unwrap();
        let out = exec.clone();
        tokio::spawn(async move {
            let mut buf = vec![0; 64 << 10];
            while let Ok(n @ 1..) = from_relay.read(&mut buf).await {
                out.append(false, &buf[..n]);
            }
        });
        let ended = exec.clone();
        tokio::spawn(async move {
            let exit = tokio::select! {
                () = relay::run(port, theirs) => json!({"reason": "exited", "code": 1}),
                Some(signal) = signalled.recv() => json!({"reason": "signaled", "signal": signal}),
            };
            ended.end(exit);
        });
        exec
    } else {
        let mut cmd = Command::new(reroot(&root, &argv0));
        cmd.args(body.argv[1..].iter().map(|a| reroot(&root, a)))
            .envs(&body.env)
            .current_dir(reroot(&root, body.cwd.as_deref().unwrap_or("/")))
            .stdin(match body.stdin_open || body.stdin_b64.is_some() {
                true => Stdio::piped(),
                false => Stdio::null(),
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => panic!("the fake could not run {:?}: {e}", body.argv),
        };
        let mut stdin = child.stdin.take();
        if let (Some(pipe), Some(data)) = (stdin.as_mut(), &body.stdin_b64) {
            pipe.write_all(&STANDARD.decode(data).unwrap())
                .await
                .unwrap();
        }
        let stdin = stdin
            .filter(|_| body.stdin_open)
            .map(|s| Box::new(s) as Box<dyn AsyncWrite + Send + Unpin>);
        let exec = new_exec(&body, stdin, signals);
        let readers = [
            pipe(exec.clone(), false, child.stdout.take().unwrap()),
            pipe(exec.clone(), true, child.stderr.take().unwrap()),
        ];
        let ended = exec.clone();
        tokio::spawn(async move {
            let exit = tokio::select! {
                status = child.wait() => json!({"reason": "exited", "code": status.unwrap().code()}),
                Some(signal) = signalled.recv() => {
                    child.kill().await.unwrap();
                    json!({"reason": "signaled", "signal": signal})
                }
            };
            for reader in readers {
                reader.await.unwrap();
            }
            ended.end(exit);
        });
        exec
    };
    let mut boxes = state.boxes.lock().unwrap();
    let b = boxes.get_mut(&id).unwrap();
    let exec_id = format!("exec_{}", b.execs.len() + 1);
    b.execs.insert(exec_id.clone(), exec);
    Json(json!({"id": exec_id})).into_response()
}

fn new_exec(
    body: &CreateExec,
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    signals: mpsc::UnboundedSender<i32>,
) -> Arc<Exec> {
    Arc::new(Exec {
        argv: body.argv.clone(),
        out: Mutex::default(),
        changed: Notify::new(),
        stdin: AsyncMutex::new((0, stdin)),
        signals,
    })
}

fn pipe(
    exec: Arc<Exec>,
    stderr: bool,
    mut from: impl AsyncRead + Unpin + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = vec![0; 4096];
        while let Ok(n @ 1..) = from.read(&mut buf).await {
            exec.append(stderr, &buf[..n]);
        }
    })
}

fn find(state: &Shared, id: &str, exec: &str) -> Result<Arc<Exec>, Refused> {
    let boxes = state.boxes.lock().unwrap();
    let b = boxes.get(id).ok_or_else(refused)?;
    b.execs.get(exec).cloned().ok_or_else(refused)
}

#[derive(Deserialize)]
struct OutputQuery {
    stdout_offset: u64,
    stderr_offset: u64,
}

async fn output(
    State(state): State<Arc<Shared>>,
    UrlPath((id, exec_id)): UrlPath<(String, String)>,
    Query(query): Query<OutputQuery>,
) -> Response {
    let exec = match find(&state, &id, &exec_id) {
        Ok(exec) => exec,
        Err(refused) => return *refused,
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let changed = exec.changed.notified();
        {
            let out = exec.out.lock().unwrap();
            let mut frames = frames_of("stdout", &out.stdout, query.stdout_offset);
            frames.extend(frames_of("stderr", &out.stderr, query.stderr_offset));
            if !frames.is_empty() || out.exit.is_some() {
                return Json(json!({"frames": frames, "exit": out.exit})).into_response();
            }
        }
        if tokio::time::timeout_at(deadline, changed).await.is_err() {
            return Json(json!({"frames": []})).into_response();
        }
    }
}

/// Everything from `offset` on, starting one byte early when there is new data past it.
fn frames_of(stream: &str, data: &[u8], offset: u64) -> Vec<Value> {
    let offset = offset as usize;
    if data.len() <= offset {
        return Vec::new();
    }
    let from = offset.saturating_sub(1);
    data[from..]
        .chunks(FRAME)
        .enumerate()
        .map(|(i, chunk)| {
            json!({
                "stream": stream,
                "offset": from + i * FRAME,
                "data_b64": STANDARD.encode(chunk),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct Input {
    seq: u64,
    data_b64: Option<String>,
    #[serde(default)]
    eof: bool,
    signal: Option<i32>,
}

async fn input(
    State(state): State<Arc<Shared>>,
    UrlPath((id, exec_id)): UrlPath<(String, String)>,
    Json(body): Json<Input>,
) -> Response {
    let exec = match find(&state, &id, &exec_id) {
        Ok(exec) => exec,
        Err(refused) => return *refused,
    };
    let delay = *state.input_delay.lock().unwrap();
    tokio::time::sleep(delay).await;
    let mut stdin = exec.stdin.lock().await;
    if stdin.0.checked_sub(1) == Some(body.seq) {
        return Json(json!({"next_seq": stdin.0})).into_response();
    }
    if body.seq != stdin.0 {
        let error = json!({"error": {
            "type": "conflict",
            "message": "Input 'seq' is out of order.",
            "next_seq": stdin.0,
        }});
        return (StatusCode::CONFLICT, Json(error)).into_response();
    }
    if let Some(data) = body.data_b64 {
        let Some(pipe) = stdin.1.as_mut() else {
            return api_error(StatusCode::CONFLICT, "conflict", "Stdin is closed.");
        };
        let data = STANDARD.decode(data).unwrap();
        if pipe.write_all(&data).await.is_err() {
            return api_error(StatusCode::CONFLICT, "conflict", "Stdin is closed.");
        }
    }
    if body.eof && stdin.1.take().is_none() {
        return api_error(StatusCode::CONFLICT, "conflict", "Stdin is closed.");
    }
    if let Some(signal) = body.signal {
        if exec.out.lock().unwrap().exit.is_some() {
            return api_error(StatusCode::CONFLICT, "conflict", "The command has ended.");
        }
        let _ = exec.signals.send(signal);
    }
    stdin.0 += 1;
    Json(json!({"next_seq": stdin.0})).into_response()
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    offset: u64,
}

/// Holds a file call for the set delay, counting it as in flight meanwhile.
async fn file_call(state: &Shared) {
    let delay = *state.file_delay.lock().unwrap();
    let now = state.file_calls.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak_file_calls.fetch_max(now, Ordering::SeqCst);
    tokio::time::sleep(delay).await;
    state.file_calls.fetch_sub(1, Ordering::SeqCst);
}

fn host_path(state: &Shared, id: &str, path: &str) -> Result<PathBuf, Refused> {
    Ok(PathBuf::from(reroot(&live_root(state, id)?, path)))
}

async fn stat(
    State(state): State<Arc<Shared>>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<FileQuery>,
) -> Response {
    let host = match host_path(&state, &id, &query.path) {
        Ok(host) => host,
        Err(refused) => return *refused,
    };
    match fs::symlink_metadata(&host) {
        Ok(meta) => Json(stat_json(&query.path, &meta)).into_response(),
        Err(_) => not_found(),
    }
}

fn stat_json(path: &str, meta: &fs::Metadata) -> Value {
    use std::os::unix::fs::PermissionsExt;
    let kind = match meta.file_type() {
        t if t.is_symlink() => "symlink",
        t if t.is_dir() => "directory",
        t if t.is_file() => "file",
        _ => "fifo",
    };
    json!({
        "path": path,
        "type": kind,
        "size": meta.len(),
        "mode": meta.permissions().mode() & 0o7777,
        "mtime": "2026-03-01T00:00:00Z",
    })
}

async fn read_file(
    State(state): State<Arc<Shared>>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response {
    let host = match host_path(&state, &id, &query.path) {
        Ok(host) => host,
        Err(refused) => return *refused,
    };
    file_call(&state).await;
    let Ok(data) = fs::read(&host) else {
        return not_found();
    };
    let total = data.len() as u64;
    if total == 0 {
        return StatusCode::OK.into_response();
    }
    let range = headers[RANGE].to_str().unwrap();
    let (first, last) = range
        .strip_prefix("bytes=")
        .and_then(|r| r.split_once('-'))
        .map(|(a, b)| (a.parse::<u64>().unwrap(), b.parse::<u64>().ok()))
        .unwrap();
    if first >= total {
        let header = [(CONTENT_RANGE, format!("bytes */{total}"))];
        return (StatusCode::RANGE_NOT_SATISFIABLE, header).into_response();
    }
    let end = last
        .map_or(total, |l| l + 1)
        .min(total)
        .min(first + RANGE_CAP)
        .min(match *state.short_ranges.lock().unwrap() {
            Some(cap) if first > 0 => first + cap,
            _ => u64::MAX,
        });
    let header = [(CONTENT_RANGE, format!("bytes {first}-{}/{total}", end - 1))];
    let body = data[first as usize..end as usize].to_vec();
    (StatusCode::PARTIAL_CONTENT, header, body).into_response()
}

async fn write_file(
    State(state): State<Arc<Shared>>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<FileQuery>,
    body: Bytes,
) -> Response {
    let host = match host_path(&state, &id, &query.path) {
        Ok(host) => host,
        Err(refused) => return *refused,
    };
    state.writes.lock().unwrap().push(query.offset);
    state.write_started.notify_waiters();
    let hold = *state.hold.lock().unwrap();
    if let Some((_, until)) = hold.filter(|(at, _)| *at == query.offset) {
        let passed = || state.writes.lock().unwrap().contains(&until);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let next = state.write_started.notified();
            if passed() || tokio::time::timeout_at(deadline, next).await.is_err() {
                break;
            }
        }
        *state.passed_held.lock().unwrap() = Some(passed());
    }
    file_call(&state).await;
    fs::create_dir_all(host.parent().unwrap()).unwrap();
    // As the box does: offset 0 replaces the file, a later one writes in place.
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(query.offset == 0)
        .open(&host)
        .unwrap();
    file.seek(SeekFrom::Start(query.offset)).unwrap();
    file.write_all(&body).unwrap();
    Json(stat_json(&query.path, &fs::metadata(&host).unwrap())).into_response()
}

async fn remove_file(
    State(state): State<Arc<Shared>>,
    UrlPath(id): UrlPath<String>,
    Query(query): Query<FileQuery>,
) -> Response {
    let host = match host_path(&state, &id, &query.path) {
        Ok(host) => host,
        Err(refused) => return *refused,
    };
    match fs::remove_file(host) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => not_found(),
    }
}
