//! The agent's tools, named as Claude Code names its own: `Bash`, `Read`, `Write` and `Edit` work in
//! the box through the sandbox API; `web_search` and `web_fetch` are `epochal-mcp`'s, held to the
//! box's as-of. What a command prints is kept within [`MAX_OUTPUT`] as it streams, so no command
//! can fill this machine's memory; a web answer is already bounded by the tool's own cap.

use std::time::Duration;

use epochal_mcp::Tools as Web;
use epochal_sandbox::{ExecExit, Sandbox};
use serde::Deserialize;
use serde_json::{Value, json};

/// The most of a tool's answer the model is shown; longer is cut in the middle.
const MAX_OUTPUT: usize = 30_000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// How long reading or writing one file may take; a FIFO or a hung mount fails rather than waits.
const FILE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_LINES: usize = 2_000;
/// What the `READ` script prints after a line it had to cut.
const LONG_LINE: &str = "[long line ";
/// The most bytes of numbered lines one `Read` returns, leaving room for its footer.
const READ_MAX: usize = MAX_OUTPUT - 200;
/// The largest file `Write` and `Edit` handle: one command's stdin.
const MAX_FILE: usize = 1 << 20;
/// `$1` the file, `$2`/`$3` the first and last line, `$4` the byte budget: its line count, then
/// those lines numbered, whole, as many as fit; a first line too long alone is cut to the budget
/// and followed by a [`LONG_LINE`] note.
const READ: &str = r#"[ -f "$1" ] || { echo "$1 is not a file" >&2; exit 1; }
grep -c '' -- "$1"
awk -v a="$2" -v b="$3" -v max="$4" 'NR < a { next }
  { n = length($0) + 8; if (used + n > max) { if (!used) printf "%6d\t%s\n[long line %d]\n", NR, substr($0, 1, max - 80), NR; exit }
    printf "%6d\t%s\n", NR, $0; used += n }
  NR >= b { exit }' "$1""#;
const WRITE: &str = r#"mkdir -p -- "$(dirname -- "$1")" && cat > "$1""#;

/// A tool as the model is told of it.
pub struct Def {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

/// What the agent works with: its box and the web at the box's as-of.
pub struct Computer<'a> {
    pub sandbox: &'a Sandbox,
    pub web: Web,
    pub cwd: &'a str,
}

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    timeout_seconds: Option<u64>,
}

#[derive(Deserialize)]
struct ReadArgs {
    file_path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct WriteArgs {
    file_path: String,
    content: String,
}

#[derive(Deserialize)]
struct EditArgs {
    file_path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

/// A finished command: what it printed, stdout and stderr in the order they came, within
/// [`MAX_OUTPUT`], and how it ended.
struct Finished {
    text: String,
    exit: ExecExit,
}

impl Computer<'_> {
    pub fn defs(&self) -> Vec<Def> {
        let def = |name: &str, description: &str, schema: Value| Def {
            name: name.into(),
            description: description.into(),
            schema,
        };
        let path = json!({"type": "string", "description": "Absolute, or relative to the working directory"});
        let mut defs = vec![
            def(
                "Bash",
                "Runs a bash command on your computer, an isolated Linux box, in the working directory, and returns what it printed and, if it failed, its exit code. Files you write stay for the rest of the task. Long output is cut in the middle: write it to a file and read it in parts.",
                json!({"type": "object", "properties": {
                    "command": {"type": "string"},
                    "timeout_seconds": {"type": "integer", "description": "Kill the command after this long; default 120, at most 600"},
                }, "required": ["command"]}),
            ),
            def(
                "Read",
                "Reads a text file, each line prefixed with its 1-based number. Reads `limit` lines (default 2000) from line `offset` (default 1).",
                json!({"type": "object", "properties": {
                    "file_path": path, "offset": {"type": "integer"}, "limit": {"type": "integer"},
                }, "required": ["file_path"]}),
            ),
            def(
                "Write",
                "Writes a text file of at most 1 MiB, creating it and its directories or replacing what is there.",
                json!({"type": "object", "properties": {"file_path": path, "content": {"type": "string"}},
                    "required": ["file_path", "content"]}),
            ),
            def(
                "Edit",
                "Replaces `old_string` with `new_string` in a text file. `old_string` must match exactly and be unique, unless `replace_all` is set.",
                json!({"type": "object", "properties": {
                    "file_path": path, "old_string": {"type": "string"}, "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"},
                }, "required": ["file_path", "old_string", "new_string"]}),
            ),
        ];
        defs.extend(self.web.definitions().into_iter().map(|t| Def {
            name: t.name.into_owned(),
            description: t.description.map(Into::into).unwrap_or_default(),
            schema: Value::Object((*t.input_schema).clone()),
        }));
        defs
    }

