use super::*;
use crate::output::progress::plain;

/// What a terminal would keep: each line, and each status as `~ label note`.
#[derive(Default)]
struct Record(Vec<String>);

impl Out for &mut Record {
    fn line(&mut self, kind: Kind, head: &str, detail: &str) {
        self.0.push(plain(kind, head, detail));
    }
    fn status(&mut self, label: &str, note: &str) {
        self.0.push(format!("~ {label}{note}"));
    }
}

fn play(harness: Harness, events: &str) -> (Vec<String>, String) {
    let mut rec = Record::default();
    let mut screen = Screen::new(harness, &mut rec);
    for line in events.lines() {
        screen.event(line.as_bytes());
    }
    let answer = screen.answer().to_owned();
    (rec.0, answer)
}

fn lines(rec: &[String]) -> Vec<&str> {
    rec.iter()
        .filter(|l| !l.starts_with('~'))
        .map(String::as_str)
        .collect()
}

/// Recorded on staging at as-of 2025-03-01, outputs trimmed: search, fetch, shell, read, edit.
#[test]
fn a_claude_run_reads_as_steps() {
    let (rec, answer) = play(
        Harness::Claude,
        include_str!("../../testdata/claude_run.jsonl"),
    );
    assert_eq!(
        lines(&rec),
        [
            "✻ Thought",
            "▸ **Todo list** 1. Search the web for the ECB's most recent rate decision. 2. Fetch the best source. 3. Write a 3-line summary to /work/ecb.md with a shell command. 4. Read the file back. 5. Edit the file to add a title line. Step 1 is the search.",
            "● Search \"ECB monetary policy decision interest rates latest\"",
            "  └ 8 results",
            "✻ Thought (~201 tokens)",
            "▸ The most recent decision is 30 January 2025. The next meeting is 6 March, which hasn't happened yet. The ECB's own statement is the best source, so I'll fetch that.",
            "● Fetch https://www.ecb.europa.eu/press/press_conference/monetary-policy-statement/2025/html/ecb.is250130~1f418aa0f4.en.html",
            "  └ Monetary policy statement (with Q&A)",
            "● Run cat > /work/ecb.md <<'EOF' On 30 January 2025 the ECB cut its three key rates by 25bp, lowering the deposit facility rate to 2.75% (main refinancing 2.90%, marginal lending 3.15%) from 5 February. It was the fourth consecutive cut, decided unanimously; the ECB says disinflation is \"well on track\" and policy remains restrictive, with growth set to stay weak near term. Guidance stays data-dependent and meeting-by-meeting with no pre-commitment to a rate path; the next meeting is 6 March 2025. EOF …",
            "  └ On 30 January 2025 the ECB cut its three key rates by 25bp, lowering the deposit facility rate to 2.75% (main refinancing 2.90%, marginal lending 3.15%) from 5 February. (+2 lines)",
            "✻ Thought",
            "▸ The Edit tool needs the file read with the Read tool first, so I'll do that.",
            "● Read /work/ecb.md",
            "  └ 3 lines",
            "● Edit /work/ecb.md",
        ],
    );
    assert!(answer.starts_with("All five steps are done"), "{answer}");
    assert!(
        rec.contains(&"~ Thinking · ~201 tokens".to_owned()),
        "{rec:#?}"
    );
    assert!(rec.contains(
        &"~ Searching \"ECB monetary policy decision interest rates latest\"".to_owned()
    ));
    let all = rec.join("\n");
    for gone in ["turns", "$0.", "cost", "usage"] {
        assert!(!all.contains(gone), "{gone} in {all}");
    }
}

