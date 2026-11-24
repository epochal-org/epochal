use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::Duration;

use bytes::Bytes;
use epochal_api::time_portal::TimePortal;
use epochal_api::time_portal::sandbox::{
    ExecExit, ExecFrame, ExecInput, ExecOutputRequest, ExecStartRequest, Stream, idempotency_key,
};
use epochal_api::{Error as ApiError, MAX_WAIT};
use tokio::sync::Mutex;

use crate::Error;
use crate::retry::{retry, transient};
use crate::sandbox::Sandbox;

/// The most one input call carries.
pub(crate) const PIPE: usize = 1 << 20;
const SIGKILL: i32 = 9;

/// A command to run in a box, from [`Sandbox::command`]: options, then [`start`](Self::start) to
/// follow its output or [`output`](Self::output) to collect it.
#[derive(Debug)]
#[must_use]
pub struct Command<'a> {
    sandbox: &'a Sandbox,
    request: ExecStartRequest,
}

impl Command<'_> {
    /// The working directory; the image's `HOME` when unset.
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.request.cwd = Some(cwd.into());
        self
    }

    /// A variable added to the box's environment for this command. Values are not logged.
    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.request.env.insert(name.into(), value.into());
        self
    }

    /// Kills the command after this long, in whole seconds; 1 h when unset.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.request.timeout = Some(timeout);
        self
    }

    /// Bytes on the command's stdin, which is then closed unless
    /// [`with_open_stdin`](Self::with_open_stdin).
    pub fn with_stdin(mut self, stdin: impl Into<Vec<u8>>) -> Self {
        self.request.stdin = stdin.into();
        self
    }

    /// The start's `Idempotency-Key`, so that a start retried by the caller runs the command once.
    /// Unset, each start uses a fresh key of its own, which covers the retries it makes itself.
    pub fn with_idempotency_key(mut self, key: impl Into<String>) -> Self {
        self.request.idempotency_key = Some(key.into());
        self
    }

    /// Keeps stdin open for [`Exec::write_stdin`] until [`Exec::close_stdin`].
    pub fn with_open_stdin(mut self) -> Self {
        self.request.stdin_open = true;
        self
    }

    /// Starts the command.
    pub async fn start(mut self) -> Result<Exec, Error> {
        let sandbox = self.sandbox;
        let request = &mut self.request;
        request.idempotency_key.get_or_insert_with(idempotency_key);
        let id = retry(|| sandbox.portal.start_exec(sandbox.id(), request)).await?;
        Ok(Exec {
            portal: sandbox.portal.clone(),
            sandbox: sandbox.id().to_owned(),
            id,
            reader: Mutex::default(),
            stdin: Mutex::default(),
            exit: OnceLock::new(),
        })
    }

    /// Runs the command to its end.
    pub async fn output(self) -> Result<Output, Error> {
        self.start().await?.output().await
    }
}

/// A finished command's output.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Output {
    /// All of its stdout.
    pub stdout: Vec<u8>,
    /// All of its stderr.
    pub stderr: Vec<u8>,
    /// How it ended.
    pub exit: ExecExit,
}

impl Output {
    /// Whether it exited with status 0.
    pub fn success(&self) -> bool {
        self.exit == ExecExit::Exited { code: 0 }
    }
}

/// A started command, from [`Command::start`]. Output is read by offset, so each poll resumes
/// where the last stopped.
#[derive(Debug)]
pub struct Exec {
    portal: TimePortal,
    sandbox: String,
    id: String,
    reader: Mutex<Reader>,
    stdin: Mutex<Stdin>,
    exit: OnceLock<ExecExit>,
}

/// Where the command's stdin stands: the next `seq`, the input sent under it with no answer seen,
/// if any, and the rest of a write not yet sent.
#[derive(Debug, Default)]
struct Stdin {
    next_seq: u64,
    in_flight: Option<ExecInput>,
    unsent: Bytes,
}

#[derive(Debug, Default)]
struct Reader {
    stdout: u64,
    stderr: u64,
    pending: VecDeque<ExecFrame>,
    ended: Option<ExecExit>,
}