    /// Runs one call: its answer for the model, and whether it failed.
    pub async fn call(&self, name: &str, input: Value) -> (String, bool) {
        let ran = match name {
            "Bash" => self.bash(input).await,
            "Read" => self.read(input).await,
            "Write" => self.write(input).await,
            "Edit" => self.edit(input).await,
            _ => return self.web_call(name, input).await,
        };
        match ran {
            Ok(text) => (text, false),
            Err(why) => (why, true),
        }
    }

    async fn web_call(&self, name: &str, input: Value) -> (String, bool) {
        let result = self.web.call(name, input).await;
        let text: Vec<&str> = result
            .content
            .iter()
            .filter_map(|c| Some(c.as_text()?.text.as_str()))
            .collect();
        (text.join("\n"), result.is_error == Some(true))
    }

    async fn bash(&self, input: Value) -> Result<String, String> {
        let a: BashArgs = args(input)?;
        let limit = a
            .timeout_seconds
            .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
        let limit = limit.clamp(Duration::from_secs(1), MAX_TIMEOUT);
        let ran = self
            .run(["bash", "-c", &a.command], limit, Vec::new())
            .await?;
        let text = ran.text.trim_end();
        let ended = match ran.exit {
            ExecExit::Exited { code: 0 } => return Ok(text.to_owned()),
            ExecExit::Exited { code } => format!("[exit code: {code}]"),
            ExecExit::Timeout => format!("[killed after {}s]", limit.as_secs()),
            ExecExit::OutOfMemory => "[killed: out of memory]".into(),
            ExecExit::Signaled { signal } => format!("[killed by signal {signal}]"),
            other => format!("[ended: {other:?}]"),
        };
        Err(format!("{text}\n{ended}").trim_start().to_owned())
    }

    async fn read(&self, input: Value) -> Result<String, String> {
        let a: ReadArgs = args(input)?;
        let first = a.offset.unwrap_or(1).max(1);
        let limit = a.limit.unwrap_or(DEFAULT_LINES).max(1);
        let last = first.saturating_add(limit - 1);
        let (first_s, last_s, max) = (first.to_string(), last.to_string(), READ_MAX.to_string());
        let argv = [
            "sh",
            "-c",
            READ,
            "sh",
            &a.file_path,
            &first_s,
            &last_s,
            &max,
        ];
        let ran = self.run(argv, FILE_TIMEOUT, Vec::new()).await?;
        if ran.exit != (ExecExit::Exited { code: 0 }) {
            return Err(ran.text.trim().to_owned());
        }
        let (total, lines) = ran.text.split_once('\n').unwrap_or((&ran.text, ""));
        let total: usize = total.trim().parse().unwrap_or(0);
        if total == 0 {
            return Ok("(empty file)".into());
        }
        if first > total {
            let path = &a.file_path;
            return Err(format!(
                "{path} has {total} lines; offset {first} is past the end"
            ));
        }
        // Every line the script prints is numbered but its note, which comes last.
        let note = format!("{LONG_LINE}{first}]\n");
        let (lines, cut) = match lines.strip_suffix(&note) {
            Some(lines) => (lines, true),
            None => (lines, false),
        };
        let shown = lines.lines().count().max(1);
        let mut out = lines.to_owned();
        if cut {
            // The characters shown follow the line's number and tab.
            let next = lines
                .trim_end_matches('\n')
                .chars()
                .count()
                .saturating_sub(7)
                + 1;
            let path = &a.file_path;
            out.push_str(&format!(
                "[line {first} is cut: it is longer than one read returns; read the rest with Bash, e.g. `cut -c {next}- {path}`]\n"
            ));
        }
        let last = (first + shown - 1).min(total);
        if last < total {
            let next = last + 1;
            out.push_str(&format!(
                "[lines {first}-{last} of {total}; read from offset {next} for more]"
            ));
        }
        Ok(out)
    }

