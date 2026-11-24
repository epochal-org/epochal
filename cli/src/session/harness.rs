//! The harnesses this runs, and how each is launched in the box against the relay's port: its model
//! endpoint, and the `epochal` MCP server for search and fetch.

use std::collections::BTreeMap;

use clap::ValueEnum;
use serde_json::json;

pub const PORT: u16 = 8787;
/// The MCP server's name, which the harness prefixes to its tools: `mcp__epochal__web_search`.
const MCP_NAME: &str = "epochal";
const PLACEHOLDER_KEY: &str = "epochal-local-proxy";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Harness {
    Claude,
    Codex,
    Grok,
}

impl Harness {
    pub fn name(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Codex => "Codex",
            Harness::Grok => "Grok Build",
        }
    }

    /// How `--harness` spells it.
    pub fn cli_name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
        }
    }

    /// The sandbox app that mounts this harness's binaries at `/opt/epochal/bin`, first on the box's PATH.
    pub fn app(self) -> &'static str {
        match self {
            Harness::Claude => "claude-code",
            Harness::Codex => "codex",
            Harness::Grok => "grok",
        }
    }
}

/// grok reads its model and MCP servers only from `config.toml`; `GROK_CONFIG` drops both.
const GROK_HOME: &str = "/tmp/epochal-grok";
const GROK_CONFIG_ENV: &str = "EPOCHAL_GROK_CONFIG";
/// Built-ins that reach xAI's own services rather than the model endpoint.
const GROK_DISALLOWED: &str =
    "image_gen,image_edit,image_to_video,reference_to_video,send_feedback";

#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

pub struct Task<'a> {
    pub harness: Harness,
    pub model: &'a str,
    pub small_model: &'a str,
    pub prompt: &'a str,
    pub cwd: &'a str,
    pub max_turns: Option<u32>,
}