impl Sandbox {
    /// A command: the program and its arguments, run without a shell.
    pub fn command<I, S>(&self, argv: I) -> Command<'_>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Command {
            sandbox: self,
            request: ExecStartRequest::new(argv),
        }
    }
}

impl Exec {
    /// The command's id within its box.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// How the command ended, once [`next`](Self::next) has returned `None`.
    pub fn exit(&self) -> Option<ExecExit> {
        self.exit.get().copied()
    }

    /// The next piece of output, in the order the box saw it; `None` once the command has ended
    /// and all of its output was returned. Bytes already returned are never returned again.
    pub async fn next(&self) -> Result<Option<ExecFrame>, Error> {
        let mut reader = self.reader.lock().await;
        loop {
            if let Some(frame) = reader.pending.pop_front() {
                return Ok(Some(frame));
            }
            if let Some(ended) = reader.ended {
                let _ = self.exit.set(ended);
                return Ok(None);
            }
            self.poll(&mut reader).await?;
        }
    }

    async fn poll(&self, reader: &mut Reader) -> Result<(), Error> {
        let mut request = ExecOutputRequest::default();
        request.stdout_offset = reader.stdout;
        request.stderr_offset = reader.stderr;
        request.wait = Some(MAX_WAIT);
        let output = retry(|| {
            self.portal
                .read_exec_output(&self.sandbox, &self.id, &request)
        })
        .await?;
        for mut frame in output.frames {
            let at = match frame.stream {
                Stream::Stdout => &mut reader.stdout,
                Stream::Stderr => &mut reader.stderr,
                _ => continue,
            };
            let end = frame.offset + frame.data.len() as u64;
            if end <= *at {
                continue;
            }
            // A repeat of bytes already read is cut; a gap means the box kept no copy of them.
            let repeated = at.saturating_sub(frame.offset);
            *at = end;
            frame.data.drain(..repeated as usize);
            frame.offset += repeated;
            reader.pending.push_back(frame);
        }
        reader.ended = output.exit;
        Ok(())
    }

    /// The rest of the output, and how the command ended.
    pub async fn output(&self) -> Result<Output, Error> {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        while let Some(frame) = self.next().await? {
            match frame.stream {
                Stream::Stdout => stdout.extend(frame.data),
                _ => stderr.extend(frame.data),
            }
        }
        Ok(Output {
            stdout,
            stderr,
            exit: self.exit().unwrap_or(ExecExit::Unknown),
        })
    }

    /// Writes `data` to stdin, in calls of at most 1 MiB; needs
    /// [`with_open_stdin`](Command::with_open_stdin). If it fails with an error that may pass, or
    /// is cancelled once it has started sending, what it had not yet written is sent, once and in
    /// order, ahead of the next input, so don't write `data` again yourself. If the API refuses a
    /// piece, the rest is dropped. Cancelled while another input on this command is still being
    /// sent, it writes none of `data`.
    pub async fn write_stdin(&self, data: &[u8]) -> Result<(), Error> {
        let mut stdin = self.stdin.lock().await;
        stdin.unsent = [&stdin.unsent[..], data].concat().into();
        self.flush(&mut stdin).await
    }

    /// Closes stdin, for a command started [`with_open_stdin`](Command::with_open_stdin).
    pub async fn close_stdin(&self) -> Result<(), Error> {
        self.input(ExecInput::Eof).await
    }

    /// Sends the signal numbered `signal`, such as 15 for `SIGTERM`, to the command.
    pub async fn signal(&self, signal: i32) -> Result<(), Error> {
        self.input(ExecInput::Signal(signal)).await
    }

