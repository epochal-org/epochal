//! `run`: one agent on one prompt in a box held to `--as-of`, its progress drawn live on stderr;
//! its answer is the report. The built-in harness runs unless `--harness` picks one to install in
//! the box, which is asked about first: its binary postdates the as-of.

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use bytesize::ByteSize;
use clap::Args as ClapArgs;
use epochal_api::Client;
use epochal_mcp::ToolCall;
use epochal_sandbox::Network;
use serde::Serialize;

use super::{Command, Done};
use crate::cli::AsOfArgs;
use crate::output::progress::{Kind, Out, Term};
use crate::output::trace::Trace;
use crate::output::watcher::Watcher;
use crate::output::{Report, instant};
use crate::sampler::Llm;
use crate::session::{self, Agent, BoxSettings, Harness, Job, Upstream};

#[derive(ClapArgs)]
pub struct Args {
    /// The prompt; `-` reads it from stdin
    prompt: String,
    /// Install this third-party harness in the box instead of running the built-in one. Its binary
    /// is a current build, so the run is not strictly held to --as-of
    #[arg(long, value_enum)]
    harness: Option<Harness>,
    /// Run a --harness without asking to confirm that it is not strictly held to --as-of
    #[arg(long, short = 'y')]
    yes: bool,
    /// Model id at EPOCHAL_LLM_BASE_URL (key: EPOCHAL_LLM_API_KEY), for the built-in harness and
    /// every --harness alike; e.g. claude-sonnet-5-5, gpt-6-sol, deepseek-v4-pro
    #[arg(long)]
    model: String,
    /// Model for --harness claude's background calls; defaults to --model
    #[arg(long)]
    small_model: Option<String>,
    #[command(flatten)]
    as_of: AsOfArgs,
    #[command(flatten)]
    sandbox: SandboxArgs,
}

#[derive(Serialize)]
pub struct RunReport {
    harness: &'static str,
    model: String,
    sandbox: String,
    as_of: String,
    code: i32,
    /// Why the downloads after the run failed; the answer is still kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    download_error: Option<String>,
    answer: String,
    calls: Vec<ToolCall>,
}

impl Command for Args {
    type Report = RunReport;

    async fn run(self, trace: Option<Trace>) -> Result<Done<RunReport>> {
        let sandbox = self.settings()?;
        let var = |name: &str| env::var(name).ok();
        let agent = match self.harness {
            None => Agent::Builtin(Llm::from_env(&self.model, var)?),
            Some(harness) => {
                let upstream = Upstream::from_env(var)?;
                confirm(harness, self.yes)?;
                Agent::App(harness, upstream)
            }
        };
        let job = Job {
            agent,
            model: self.model,
            prompt: text_arg(&self.prompt)?,
            as_of: self.as_of.as_of,
        };
        let mut term = Term::stderr();
        let mut watcher = Watcher::new(job.agent.dialect(), term.clone(), trace);
        let outcome = session::session(&sandbox, &job, &mut watcher).await;
        term.clear();
        let outcome = outcome?;
        let (code, download_error) = match outcome.code {
            Ok(code) => (code, None),
            Err(e) => (1, Some(format!("{e:#}"))),
        };
        if let Some(e) = &download_error {
            term.line(Kind::Failed, e, "");
        }
        Ok(Done {
            code,
            report: RunReport {
                harness: job.agent.name(),
                answer: watcher.answer().to_owned(),
                model: job.model,
                sandbox: outcome.sandbox,
                as_of: instant(outcome.as_of),
                code,
                download_error,
                calls: outcome.calls,
            },
        })
    }
}

/// Says what `--harness` gives up, then asks on a terminal; without one, only `--yes` goes on.
fn confirm(harness: Harness, yes: bool) -> Result<()> {
    let mut err = io::stderr().lock();
    writeln!(
        err,
        "warning: --harness {} installs today's {} in the box. Its version and built-in prompts \
         postdate --as-of and can tell the agent the real date, so this run is not strictly held \
         to --as-of.\n         Leave out --harness to use the built-in harness, which keeps the \
         agent's prompt, tools and box at --as-of.",
        harness.cli_name(),
        harness.name(),
    )?;
    if yes {
        return Ok(());
    }
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("not continuing without a terminal to ask on: pass --yes to run it anyway");
    }
    write!(err, "Continue? [y/N] ")?;
    err.flush()?;
    let mut answer = String::new();
    io::stdin().lock().read_line(&mut answer)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => bail!("aborted"),
    }
}