    async fn write(&self, input: Value) -> Result<String, String> {
        let a: WriteArgs = args(input)?;
        self.put(&a.file_path, a.content.into_bytes()).await?;
        Ok(format!("wrote {}", a.file_path))
    }

    async fn edit(&self, input: Value) -> Result<String, String> {
        let a: EditArgs = args(input)?;
        if a.old_string.is_empty() {
            return Err("old_string is empty; use Write to create a file".into());
        }
        let path = &a.file_path;
        // One byte past the limit tells a file too large to edit from one that just fits.
        let max = (MAX_FILE + 1).to_string();
        let exec = self
            .sandbox
            .command(["head", "-c", &max, "--", path])
            .with_cwd(self.cwd)
            .with_timeout(FILE_TIMEOUT)
            .output()
            .await
            .map_err(|e| format!("could not read {path}: {e}"))?;
        if !exec.success() {
            return Err(String::from_utf8_lossy(&exec.stderr).trim().to_owned());
        }
        if exec.stdout.len() > MAX_FILE {
            return Err(format!("{path} is over 1 MiB; edit it with Bash"));
        }
        let text = String::from_utf8(exec.stdout)
            .map_err(|_| format!("{path} is not UTF-8 text; edit it with Bash"))?;
        let edited = replace(&text, &a.old_string, &a.new_string, a.replace_all)?;
        self.put(path, edited.into_bytes()).await?;
        Ok(format!("edited {path}"))
    }

    async fn put(&self, path: &str, data: Vec<u8>) -> Result<(), String> {
        if data.len() > MAX_FILE {
            let len = data.len();
            return Err(format!(
                "{len} bytes is over the 1 MiB one write takes; write it in parts with Bash"
            ));
        }
        let ran = self
            .run(["sh", "-c", WRITE, "sh", path], FILE_TIMEOUT, data)
            .await?;
        match ran.exit {
            ExecExit::Exited { code: 0 } => Ok(()),
            _ => Err(ran.text.trim().to_owned()),
        }
    }

    /// Runs `argv` in the working directory, keeping what it prints within [`MAX_OUTPUT`].
    async fn run<const N: usize>(
        &self,
        argv: [&str; N],
        limit: Duration,
        stdin: Vec<u8>,
    ) -> Result<Finished, String> {
        let failed = |e: epochal_sandbox::Error| format!("could not run the command: {e}");
        let exec = self
            .sandbox
            .command(argv)
            .with_cwd(self.cwd)
            .with_timeout(limit)
            .with_stdin(stdin)
            .start()
            .await
            .map_err(failed)?;
        let mut out = Capped::default();
        while let Some(chunk) = exec.next().await.map_err(failed)? {
            out.push(&chunk.data);
        }
        Ok(Finished {
            text: out.text(),
            exit: exec.exit().unwrap_or(ExecExit::Unknown),
        })
    }
}

fn args<T: for<'de> Deserialize<'de>>(input: Value) -> Result<T, String> {
    serde_json::from_value(input).map_err(|e| format!("invalid arguments: {e}"))
}

fn replace(text: &str, old: &str, new: &str, all: bool) -> Result<String, String> {
    match text.matches(old).count() {
        0 => Err("old_string is not in the file".into()),
        1 => Ok(text.replacen(old, new, 1)),
        _ if all => Ok(text.replace(old, new)),
        n => Err(format!(
            "old_string is in the file {n} times; add context to make it unique, or set replace_all"
        )),
    }
}

/// A stream kept as its first and last `MAX_OUTPUT / 2` bytes, and how many were dropped between.
#[derive(Default)]
struct Capped {
    head: Vec<u8>,
    tail: Vec<u8>,
    dropped: usize,
}

impl Capped {
    fn push(&mut self, data: &[u8]) {
        let half = MAX_OUTPUT / 2;
        let room = half.saturating_sub(self.head.len());
        let (into_head, rest) = data.split_at(room.min(data.len()));
        self.head.extend_from_slice(into_head);
        self.tail.extend_from_slice(rest);
        if self.tail.len() > half {
            let over = self.tail.len() - half;
            self.tail.drain(..over);
            self.dropped += over;
        }
    }

