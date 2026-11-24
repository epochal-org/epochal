//! `…/sandbox/{id}/execs`: commands in a box, their output, and their stdin. Following output
//! to the end, and resyncing a lost input, is the caller's.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::Not;
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use super::{idempotency_key, sandbox_path};
use crate::client::{with_idempotency_key, with_json};
use crate::error::Error;
use crate::time_portal::TimePortal;
use crate::wire::{bytes_b64, duration_param, segment, serialize_duration};

/// `POST …/execs`: a command to start, run without a shell.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct ExecStartRequest {
    /// The program and its arguments.
    pub argv: Vec<String>,
    /// Variables added to the box's environment for this command. Values are not logged.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The working directory; the image's `HOME` when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Kills the command after this long; 1 h when unset. A positive whole number of seconds.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_duration"
    )]
    pub timeout: Option<Duration>,
    /// Bytes written to stdin at start.
    #[serde(
        rename = "stdin_b64",
        skip_serializing_if = "Vec::is_empty",
        with = "bytes_b64"
    )]
    pub stdin: Vec<u8>,
    /// Keeps stdin open for [`TimePortal::send_exec_input`]; otherwise it is closed after `stdin`.
    #[serde(skip_serializing_if = "Not::not")]
    pub stdin_open: bool,
    /// Sent as `Idempotency-Key`: a start retried with the same key runs the command once.
    /// Unset, each call sends a fresh key, so only a retry that sets the first call's key is
    /// covered.
    #[serde(skip)]
    pub idempotency_key: Option<String>,
}

impl ExecStartRequest {
    /// `argv` with every other field unset.
    pub fn new<I, S>(argv: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        ExecStartRequest {
            argv: argv.into_iter().map(Into::into).collect(),
            env: BTreeMap::new(),
            cwd: None,
            timeout: None,
            stdin: Vec::new(),
            stdin_open: false,
            idempotency_key: None,
        }
    }
}

impl fmt::Debug for ExecStartRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecStartRequest")
            .field("argv", &self.argv)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .field("stdin", &self.stdin.len())
            .field("stdin_open", &self.stdin_open)
            .field("idempotency_key", &self.idempotency_key)
            .finish()
    }
}

#[derive(Deserialize)]
struct Started {
    id: String,
}

/// `GET …/execs/{exec}/output`: what to read, from `ExecOutputRequest::default()` then its
/// fields. Defaults read from the start of both streams without waiting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExecOutputRequest {
    /// Where to start in stdout.
    pub stdout_offset: u64,
    /// Where to start in stderr.
    pub stderr_offset: u64,
    /// Hold the call until there is new output, the command ends, or this passes; at most
    /// [`MAX_WAIT`](crate::MAX_WAIT).
    pub wait: Option<Duration>,
    /// The most output bytes in the answer.
    pub max_bytes: Option<u64>,
}

/// Which stream output came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
    /// A stream from a newer server.
    #[serde(other)]
    Unknown,
}

/// How a command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExecExit {
    /// It exited on its own.
    Exited {
        /// Its exit status.
        code: i32,
    },
    /// A signal killed it.
    Signaled {
        /// The signal number.
        signal: i32,
    },
    /// It ran out of the box's memory.
    OutOfMemory,
    /// Its timeout passed.
    Timeout,
    /// The box ended under it.
    SandboxEnded,
    /// The box stopped reporting on it.
    Lost,
    /// A reason from a newer server.
    #[serde(other)]
    Unknown,
}

/// The output after the offsets asked for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExecOutput {
    /// Output in the order the box saw it. A frame may repeat bytes before the offset asked
    /// for, and a gap means the box kept no copy of the bytes in it.
    pub frames: Vec<ExecFrame>,
    /// How the command ended, once it has and every frame up to its end was returned.
    #[serde(default)]
    pub exit: Option<ExecExit>,
}

/// A piece of a command's output.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExecFrame {
    /// Which stream it came from.
    pub stream: Stream,
    /// Its position in that stream.
    pub offset: u64,
    /// The bytes.
    #[serde(rename = "data_b64", with = "bytes_b64")]
    pub data: Vec<u8>,
}

impl fmt::Debug for ExecFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecFrame")
            .field("stream", &self.stream)
            .field("offset", &self.offset)
            .field("len", &self.data.len())
            .finish()
    }
}

/// One action on a running command, for [`TimePortal::send_exec_input`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExecInput {
    /// Bytes for stdin.
    Data(Vec<u8>),
    /// Close stdin.
    Eof,
    /// Send this signal number.
    Signal(i32),
}

#[derive(Serialize)]
struct InputBody {
    seq: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_b64: Option<String>,
    #[serde(skip_serializing_if = "Not::not")]
    eof: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    signal: Option<i32>,
}

#[derive(Deserialize)]
struct Ack {
    next_seq: u64,
}

impl TimePortal {
    /// Starts a command and returns its id.
    pub async fn start_exec(&self, id: &str, request: &ExecStartRequest) -> Result<String, Error> {
        if request.argv.first().is_none_or(String::is_empty) {
            return Err(Error::InvalidArgument("argv must name a program".into()));
        }
        let key = request
            .idempotency_key
            .clone()
            .unwrap_or_else(idempotency_key);
        let http = self.request(Method::POST, &sandbox_path(id, "/execs")?);
        let http = with_idempotency_key(http, &key);
        let started: Started = self.client.json(with_json(http, request)?).await?;
        Ok(started.id)
    }

    /// The output of `exec` after the offsets in `request`.
    pub async fn read_exec_output(
        &self,
        id: &str,
        exec: &str,
        request: &ExecOutputRequest,
    ) -> Result<ExecOutput, Error> {
        let mut query = vec![
            ("stdout_offset", request.stdout_offset.to_string()),
            ("stderr_offset", request.stderr_offset.to_string()),
        ];
        if let Some(wait) = request.wait {
            query.push(("wait", duration_param(wait)?));
        }
        if let Some(max_bytes) = request.max_bytes {
            query.push(("max_bytes", max_bytes.to_string()));
        }
        let path = exec_path(id, exec, "/output")?;
        self.client
            .json(self.request(Method::GET, &path).query(&query))
            .await
    }

    /// Applies `input` as call number `seq` (from 0) and returns the next `seq`. A call the API
    /// already applied is not applied again; a `seq` out of order is refused with
    /// [`Error::Api`] carrying the `next_seq` the command expects.
    pub async fn send_exec_input(
        &self,
        id: &str,
        exec: &str,
        seq: u64,
        input: &ExecInput,
    ) -> Result<u64, Error> {
        let mut body = InputBody {
            seq,
            data_b64: None,
            eof: false,
            signal: None,
        };
        match input {
            ExecInput::Data(data) => body.data_b64 = Some(bytes_b64::encode(data)),
            ExecInput::Eof => body.eof = true,
            ExecInput::Signal(signal) => body.signal = Some(*signal),
        }
        let http = self.request(Method::POST, &exec_path(id, exec, "/input")?);
        let ack: Ack = self.client.json(with_json(http, &body)?).await?;
        Ok(ack.next_seq)
    }
}

fn exec_path(id: &str, exec: &str, rest: &str) -> Result<String, Error> {
    sandbox_path(id, &format!("/execs/{}{rest}", segment(exec)?))
}