/// Recorded on staging at as-of 2025-03-12 with gpt-6-sol, outputs trimmed: write, a search and
/// a fetch between the runs, edit, run, read.
#[test]
fn a_builtin_run_reads_as_steps() {
    let (rec, answer) = play(
        Harness::Claude,
        include_str!("../../testdata/builtin_run.jsonl"),
    );
    assert_eq!(
        lines(&rec),
        [
            "● Write /home/epochal/q.py",
            "● Search \"Federal Reserve latest FOMC rate decision March 2025 January 29 2025 target range federal funds rate official\"",
            "  └ 5 results",
            "● Run python /home/epochal/q.py",
            "  └ 55",
            "● Fetch https://www.federalreserve.gov/newsevents/pressreleases/monetary20250129a.htm",
            "  └ Federal Reserve issues FOMC statement",
            "● Edit /home/epochal/q.py",
            "● Run python /home/epochal/q.py",
            "  └ 144",
            "● Read /home/epochal/q.py",
            "  └ 4 lines",
        ],
    );
    assert!(
        answer.starts_with("Created `/home/epochal/q.py`"),
        "{answer}"
    );
}

#[test]
fn a_codex_run_reads_as_steps() {
    let (rec, answer) = play(
        Harness::Codex,
        include_str!("../../testdata/codex_run.jsonl"),
    );
    assert_eq!(
        lines(&rec),
        [
            "▸ Todo: 1. Find the ECB’s latest rate decision with web search. 2. Open the ECB’s announcement and write a three-line summary to `/work/ecb.md`. 3. Read the file back, then add a title line.",
            "● Search \"site:ecb.europa.eu press monetary policy decisions latest ECB deposit facility rate March 2025 2026\"",
            "  └ 8 results",
            "● Fetch https://www.ecb.europa.eu/press/pr/date/2025/html/ecb.mp250130~530b29e622.en.html",
            "  └ Monetary policy decisions",
            "▸ The ECB’s 30 January 2025 press release is the latest rate decision available as of 2 March 2025. It confirms a 25 basis point cut to all three policy rates, effective 5 February. I’m writing the summary now.",
            "● Run printf '%s\\n' 'On 30 January 2025, the ECB cut all three key interest rates by 25 basis points.' 'From 5 February, the deposit, main refinancing and marginal lending rates are 2.75%, 2.90% and 3.15%.' 'The ECB expects inflation to return to its 2% target in 2025 and will decide future rates meeting by meeting. Source: https://www.ecb.europa.eu/press/pr/date/2025/html/ecb.mp250130~530b29e622.en.html' > /work/ecb.md",
            "● Run cat /work/ecb.md",
            "  └ On 30 January 2025, the ECB cut all three key interest rates by 25 basis points. (+2 lines)",
            "● Run sed -i '1i# ECB rate decision — 30 January 2025' /work/ecb.md",
            "● Run cat /work/ecb.md",
            "  └ # ECB rate decision — 30 January 2025 (+3 lines)",
        ],
    );
    assert!(
        answer.starts_with("I found the ECB’s latest rate decision"),
        "{answer}"
    );
    assert_eq!(rec[0], "~ Thinking");
    let all = rec.join("\n");
    assert!(!all.contains("tokens") && !all.contains("thread"), "{all}");
}

/// grok 1.0.77's `streaming-messages-json`, launched as `run` launches it, through the relay's
/// router to a scripted Responses endpoint and the time portal's search.
#[test]
fn a_grok_run_reads_as_steps() {
    let (rec, answer) = play(Harness::Grok, include_str!("../../testdata/grok_run.jsonl"));
    assert_eq!(
        lines(&rec),
        ["● Search \"nvidia quarterly revenue\"", "  └ 1 result"]
    );
    assert_eq!(answer, "Nvidia's Q4 FY25 revenue was $39.3B.");
}

/// grok 1.0.77 running a shell command, against a scripted Responses endpoint.
#[test]
fn a_grok_command_reads_as_its_own_output() {
    let (rec, answer) = play(
        Harness::Grok,
        include_str!("../../testdata/grok_shell.jsonl"),
    );
    assert_eq!(lines(&rec), ["● Run echo hi", "  └ hi"]);
    assert_eq!(answer, "done: hi");
}