impl Report for RunReport {
    fn text(&self) -> String {
        self.answer.clone()
    }
}

/// How the box is made, and what is copied in and out.
#[derive(ClapArgs)]
struct SandboxArgs {
    #[arg(
        long,
        help = "LOCAL[:BOX_PATH], repeatable; BOX_PATH, after the last colon, defaults to --cwd/<name>"
    )]
    upload: Vec<String>,
    #[arg(
        long,
        help = "BOX_PATH[:LOCAL], repeatable; copied out after the run. BOX_PATH ends at the first colon"
    )]
    download: Vec<String>,
    #[arg(long, default_value = "/home/epochal")]
    cwd: String,
    /// Virtual CPUs reserved for the box
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    cpu: u32,
    /// Memory reserved for the box, whole GiB: 4Gi, 4GiB, 4096MiB
    #[arg(long, default_value = "2Gi", value_parser = gib)]
    memory: u32,
    /// Box lifetime, whole seconds: 90s, 10m, 1h30m
    #[arg(long, default_value = "1h", value_parser = lifetime)]
    timeout: Duration,
    /// Cap on model turns; not for codex. The built-in harness's last turn must answer
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    max_turns: Option<u32>,
    /// Leave the box running afterwards
    #[arg(long)]
    keep: bool,
    /// Let the box's own processes (curl, pip, git) reach the web as it stood at --as-of, billed
    /// per page; the agent's model and web tool calls go through this machine either way
    #[arg(long)]
    internet: bool,
}

/// Memory in whole GiB, the unit the API reserves in.
fn gib(text: &str) -> Result<u32, String> {
    const GIB: u64 = 1 << 30;
    let bytes = text.parse::<ByteSize>()?.as_u64();
    if bytes == 0 || bytes % GIB != 0 {
        return Err(format!("{text} is not a whole number of GiB; use e.g. 4Gi"));
    }
    u32::try_from(bytes / GIB).map_err(|_| format!("{text} is too large"))
}

/// A lifetime in whole seconds, the unit the API takes.
fn lifetime(text: &str) -> Result<Duration, String> {
    let d = humantime::parse_duration(text).map_err(|e| e.to_string())?;
    if d.is_zero() || d.subsec_nanos() != 0 {
        return Err(format!("{text} is not a positive whole number of seconds"));
    }
    Ok(d)
}

impl Args {
    /// The box settings, refusing flags the chosen harness does not take.
    fn settings(&self) -> Result<BoxSettings> {
        let s = &self.sandbox;
        if s.max_turns.is_some() && self.harness == Some(Harness::Codex) {
            bail!("--max-turns is not for codex: codex exec has no turn limit");
        }
        if self.small_model.is_some() && self.harness != Some(Harness::Claude) {
            bail!("--small-model is only for --harness claude, whose background calls it names");
        }
        Ok(BoxSettings {
            client: Client::builder().build()?,
            small_model: self.small_model.clone(),
            network: if s.internet {
                Network::TimePortal
            } else {
                Network::None
            },
            uploads: s
                .upload
                .iter()
                .map(|spec| upload(spec, &s.cwd))
                .collect::<Result<_>>()?,
            downloads: s.download.iter().map(|d| download(d)).collect(),
            cwd: s.cwd.clone(),
            vcpus: s.cpu,
            memory_gib: s.memory,
            timeout: s.timeout,
            max_turns: s.max_turns,
            keep: s.keep,
        })
    }
}

