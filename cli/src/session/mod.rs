//! A session: makes the box and drives an agent in it to the end. The built-in harness runs on this
//! machine with the box as its computer. A third-party harness is installed in the box as an app,
//! with the relay, and its model and web tool calls are served from this machine through the relay.
//! What happens is reported to a [`Watch`].

mod harness;
mod proxy;
mod scrub;

use std::collections::VecDeque;
use std::mem;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use epochal_api::chrono::{DateTime, Utc};
use epochal_api::{Client, Error as ApiError};
use epochal_mcp::{ToolCall, Tools};
use epochal_sandbox::{
    Error as SandboxError, ExecExit, FileType, Network, Sandbox, Stream, Tunnel,
};

pub use harness::Harness;
use harness::Task;
use proxy::Endpoint;
pub use proxy::Upstream;

use crate::lines::Lines;
use crate::output::instant;
use crate::output::progress::{self, Kind, Out};
use crate::output::setup::Setup;
use crate::output::trace::Shared;
use crate::sampler::{self, Llm};

/// The most lines of the harness's stderr kept to show when it fails.
const STDERR_TAIL: usize = 20;
const NOT_FOUND: u16 = 404;
const INTERRUPTED: i32 = 130;
/// The most of the box's life kept back from the harness for the downloads and cleanup after it.
const WIND_DOWN: Duration = Duration::from_secs(5 * 60);

/// How the box is made.
pub struct BoxSettings {
    pub client: Client,
    pub small_model: Option<String>,
    pub network: Network,
    pub uploads: Vec<(PathBuf, String)>,
    pub downloads: Vec<(String, PathBuf)>,
    pub cwd: String,
    pub vcpus: u32,
    pub memory_gib: u32,
    pub timeout: Duration,
    pub max_turns: Option<u32>,
    pub keep: bool,
}

/// What drives the run, and where its model calls go.
pub enum Agent {
    Builtin(Llm),
    /// Installed in the box, its model calls sent upstream through the relay.
    App(Harness, Upstream),
}

impl Agent {
    pub fn name(&self) -> &'static str {
        match self {
            Agent::Builtin(_) => "builtin",
            Agent::App(harness, _) => harness.app(),
        }
    }

    /// Whose events the agent writes: the built-in harness writes Claude Code's.
    pub fn dialect(&self) -> Harness {
        match self {
            Agent::Builtin(_) => Harness::Claude,
            Agent::App(harness, _) => *harness,
        }
    }

    fn title(&self) -> &'static str {
        match self {
            Agent::Builtin(_) => "Built-in harness",
            Agent::App(harness, _) => harness.name(),
        }
    }
}

/// One run: the agent and model, what they're asked, and the instant the box is held to.
pub struct Job {
    pub agent: Agent,
    pub model: String,
    pub prompt: String,
    pub as_of: DateTime<Utc>,
}

/// Sees a session as it happens: its progress, and each harness event as the harness sent it.
pub trait Watch: Out {
    fn event(&mut self, raw: &[u8]);

    /// Where each web call is written as it is made, if anywhere.
    fn trace(&self) -> Option<Shared>;
}

impl<W: Watch> sampler::Listener for W {
    fn event(&mut self, raw: Vec<u8>) {
        Watch::event(self, &raw);
    }

    fn thinking(&mut self) {
        self.status("Thinking", "");
    }
}

/// What a session left: the web tools' calls as the API answered them, and the exit code as a
/// shell reports one, or why the downloads after it failed.
pub struct Outcome {
    pub sandbox: String,
    pub as_of: DateTime<Utc>,
    pub calls: Vec<ToolCall>,
    pub code: Result<i32>,
}

/// Every web call so far, each also written to the trace, if any, the moment it is made: a
/// session that fails midway still leaves its calls in the trace.
#[derive(Clone)]
struct Log {
    calls: Arc<Mutex<Vec<ToolCall>>>,
    trace: Option<Shared>,
}

impl Log {
    fn push(&self, call: ToolCall) {
        if let Some(trace) = &self.trace {
            trace
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .call(&call);
        }
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }

    fn take(&self) -> Vec<ToolCall> {
        mem::take(&mut *self.calls.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// Makes the box, drives the harness in it to the end, and removes the box unless `keep`.
pub async fn session(cfg: &BoxSettings, job: &Job, out: &mut impl Watch) -> Result<Outcome> {
    let mut setup = Setup::new();
    setup.now(out, &format!("Creating sandbox at {}", instant(job.as_of)));
    let t0 = Instant::now();
    let mut builder = Sandbox::time_portal(job.as_of)
        .with_client(cfg.client.clone())
        .with_network(cfg.network)
        .with_vcpus(cfg.vcpus)
        .with_memory_gib(cfg.memory_gib)
        .with_timeout(cfg.timeout);
    builder = match &job.agent {
        Agent::App(harness, _) => builder.with_app(harness.app()).with_relay(),
        // No command runs while the model thinks, which must not count as idle.
        Agent::Builtin(_) => builder.with_idle_timeout(cfg.timeout),
    };
    // Watched from the start, by one listener, so no signal goes uncounted.
    let mut stops = Stops::new();
    let mut build = pin!(builder.build());
    let (made, stopped) = tokio::select! {
        made = &mut build => (made, false),
        () = stops.next() => {
            out.status("Stopping once the sandbox is made, to remove it", "");
            tokio::select! {
                made = build => (made, true),
                () = stops.next() => abandon(out, ""),
            }
        }
    };
    let sb = match made {
        Ok(sb) => sb,
        Err(e) => {
            setup.failed(out);
            return Err(e.into());
        }
    };
    setup.created(sb.id());
    let made = format!("· as of {}", instant(sb.as_of()));
    setup.step(format!("Created {}", sb.id()), done(&made, t0));
    for app in sb.apps() {
        setup.step(format!("Installed {app}"), "");
    }
    let (result, stopped) = if stopped {
        (Err(anyhow!("interrupted")), true)
    } else {
        tokio::select! {
            result = drive(&sb, cfg, job, out, setup, t0) => (result, false),
            () = stops.next() => (Err(anyhow!("interrupted")), true),
        }
    };
    if cfg.keep {
        out.line(Kind::Step, "Kept the sandbox", sb.id());
        return result;
    }
    out.status("Removing sandbox", "");
    // The signal after the one that stopped the run, if one did, abandons the box.
    let again = async {
        if !stopped {
            stops.next().await;
        }
        stops.next().await;
    };
    tokio::select! {
        ended = sb.terminate() => if let Err(e) = ended {
            tracing::warn!(%e, "terminating the box");
        },
        () = again => abandon(out, sb.id()),
    }
    result
}

/// SIGINT, SIGTERM and SIGHUP, each counted once from when this is made.
struct Stops {
    #[cfg(unix)]
    kinds: Vec<tokio::signal::unix::Signal>,
}

impl Stops {
    #[cfg(unix)]
    fn new() -> Stops {
        use tokio::signal::unix::{SignalKind, signal};
        let kinds = [
            SignalKind::interrupt(),
            SignalKind::terminate(),
            SignalKind::hangup(),
        ];
        Stops {
            kinds: kinds.into_iter().filter_map(|k| signal(k).ok()).collect(),
        }
    }

    #[cfg(not(unix))]
    fn new() -> Stops {
        Stops {}
    }

    /// The next signal; never if none can be watched.
    #[cfg(unix)]
    async fn next(&mut self) {
        if self.kinds.is_empty() {
            return std::future::pending().await;
        }
        let any = self.kinds.iter_mut().map(|s| Box::pin(s.recv()));
        if futures_util::future::select_all(any).await.0.is_none() {
            std::future::pending::<()>().await;
        }
    }

    #[cfg(not(unix))]
    async fn next(&mut self) {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// A second stop signal while the box is made or removed: exits now and leaves it running.
fn abandon(out: &mut impl Out, sandbox: &str) -> ! {
    let id = if sandbox.is_empty() {
        "id not known yet"
    } else {
        sandbox
    };
    out.line(Kind::Failed, "Stopped without removing the sandbox", id);
    std::process::exit(INTERRUPTED);
}

fn done(what: &str, t0: Instant) -> String {
    format!("{what} · {}", progress::secs(t0.elapsed()))
}

async fn drive(
    sb: &Sandbox,
    cfg: &BoxSettings,
    job: &Job,
    out: &mut impl Watch,
    mut setup: Setup,
    created: Instant,
) -> Result<Outcome> {
    let log = Log {
        calls: Arc::default(),
        trace: out.trace(),
    };
    let observed = log.clone();
    let web = Tools::time_portal(sb.as_of())
        .with_client(cfg.client.clone())
        .with_observer(move |call| observed.push(call))
        .build()?;
    let relay = prepare(sb, cfg, &job.agent, &web, out, &mut setup).await;
    let relay = match relay {
        Ok(relay) => {
            setup.ready(out);
            relay
        }
        Err(e) => {
            setup.failed(out);
            return Err(e);
        }
    };
    let budget = harness_budget(cfg.timeout, created.elapsed())?;
    out.status(
        &format!("Starting {}", job.agent.title()),
        &format!(" · {}", job.model),
    );
    let started = format!("{} · {}", job.agent.title(), job.model);
    let t1 = Instant::now();
    let code = match &job.agent {
        Agent::App(harness, _) => {
            let ran = app(sb, cfg, job, *harness, budget, &started, out).await;
            if let Some(relay) = relay
                && let Err(e) = relay.stop().await
            {
                tracing::warn!(%e, "stopping the relay");
            }
            ran?
        }
        Agent::Builtin(llm) => {
            out.line(Kind::Step, "Agent started", &started);
            builtin(sb, cfg, job, llm, web, budget, out).await
        }
    };
    if code == 0 {
        out.line(Kind::Step, "Agent finished", &progress::secs(t1.elapsed()));
    }
    let downloaded = download(sb, cfg, out).await;
    Ok(Outcome {
        sandbox: sb.id().to_owned(),
        as_of: sb.as_of(),
        calls: log.take(),
        code: downloaded.map(|()| code),
    })
}

/// Runs the built-in harness to its answer within `budget`: its exit code.
async fn builtin(
    sb: &Sandbox,
    cfg: &BoxSettings,
    job: &Job,
    llm: &Llm,
    web: Tools,
    budget: Duration,
    out: &mut impl Watch,
) -> i32 {
    let run = sampler::Run {
        llm,
        prompt: &job.prompt,
        sandbox: sb,
        web,
        cwd: &cfg.cwd,
        max_turns: cfg.max_turns,
    };
    match tokio::time::timeout(budget, run.drive(out)).await {
        Ok(Ok(())) => 0,
        Ok(Err(why)) => {
            out.line(Kind::Failed, &why, "");
            out.line(Kind::Failed, "Agent stopped: the model endpoint failed", "");
            1
        }
        Err(_) => {
            out.line(Kind::Failed, "Agent ran out of time", "");
            124
        }
    }
}

/// Runs the installed harness as the box's command to its end: its exit code.
async fn app(
    sb: &Sandbox,
    cfg: &BoxSettings,
    job: &Job,
    harness: Harness,
    budget: Duration,
    started: &str,
    out: &mut impl Watch,
) -> Result<i32> {
    let task = Task {
        harness,
        model: &job.model,
        small_model: cfg.small_model.as_deref().unwrap_or(&job.model),
        prompt: &job.prompt,
        cwd: &cfg.cwd,
        max_turns: cfg.max_turns,
    };
    let launch = task.launch();
    let mut cmd = sb
        .command(launch.argv)
        .with_cwd(&cfg.cwd)
        .with_timeout(budget);
    for (k, v) in launch.env {
        cmd = cmd.with_env(k, v);
    }
    let exec = cmd.start().await?;
    out.line(Kind::Step, "Agent started", started);
    let (mut lines, mut errs) = (Lines::default(), Lines::default());
    let mut stderr_tail = VecDeque::with_capacity(STDERR_TAIL + 1);
    while let Some(chunk) = exec.next().await? {
        if chunk.stream == Stream::Stderr {
            for l in errs.push(&chunk.data).unwrap_or_default() {
                tracing::debug!("harness stderr: {}", String::from_utf8_lossy(&l));
                stderr_tail.push_back(l);
                if stderr_tail.len() > STDERR_TAIL {
                    stderr_tail.pop_front();
                }
            }
            continue;
        }
        for event in lines
            .push(&chunk.data)
            .context("reading the harness's events")?
        {
            out.event(&event);
        }
    }
    if let Some(rest) = lines.finish() {
        out.event(&rest);
    }
    let (code, ended) = exit_code(exec.exit());
    if code != 0 {
        for l in &stderr_tail {
            out.line(Kind::Failed, &String::from_utf8_lossy(l), "");
        }
        out.line(Kind::Failed, &format!("Agent {ended}"), "");
    }
    Ok(code)
}

/// How long the harness may run in a box whose `lifetime` started `used` ago: what is left, less a
/// margin for the downloads and cleanup after it.
fn harness_budget(lifetime: Duration, used: Duration) -> Result<Duration> {
    let left = lifetime.saturating_sub(used);
    let budget = left.saturating_sub(WIND_DOWN.min(lifetime / 4));
    if budget < Duration::from_secs(1) {
        bail!("the box's --timeout of {lifetime:?} left no time to run the harness");
    }
    Ok(Duration::from_secs(budget.as_secs()))
}

/// How the harness ended, as a shell's exit code, and in words.
fn exit_code(exit: Option<ExecExit>) -> (i32, String) {
    match exit {
        Some(ExecExit::Exited { code }) => (code, format!("exited with code {code}")),
        Some(ExecExit::Timeout) => (124, "ran out of time".into()),
        Some(ExecExit::Signaled { signal }) => {
            (128 + signal, format!("was killed by signal {signal}"))
        }
        Some(ExecExit::OutOfMemory) => (137, "ran out of memory".into()),
        Some(ExecExit::SandboxEnded) => (1, "stopped: the box ended".into()),
        other => (1, format!("ended: {other:?}")),
    }
}

/// Everything between a live box and the agent's start: its directories, the uploads, and, for an
/// installed harness, the relay serving it the model and the web tools.
async fn prepare(
    sb: &Sandbox,
    cfg: &BoxSettings,
    agent: &Agent,
    web: &Tools,
    out: &mut impl Out,
    setup: &mut Setup,
) -> Result<Option<Tunnel>> {
    setup.now(out, "Preparing directories");
    let made = sb.command(["mkdir", "-p", &cfg.cwd]).output().await?;
    if !made.success() {
        bail!(
            "preparing the box: {}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
    if !cfg.uploads.is_empty() {
        let t1 = Instant::now();
        for (local, dest) in &cfg.uploads {
            setup.now(out, &format!("Uploading {}", local.display()));
            if local.is_dir() {
                sb.files().upload_dir(local, dest).await?;
            } else {
                sb.files().upload_file(local, dest).await?;
            }
        }
        let what = format!("{} path(s)", cfg.uploads.len());
        setup.step("Uploaded", done(&what, t1));
    }
    let Agent::App(_, upstream) = agent else {
        return Ok(None);
    };
    setup.now(out, "Connecting model and web tools");
    let t2 = Instant::now();
    let endpoint = Endpoint::new(upstream.clone(), sb.as_of().timestamp())?;
    let tunnel = sb
        .serve_http(harness::PORT, proxy::router(endpoint, web.clone()))
        .await
        .context("serving the model and web tools to the box")?;
    let took = format!("· {}", progress::secs(t2.elapsed()));
    setup.step("Connected model and web tools", took);
    Ok(Some(tunnel))
}

/// A path the agent didn't write is skipped with a warning; any other failure is shown, the rest
/// still tried, and the first failure returned.
async fn download(sb: &Sandbox, cfg: &BoxSettings, out: &mut impl Out) -> Result<()> {
    let t0 = Instant::now();
    let (mut got, mut first) = (0, None);
    for (remote, local) in &cfg.downloads {
        out.status("Downloading", &format!(" {remote}"));
        let stat = match sb.files().stat(remote).await {
            Ok(stat) => stat,
            Err(e) => {
                out.line(
                    Kind::Failed,
                    &format!("Not downloaded: {remote}"),
                    &e.to_string(),
                );
                if !missing(&e) {
                    first.get_or_insert(anyhow!(e).context(format!("downloading {remote}")));
                }
                continue;
            }
        };
        let fetched = if stat.file_type == FileType::Directory {
            sb.files().download_dir(remote, local).await
        } else {
            sb.files().download_file(remote, local).await
        };
        match fetched {
            Ok(()) => got += 1,
            Err(e) => {
                out.line(
                    Kind::Failed,
                    &format!("Download failed: {remote}"),
                    &e.to_string(),
                );
                first.get_or_insert(anyhow!(e).context(format!("downloading {remote}")));
            }
        }
    }
    if got > 0 {
        out.line(
            Kind::Step,
            "Downloaded",
            &done(&format!("{got} path(s)"), t0),
        );
    }
    first.map_or(Ok(()), Err)
}

/// Whether `e` says the path is not there, rather than that it could not be looked at.
fn missing(e: &SandboxError) -> bool {
    matches!(
        e,
        SandboxError::Api(ApiError::Api {
            status: NOT_FOUND,
            ..
        })
    )
}

#[cfg(test)]
mod tests {
    use axum::extract::Query;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;

    use super::*;

    struct Quiet;

    impl Out for Quiet {
        fn line(&mut self, _: Kind, _: &str, _: &str) {}
        fn status(&mut self, _: &str, _: &str) {}
    }

    /// A box whose `/gone` is missing and whose `/broken` can't be looked at.
    async fn sandbox() -> Sandbox {
        let info = json!({"id": "sbx_1", "state": "live", "as_of": "2025-03-01T23:59:59Z",
            "network": "none", "vcpus": 1, "memory_gib": 1, "timeout": "1h",
            "idle_timeout": "10m", "created_at": "2025-03-01T23:59:59Z"});
        let stat = |Query(q): Query<Vec<(String, String)>>| async move {
            let (status, kind) = match q[0].1.as_str() {
                "/gone" => (StatusCode::NOT_FOUND, "not_found"),
                _ => (StatusCode::FORBIDDEN, "trial_only"),
            };
            let error = json!({"error": {"type": kind, "message": kind}});
            (status, Json(error)).into_response()
        };
        let app = Router::new()
            .route(
                "/v1/time-portal/sandbox",
                post(move || async move { Json(info) }),
            )
            .route("/v1/time-portal/sandbox/sbx_1/files/stat", get(stat));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = Client::builder()
            .with_api_key("k")
            .with_base_url(url)
            .build()
            .unwrap();
        Sandbox::time_portal("2025-03-01T23:59:59Z".parse().unwrap())
            .with_client(client)
            .build()
            .await
            .unwrap()
    }

    fn settings(downloads: &[&str]) -> BoxSettings {
        BoxSettings {
            client: Client::builder().with_api_key("k").build().unwrap(),
            small_model: None,
            network: Network::None,
            uploads: Vec::new(),
            downloads: downloads
                .iter()
                .map(|d| ((*d).to_owned(), PathBuf::from("unused")))
                .collect(),
            cwd: "/work".into(),
            vcpus: 1,
            memory_gib: 1,
            timeout: Duration::from_secs(60),
            max_turns: None,
            keep: false,
        }
    }

    #[test]
    fn the_harness_gets_the_box_life_left_less_time_to_wind_down() {
        let hour = Duration::from_secs(3600);
        let budget = harness_budget(hour, Duration::from_secs(30)).unwrap();
        assert_eq!(budget, Duration::from_secs(3600 - 30 - 300));
        let short = harness_budget(Duration::from_secs(120), Duration::from_secs(10)).unwrap();
        assert_eq!(
            short,
            Duration::from_secs(80),
            "a short box keeps back a quarter"
        );
        assert!(harness_budget(Duration::from_secs(60), Duration::from_secs(59)).is_err());
    }

    #[test]
    fn each_ending_reads_as_a_shell_exit_code() {
        assert_eq!(exit_code(Some(ExecExit::Exited { code: 3 })).0, 3);
        assert_eq!(exit_code(Some(ExecExit::Timeout)).0, 124);
        assert_eq!(exit_code(Some(ExecExit::Signaled { signal: 15 })).0, 143);
        assert_eq!(exit_code(Some(ExecExit::OutOfMemory)).0, 137);
        assert_eq!(exit_code(Some(ExecExit::SandboxEnded)).0, 1);
        assert_eq!(exit_code(None).0, 1);
    }

    /// Signals that land before `next` is awaited are counted, one each.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_signal_sent_before_it_is_awaited_still_counts() {
        let mut stops = Stops::new();
        let hup = || {
            let pid = std::process::id().to_string();
            let sent = std::process::Command::new("kill")
                .args(["-HUP", &pid])
                .status();
            assert!(sent.unwrap().success());
        };
        for nth in ["first", "second"] {
            hup();
            let counted = tokio::time::timeout(Duration::from_secs(5), stops.next()).await;
            assert!(counted.is_ok(), "the {nth} signal was lost");
        }
    }

    #[tokio::test]
    async fn a_missing_path_is_skipped_but_a_failed_look_is_an_error() {
        let sb = sandbox().await;

        assert!(
            download(&sb, &settings(&["/gone"]), &mut Quiet)
                .await
                .is_ok()
        );
        let err = download(&sb, &settings(&["/gone", "/broken"]), &mut Quiet)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("/broken"), "{err:#}");
    }
}