#[test]
fn a_grok_mcp_error_fails() {
    let (rec, _) = play(
        Harness::Grok,
        concat!(
            r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"epochal__web_fetch","input":{"url":"https://a.example"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":false,"content":"{\"type\":\"MCP\",\"output\":{\"Error\":\"not archived\"}}"}]}}"#,
        ),
    );
    assert_eq!(
        lines(&rec),
        ["● Fetch https://a.example", "  ✗ not archived"]
    );
}

#[test]
fn a_claude_command_that_prints_a_grok_record_is_shown_as_printed() {
    let (rec, _) = play(
        Harness::Claude,
        concat!(
            r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"cat r.json"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":false,"content":"{\"type\":\"Bash\",\"output\":[104,105],\"exit_code\":1}"}]}}"#,
        ),
    );
    assert_eq!(
        lines(&rec),
        [
            "● Run cat r.json",
            r#"  └ {"type":"Bash","output":[104,105],"exit_code":1}"#
        ]
    );
}

#[test]
fn a_grok_command_that_exits_nonzero_fails() {
    let (rec, _) = play(
        Harness::Grok,
        concat!(
            r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"run_terminal_command","input":{"command":"false"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":false,"content":"{\"type\":\"Bash\",\"output\":[110,111,10],\"exit_code\":1}"}]}}"#,
        ),
    );
    assert_eq!(lines(&rec), ["● Run false", "  ✗ no"]);
}

#[test]
fn failures_and_plans_show_under_their_call() {
    let (rec, _) = play(
        Harness::Codex,
        concat!(
            r#"{"type":"item.started","item":{"id":"a","type":"command_execution","command":"/bin/bash -lc 'ls /nope'"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"a","type":"command_execution","aggregated_output":"ls: cannot access '/nope': No such file or directory\n","exit_code":2}}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"b","type":"mcp_tool_call","server":"epochal","tool":"web_fetch","arguments":{"url":"https://a.example"}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"b","type":"mcp_tool_call","error":{"message":"fetch failed: not archived"}}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"c","type":"todo_list","items":[{"text":"search","completed":true},{"text":"write","completed":false}]}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"d","type":"file_change","changes":[{"path":"/work/fed.md","kind":"add"},{"path":"/work/old.md","kind":"delete"},{"path":"/work/a.md","kind":"update"}]}}"#,
        ),
    );
    assert_eq!(
        lines(&rec),
        [
            "● Run ls /nope",
            "  ✗ ls: cannot access '/nope': No such file or directory",
            "● Fetch https://a.example",
            "  ✗ fetch failed: not archived",
            "● Plan",
            "    ☒ search",
            "    ☐ write",
            "● Write /work/fed.md",
            "● Delete /work/old.md",
            "● Edit /work/a.md",
        ]
    );
}

#[test]
fn only_the_last_message_is_the_answer() {
    let msg = |id: &str, t: &str| {
        format!(
            r#"{{"type":"item.completed","item":{{"id":"{id}","type":"agent_message","text":{t:?}}}}}"#
        )
    };
    let events = [msg("i0", "Checking."), msg("i1", "## Held"), "".into()].join("\n");
    let (rec, answer) = play(Harness::Codex, &events);
    assert_eq!(lines(&rec), ["▸ Checking."]);
    assert_eq!(answer, "## Held");

    let part = |t: &str| {
        format!(
            r#"{{"type":"assistant","message":{{"id":"m9","content":[{{"type":"text","text":{t:?}}}]}}}}"#
        )
    };
    let (rec, answer) = play(
        Harness::Claude,
        &[part("## Held"), part("4.25%")].join("\n"),
    );
    assert!(lines(&rec).is_empty(), "{rec:?}");
    assert_eq!(answer, "## Held\n\n4.25%");
}