    /// Kills the command and waits until it has ended, discarding output not yet read; one
    /// already over counts as killed.
    pub async fn kill(&self) -> Result<(), Error> {
        if self.exit().is_none() {
            // A refused stdin piece ahead of it is dropped with the rest of its write, so the
            // second try sends only the signal; only its own refusal means the command is over.
            let killed = match self.signal(SIGKILL).await {
                Err(e) if gone(&e) => self.signal(SIGKILL).await,
                sent => sent,
            };
            match killed {
                Err(e) if !gone(&e) => return Err(e),
                _ => {}
            }
        }
        loop {
            match self.next().await {
                Ok(Some(_)) => {}
                Ok(None) => return Ok(()),
                Err(e) if gone(&e) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }

    /// Applies `input` once, after anything an earlier call left unsent.
    async fn input(&self, input: ExecInput) -> Result<(), Error> {
        let mut stdin = self.stdin.lock().await;
        self.flush(&mut stdin).await?;
        self.send(&mut stdin, input).await
    }

    /// Sends what earlier calls left: first an input sent with no answer seen, then the rest of a
    /// write, a piece at a time. Each input has a `seq`, which the API applies at most once, so a
    /// retry resends it under the same one. An input whose outcome is unknown, because it failed,
    /// was answered unreadably or was cancelled, may still have been applied: a new input under the
    /// next `seq` would be taken for its retry and dropped, so it is resent first.
    async fn flush(&self, stdin: &mut Stdin) -> Result<(), Error> {
        if let Some(earlier) = stdin.in_flight.clone() {
            self.send(stdin, earlier).await?;
        }
        while !stdin.unsent.is_empty() {
            let piece = stdin.unsent.split_to(stdin.unsent.len().min(PIPE));
            self.send(stdin, ExecInput::Data(piece.to_vec())).await?;
        }
        Ok(())
    }

    /// Sends `input` under the next `seq`. One the API refused was not applied: it is forgotten,
    /// with the rest of its write.
    async fn send(&self, stdin: &mut Stdin, input: ExecInput) -> Result<(), Error> {
        let seq = stdin.next_seq;
        stdin.in_flight = Some(input);
        let input = stdin.in_flight.as_ref().expect("just set");
        let next = retry(|| {
            self.portal
                .send_exec_input(&self.sandbox, &self.id, seq, input)
        })
        .await
        .inspect_err(|e| {
            if refused(e) {
                stdin.in_flight = None;
                stdin.unsent.clear();
            }
        })?;
        stdin.next_seq = next;
        stdin.in_flight = None;
        Ok(())
    }
}

/// The command, or its box, is already over.
fn gone(e: &Error) -> bool {
    matches!(e, Error::Api(ApiError::Api { kind, .. }) if kind == "conflict" || kind == "not_found")
}

/// Whether the API refused the call, so it was not applied; any other failure leaves it unknown.
fn refused(e: &ApiError) -> bool {
    match e {
        ApiError::Api { .. } => transient(e).is_none(),
        ApiError::InvalidArgument(_) | ApiError::MissingApiKey => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::post;
    use axum::{Json, Router};
    use epochal_api::Client;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use super::*;

    /// A command's stdin as the API keeps it: the next `seq`, each input applied, how many more
    /// acks to lose, or garble, after applying, and whether to hold every answer after applying.
    #[derive(Default)]
    struct Fake {
        next: u64,
        applied: Vec<Value>,
        lose: u32,
        garble: u32,
        stall: bool,
    }

    type Shared = Arc<StdMutex<Fake>>;

    /// The API's rule: `next` applies, `next - 1` is acked again unapplied, anything else is out of
    /// order. Then the answer is held, or lost, as the fake is set to.
    async fn input(State(shared): State<Shared>, Json(body): Json<Value>) -> Response {
        let (stall, answer) = {
            let mut stdin = shared.lock().unwrap();
            let seq = body["seq"].as_u64().unwrap();
            if body["signal"] == 0 {
                let error =
                    json!({"error": {"type": "invalid_request", "message": "No such signal."}});
                return (StatusCode::BAD_REQUEST, Json(error)).into_response();
            }
            if seq == stdin.next {
                stdin.next += 1;
                stdin.applied.push(body);
            } else if seq + 1 != stdin.next {
                let error = json!({"error": {"type": "conflict", "message": "Out of order.", "next_seq": stdin.next}});
                return (StatusCode::CONFLICT, Json(error)).into_response();
            }
            let answer = if stdin.lose > 0 {
                stdin.lose -= 1;
                StatusCode::BAD_GATEWAY.into_response()
            } else if stdin.garble > 0 {
                stdin.garble -= 1;
                (StatusCode::OK, "{").into_response()
            } else {
                Json(json!({"next_seq": stdin.next})).into_response()
            };
            (stdin.stall, answer)
        };
        if stall {
            std::future::pending::<()>().await;
        }
        answer
    }

    async fn started(stdin: Shared) -> Exec {
        let box_json = json!({"id": "sbx_1", "state": "live", "as_of": "2025-03-01T00:00:00Z",
            "network": "none", "vcpus": 1, "memory_gib": 1, "timeout": "1h",
            "idle_timeout": "10m", "created_at": "2026-11-09T10:00:00Z"});
        let app = Router::new()
            .route(
                "/v1/time-portal/sandbox",
                post(move || async move { Json(box_json) }),
            )
            .route(
                "/v1/time-portal/sandbox/sbx_1/execs",
                post(|| async { Json(json!({"id": "exec_1"})) }),
            )
            .route(
                "/v1/time-portal/sandbox/sbx_1/execs/exec_1/input",
                post(input),
            )
            .with_state(stdin);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::builder()
            .with_api_key("k")
            .with_base_url(url)
            .build()
            .unwrap();
        let sb = Sandbox::time_portal("2025-03-01T00:00:00Z".parse().unwrap())
            .with_client(client)
            .build()
            .await
            .unwrap();
        sb.command(["relay"]).start().await.unwrap()
    }

    #[tokio::test]
    async fn an_input_whose_ack_is_lost_is_applied_once_and_the_next_follows_it() {
        let stdin = Shared::new(StdMutex::new(Fake {
            lose: 1,
            ..Fake::default()
        }));
        let exec = started(stdin.clone()).await;

        exec.signal(9).await.unwrap();
        exec.write_stdin(b"after").await.unwrap();

        let stdin = stdin.lock().unwrap();
        let seqs: Vec<_> = stdin
            .applied
            .iter()
            .map(|b| b["seq"].as_u64().unwrap())
            .collect();
        assert_eq!(seqs, [0, 1]);
        assert_eq!(stdin.applied[0]["signal"], 9);
        assert_eq!(stdin.applied[1]["data_b64"], "YWZ0ZXI=");
    }

    #[tokio::test]
    async fn an_input_cancelled_after_it_landed_is_resent_before_the_next_and_applied_once() {
        let stdin = Shared::new(StdMutex::new(Fake {
            stall: true,
            ..Fake::default()
        }));
        let exec = started(stdin.clone()).await;

        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            exec.write_stdin(b"landed"),
        )
        .await;
        assert!(cancelled.is_err(), "the answer never came");
        stdin.lock().unwrap().stall = false;
        exec.signal(9).await.unwrap();

        let stdin = stdin.lock().unwrap();
        let applied: Vec<_> = stdin
            .applied
            .iter()
            .map(|b| (b["seq"].clone(), b["signal"].clone()))
            .collect();
        assert_eq!(applied, [(json!(0), Value::Null), (json!(1), json!(9))]);
    }

    #[tokio::test]
    async fn an_input_applied_but_answered_unreadably_is_resent_before_the_next() {
        let stdin = Shared::new(StdMutex::new(Fake {
            garble: 1,
            ..Fake::default()
        }));
        let exec = started(stdin.clone()).await;

        let unreadable = exec.write_stdin(b"one ").await;
        assert!(
            matches!(unreadable, Err(Error::Api(ApiError::Decode(_)))),
            "{unreadable:?}"
        );
        exec.write_stdin(b"two").await.unwrap();

        let stdin = stdin.lock().unwrap();
        let applied: Vec<_> = stdin
            .applied
            .iter()
            .map(|b| (b["seq"].clone(), b["data_b64"].clone()))
            .collect();
        assert_eq!(
            applied,
            [(json!(0), json!("b25lIA==")), (json!(1), json!("dHdv"))]
        );
    }

    #[tokio::test]
    async fn a_refused_input_is_forgotten_and_the_next_one_still_lands() {
        let stdin = Shared::default();
        let exec = started(stdin.clone()).await;

        assert!(exec.signal(0).await.is_err());
        exec.signal(9).await.unwrap();

        let stdin = stdin.lock().unwrap();
        let applied: Vec<_> = stdin
            .applied
            .iter()
            .map(|b| (b["seq"].clone(), b["signal"].clone()))
            .collect();
        assert_eq!(applied, [(json!(0), json!(9))]);
    }
}