/// `LOCAL[:BOX_PATH]`, the box path defaulting to `cwd/<name>`. A spec that names a local path
/// whole is that path; any other splits at the last colon, so a box path holds none.
fn upload(spec: &str, cwd: &str) -> Result<(PathBuf, String)> {
    let split = spec.rsplit_once(':').filter(|_| !Path::new(spec).exists());
    let (local, remote) = match split {
        Some((l, r)) => (PathBuf::from(l), r.to_owned()),
        None => {
            let local = PathBuf::from(spec);
            let name = local.file_name().context("upload path has no name")?;
            let remote = format!("{cwd}/{}", name.to_string_lossy());
            (local, remote)
        }
    };
    if !local.exists() {
        bail!("{} does not exist", local.display());
    }
    Ok((local, remote))
}

/// `BOX_PATH[:LOCAL]`, the local path defaulting to the box path's last part, here. It splits at
/// the first colon, so a box path may not hold one.
fn download(spec: &str) -> (String, PathBuf) {
    match spec.split_once(':') {
        Some((r, l)) => (r.to_owned(), PathBuf::from(l)),
        None => {
            let name = spec.rsplit('/').next().unwrap_or(spec);
            (spec.to_owned(), PathBuf::from(name))
        }
    }
}

/// The text of an argument, or of stdin for `-`.
fn text_arg(arg: &str) -> Result<String> {
    match arg {
        "-" => Ok(io::read_to_string(io::stdin())?),
        text => Ok(text.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_paths_default_to_the_name_on_the_other_side() {
        assert_eq!(
            download("/work/out/report.md"),
            ("/work/out/report.md".into(), PathBuf::from("report.md"))
        );
        assert_eq!(
            download("/work/out:./here"),
            ("/work/out".into(), PathBuf::from("./here"))
        );
        let here = std::env::current_dir().unwrap();
        let name = here.file_name().unwrap().to_string_lossy().into_owned();
        let (local, remote) = upload(here.to_str().unwrap(), "/work").unwrap();
        assert_eq!((local, remote), (here, format!("/work/{name}")));
        assert!(upload("/no/such/path", "/work").is_err());
    }

    /// On Windows `a:b` is a drive-relative path, not a file name.
    #[cfg(unix)]
    #[test]
    fn an_upload_may_name_a_local_file_with_a_colon() {
        let dir = tempfile::tempdir().unwrap();
        let colon = dir.path().join("a:b");
        std::fs::write(&colon, "").unwrap();
        let spec = format!("{}:/work/c", colon.display());
        assert_eq!(
            upload(&spec, "/work").unwrap(),
            (colon.clone(), "/work/c".into())
        );
        let alone = upload(colon.to_str().unwrap(), "/work").unwrap();
        assert_eq!(
            alone,
            (colon, "/work/a:b".into()),
            "a local name with a colon, no box path"
        );
    }

    #[test]
    fn small_model_is_refused_but_for_claude() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(flatten)]
            run: Args,
        }
        let settings = |harness: &[&str]| {
            let argv = ["epochal", "p", "--model", "m", "--as-of", "2025-03-01"];
            let small = ["--small-model", "s"];
            let cli =
                <Cli as clap::Parser>::try_parse_from(argv.iter().chain(&small).chain(harness))
                    .unwrap();
            cli.run.settings().err().map(|e| e.to_string())
        };
        for refused in [&[][..], &["--harness", "codex"], &["--harness", "grok"]] {
            let err = settings(refused).expect("refused");
            assert!(err.contains("--small-model"), "{err}");
        }
        let claude = settings(&["--harness", "claude"]);
        assert!(!claude.is_some_and(|e| e.contains("--small-model")));
    }

    #[test]
    fn memory_takes_whole_gib_in_any_binary_spelling() {
        assert_eq!(gib("2Gi"), Ok(2));
        assert_eq!(gib("2GiB"), Ok(2));
        assert_eq!(gib("2048MiB"), Ok(2));
        for bad in ["1.5Gi", "2GB", "512Mi", "0Gi", "lots"] {
            assert!(gib(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn timeout_takes_human_durations_of_whole_seconds() {
        assert_eq!(lifetime("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(lifetime("1h 30m"), Ok(Duration::from_secs(5400)));
        assert_eq!(lifetime("10m"), Ok(Duration::from_secs(600)));
        for bad in ["0s", "1500ms", "10", "soon"] {
            assert!(lifetime(bad).is_err(), "{bad}");
        }
    }
}