#[test]
fn codexs_shell_wrapper_is_peeled() {
    assert_eq!(
        unwrap_shell("/bin/bash -lc 'cat /work/a.md'"),
        "cat /work/a.md"
    );
    assert_eq!(
        unwrap_shell(r#"/bin/bash -lc "printf '%s\\n' \"a\"""#),
        r#"printf '%s\n' "a""#
    );
    assert_eq!(unwrap_shell("python3 x.py"), "python3 x.py");
    // Single quotes: backslashes are literal; a quote inside is closed, escaped, reopened.
    assert_eq!(
        unwrap_shell(r#"/bin/bash -lc 'printf "a\"b\\n" && echo '\''hi'\'''"#),
        r#"printf "a\"b\\n" && echo 'hi'"#
    );
    // Double quotes: \$ \` \" \\ are escapes; any other backslash stays.
    assert_eq!(
        unwrap_shell(r#"/bin/bash -lc "echo \$HOME \"x\" a\\b c\d""#),
        r#"echo $HOME "x" a\b c\d"#
    );
}

#[test]
fn a_read_counts_blank_lines_but_not_the_one_after_the_last_newline() {
    let read = |content: &str| {
        let call = r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"Read","input":{"file_path":"/a"}}]}}"#;
        let result = format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"t","content":{content:?}}}]}}}}"#
        );
        let (rec, _) = play(Harness::Claude, &format!("{call}\n{result}"));
        lines(&rec)[1].to_owned()
    };
    assert_eq!(read("1\tfirst\n2\t\n3\tthird\n4\t"), "  └ 3 lines");
    assert_eq!(read("1\tonly"), "  └ 1 lines");
}

#[test]
fn a_result_after_another_call_names_its_own() {
    let start = |id: &str, q: &str| {
        format!(
            r#"{{"type":"item.started","item":{{"id":"{id}","type":"mcp_tool_call","server":"epochal","tool":"web_search","arguments":{{"query":"{q}"}}}}}}"#
        )
    };
    let end = |id: &str, n: usize| {
        let text: String = (1..=n)
            .map(|i| format!("{i}. t\n   https://a.example/{i}\n"))
            .collect();
        format!(
            r#"{{"type":"item.completed","item":{{"id":"{id}","type":"mcp_tool_call","result":{{"content":[{{"type":"text","text":{text:?}}}]}},"error":null}}}}"#
        )
    };
    let (rec, _) = play(
        Harness::Codex,
        &[
            start("a", "fed"),
            start("b", "ecb"),
            end("a", 2),
            end("b", 3),
            start("c", "boj"),
            end("c", 1),
        ]
        .join("\n"),
    );
    assert_eq!(
        lines(&rec),
        [
            "● Search \"fed\"",
            "● Search \"ecb\"",
            "  └ Search \"fed\" · 2 results",
            "  └ Search \"ecb\" · 3 results",
            "● Search \"boj\"",
            "  └ 1 result",
        ]
    );
}

#[test]
fn after_a_plan_and_a_command_the_agent_is_thinking_again() {
    let (rec, _) = play(
        Harness::Codex,
        concat!(
            r#"{"type":"item.updated","item":{"id":"p","type":"todo_list","items":[{"text":"search","completed":false}]}}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"a","type":"command_execution","command":"date"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"a","type":"command_execution","aggregated_output":"","exit_code":0}}"#,
        ),
    );
    assert_eq!(
        rec.last().map(String::as_str),
        Some("~ Thinking"),
        "{rec:#?}"
    );
}

#[test]
fn a_silent_command_shows_no_result() {
    let (rec, _) = play(
        Harness::Claude,
        concat!(
            r#"{"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":"t","name":"Bash","input":{"command":"echo ok > /work/out.md"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":"(Bash completed with no output)"}]}}"#,
        ),
    );
    assert_eq!(lines(&rec), ["● Run echo ok > /work/out.md"]);
}