    fn text(&self) -> String {
        if self.dropped == 0 {
            return String::from_utf8_lossy(&[self.head.as_slice(), &self.tail].concat())
                .into_owned();
        }
        // Each side ends at a whole character, so the cut splits none.
        let head = whole(&self.head, |b, n| &b[..n]);
        let tail = whole(&self.tail, |b, n| &b[b.len() - n..]);
        format!("{head}\n[… {} bytes cut …]\n{tail}", self.dropped)
    }
}

/// `bytes` as text, less up to 3 bytes of a character split at the end `keep` takes them from.
fn whole(bytes: &[u8], keep: impl Fn(&[u8], usize) -> &[u8]) -> String {
    let n = (bytes.len().saturating_sub(3)..=bytes.len())
        .rev()
        .find(|&n| std::str::from_utf8(keep(bytes, n)).is_ok())
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(keep(bytes, n)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(text: &str) -> String {
        let mut capped = Capped::default();
        capped.push(text.as_bytes());
        capped.text()
    }

    #[test]
    fn an_edit_replaces_one_exact_match_and_refuses_the_rest() {
        assert_eq!(replace("a b a", "b", "c", false).unwrap(), "a c a");
        assert!(
            replace("a b", "x", "c", false)
                .unwrap_err()
                .contains("not in")
        );
        let twice = replace("a b a", "a", "c", false).unwrap_err();
        assert!(twice.contains("2 times"), "{twice}");
        assert_eq!(replace("a b a", "a", "c", true).unwrap(), "c b c");
    }

    /// The `READ` script run here by `sh`, as the box runs it.
    fn read_script(contents: &str, first: usize, last: usize) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f.txt");
        std::fs::write(&path, contents).unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", READ, "sh"])
            .arg(&path)
            .args([first.to_string(), last.to_string(), READ_MAX.to_string()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn a_read_numbers_whole_lines_in_the_box_within_its_budget() {
        let big: String = (1..=3000)
            .map(|i| format!("line {i:05} {}\n", "x".repeat(80)))
            .collect();
        let out = read_script(&big, 1, 2000);
        let (total, lines) = out.split_once('\n').unwrap();
        assert_eq!(total, "3000");
        assert!(lines.len() <= READ_MAX, "{}", lines.len());
        for (i, line) in lines.lines().enumerate() {
            assert_eq!(
                line,
                format!("{:>6}\tline {:05} {}", i + 1, i + 1, "x".repeat(80))
            );
        }
        let shown = lines.lines().count();
        assert!(shown > 300 && shown < 2000, "{shown}");
        let tail = format!("  3000\tline 03000 {}\n", "x".repeat(80));
        assert!(read_script(&big, 2999, 5000).ends_with(&tail));
        assert_eq!(
            read_script("a\nb", 1, 10),
            "2\n     1\ta\n     2\tb\n",
            "no final newline"
        );
        let long = read_script(&"y".repeat(100_000), 1, 10);
        assert!(
            long.len() < READ_MAX
                && long.contains("     1\tyyy")
                && long.ends_with("[long line 1]\n"),
            "{}",
            long.len()
        );
    }

    #[test]
    fn a_character_at_the_halfway_point_survives_and_the_cut_splits_none() {
        let whole_text = format!("{}é", "a".repeat(MAX_OUTPUT / 2 - 1));
        assert_eq!(cap(&whole_text), whole_text);
        let cut = cap(&format!(
            "{}é{}é",
            "a".repeat(MAX_OUTPUT / 2 - 1),
            "b".repeat(MAX_OUTPUT)
        ));
        assert!(!cut.contains('\u{fffd}'), "no replacement character");
        assert!(cut.ends_with("bé") && cut.contains("bytes cut"));
    }

    #[test]
    fn output_keeps_its_head_and_tail_however_it_streams() {
        let text = format!("{}{}", "h".repeat(MAX_OUTPUT), "t".repeat(MAX_OUTPUT));
        let mut streamed = Capped::default();
        for piece in text.as_bytes().chunks(7) {
            streamed.push(piece);
        }
        let capped = streamed.text();
        assert_eq!(capped, cap(&text));
        assert!(capped.len() <= MAX_OUTPUT + 40, "{}", capped.len());
        assert!(capped.starts_with("hhh") && capped.ends_with("ttt"));
        assert!(capped.contains(&format!("[… {MAX_OUTPUT} bytes cut …]")));
        assert_eq!(cap("short"), "short");
    }
}