impl Task<'_> {
    pub fn launch(&self) -> Launch {
        let base = format!("http://127.0.0.1:{PORT}");
        let mcp = format!("{base}/mcp");
        let mut env = BTreeMap::new();
        let argv = match self.harness {
            Harness::Claude => {
                for (k, v) in [
                    ("ANTHROPIC_BASE_URL", base.as_str()),
                    ("ANTHROPIC_AUTH_TOKEN", PLACEHOLDER_KEY),
                    ("ANTHROPIC_MODEL", self.model),
                    ("ANTHROPIC_DEFAULT_OPUS_MODEL", self.model),
                    ("ANTHROPIC_DEFAULT_SONNET_MODEL", self.model),
                    ("ANTHROPIC_DEFAULT_HAIKU_MODEL", self.small_model),
                    ("CLAUDE_CODE_SUBAGENT_MODEL", self.model),
                    ("IS_SANDBOX", "1"),
                    ("DISABLE_AUTOUPDATER", "1"),
                    ("DISABLE_TELEMETRY", "1"),
                    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
                ] {
                    env.insert(k.to_owned(), v.to_owned());
                }
                let mut argv = vec![
                    "claude".into(),
                    "-p".into(),
                    self.prompt.into(),
                    "--output-format".into(),
                    "stream-json".into(),
                    "--verbose".into(),
                    "--dangerously-skip-permissions".into(),
                    "--no-session-persistence".into(),
                    "--strict-mcp-config".into(),
                    "--mcp-config".into(),
                    json!({"mcpServers": {MCP_NAME: {"type": "http", "url": mcp}}}).to_string(),
                    // Server-side web tools would reach the present; `epochal` serves the as-of web.
                    "--disallowedTools".into(),
                    "WebSearch WebFetch".into(),
                    "--model".into(),
                    self.model.into(),
                ];
                if let Some(turns) = self.max_turns {
                    argv.extend(["--max-turns".into(), turns.to_string()]);
                }
                argv
            }
            Harness::Codex => {
                env.insert("OPENAI_API_KEY".into(), PLACEHOLDER_KEY.into());
                let mut argv = vec![
                    "codex".into(),
                    "exec".into(),
                    "--json".into(),
                    "--dangerously-bypass-approvals-and-sandbox".into(),
                    "--skip-git-repo-check".into(),
                    "--ephemeral".into(),
                    "-C".into(),
                    self.cwd.into(),
                    "-m".into(),
                    self.model.into(),
                ];
                for kv in [
                    r#"model_provider="epochal""#.to_owned(),
                    r#"model_providers.epochal.name="epochal""#.to_owned(),
                    format!(r#"model_providers.epochal.base_url="{base}/v1""#),
                    r#"model_providers.epochal.env_key="OPENAI_API_KEY""#.to_owned(),
                    r#"model_providers.epochal.wire_api="responses""#.to_owned(),
                    "check_for_update_on_startup=false".to_owned(),
                    r#"web_search="disabled""#.to_owned(),
                    format!(r#"mcp_servers.{MCP_NAME}.url="{mcp}""#),
                ] {
                    argv.extend(["-c".into(), kv]);
                }
                argv.push(self.prompt.into());
                argv
            }
            Harness::Grok => {
                env.insert("GROK_HOME".into(), GROK_HOME.into());
                env.insert(GROK_CONFIG_ENV.into(), self.grok_config(&base, &mcp));
                let mut grok = vec![
                    "grok".to_owned(),
                    "-p".into(),
                    self.prompt.into(),
                    "--output-format".into(),
                    "streaming-messages-json".into(),
                    "--always-approve".into(),
                    "--disable-web-search".into(),
                    "--disallowed-tools".into(),
                    GROK_DISALLOWED.into(),
                    "-m".into(),
                    MCP_NAME.into(),
                    "--cwd".into(),
                    self.cwd.into(),
                ];
                if let Some(turns) = self.max_turns {
                    grok.extend(["--max-turns".into(), turns.to_string()]);
                }
                let write = format!(
                    r#"mkdir -p "$GROK_HOME" && printf '%s' "${GROK_CONFIG_ENV}" > "$GROK_HOME/config.toml" && exec "$0" "$@""#
                );
                let mut argv = vec!["/bin/sh".into(), "-c".into(), write];
                argv.extend(grok);
                argv
            }
        };
        Launch { argv, env }
    }

    /// One model, named after the MCP server, at the relay over the Responses API; its key is a
    /// placeholder the proxy replaces, set on the model so grok neither logs in nor probes xAI.
    fn grok_config(&self, base: &str, mcp: &str) -> String {
        let s = |v: &str| serde_json::to_string(v).expect("a string serializes");
        format!(
            "[models]\ndefault = {name}\n\n\
             [model.{MCP_NAME}]\nmodel = {model}\nbase_url = {base}\napi_backend = \"responses\"\napi_key = {key}\n\n\
             [mcp_servers.{MCP_NAME}]\nurl = {mcp}\n\n\
             [cli]\nauto_update = false\n",
            name = s(MCP_NAME),
            model = s(self.model),
            base = s(&format!("{base}/v1")),
            key = s(PLACEHOLDER_KEY),
            mcp = s(mcp),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(harness: Harness) -> Task<'static> {
        Task {
            harness,
            model: "deepseek-v4-pro",
            small_model: "claude-haiku-5-5",
            prompt: "hi",
            cwd: "/home/epochal",
            max_turns: None,
        }
    }

    #[test]
    fn claude_sends_every_tier_to_the_relay_with_the_chosen_models() {
        let l = task(Harness::Claude).launch();
        assert_eq!(l.env["ANTHROPIC_BASE_URL"], "http://127.0.0.1:8787");
        assert_eq!(l.env["ANTHROPIC_DEFAULT_OPUS_MODEL"], "deepseek-v4-pro");
        assert_eq!(l.env["ANTHROPIC_DEFAULT_HAIKU_MODEL"], "claude-haiku-5-5");
        assert!(!l.env.values().any(|v| v.starts_with("sk-")));
        let mcp = &l.argv[l.argv.iter().position(|a| a == "--mcp-config").unwrap() + 1];
        assert!(
            mcp.contains(r#""url":"http://127.0.0.1:8787/mcp""#),
            "{mcp}"
        );
    }

    #[test]
    fn codex_uses_a_custom_responses_provider_and_mcp_at_the_relay() {
        let l = task(Harness::Codex).launch();
        assert!(l.argv.contains(
            &r#"model_providers.epochal.base_url="http://127.0.0.1:8787/v1""#.to_owned()
        ));
        assert!(
            l.argv
                .contains(&r#"mcp_servers.epochal.url="http://127.0.0.1:8787/mcp""#.to_owned())
        );
        assert_eq!(l.argv.last().unwrap(), "hi");
    }

    /// Runs the launch's shell step with `grok` swapped for `echo`: the args it would get are
    /// printed, and the config it would read is left on disk.
    #[test]
    fn grok_reads_a_config_naming_the_relay_and_gets_the_prompt_verbatim() {
        let home = std::env::temp_dir().join(format!("grok-launch-{}", std::process::id()));
        let mut l = Task {
            prompt: r#"it's "$HOME" `x`"#,
            model: r#"we"ird\model"#,
            max_turns: Some(7),
            ..task(Harness::Grok)
        }
        .launch();
        l.env.insert("GROK_HOME".into(), home.display().to_string());
        assert_eq!(l.argv[3], "grok");
        l.argv[3] = "/bin/echo".into();
        let out = std::process::Command::new(&l.argv[0])
            .args(&l.argv[1..])
            .env_clear()
            .envs(&l.env)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let args = String::from_utf8(out.stdout).unwrap();
        assert!(args.starts_with(r#"-p it's "$HOME" `x` "#), "{args}");
        assert!(args.contains("-m epochal") && args.trim_end().ends_with("--max-turns 7"));
        let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
        for line in [
            "[model.epochal]",
            r#"model = "we\"ird\\model""#,
            r#"base_url = "http://127.0.0.1:8787/v1""#,
            r#"api_backend = "responses""#,
            r#"api_key = "epochal-local-proxy""#,
            r#"url = "http://127.0.0.1:8787/mcp""#,
        ] {
            assert!(config.lines().any(|l| l == line), "{line} in\n{config}");
        }
    }
}
