//! Progress on stderr: finished lines scroll up, and a spinner below says what is happening now.
//! Without a terminal, only the lines are written, unstyled and whole; on one, each is cut to fit.

use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use console::{Term as Console, truncate_str};
use indicatif::{ProgressBar, ProgressStyle};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A setup step that finished.
    Step,
    /// A step in a group, with more after it; the last is a `Result`.
    Branch,
    /// A group that did not finish.
    Stopped,
    /// The step it stopped at, last in the group.
    StoppedAt,
    Tool,
    /// A tool's outcome, under its call.
    Result,
    Failed,
    Thought,
    /// Something the agent said along the way.
    Message,
    /// One entry of the agent's plan, `head` its checkbox.
    Plan,
}

impl Kind {
    fn glyph(self) -> &'static str {
        match self {
            Kind::Step => "✓",
            Kind::Branch => "  ├",
            Kind::Stopped => "✗",
            Kind::StoppedAt => "  └ ✗",
            Kind::Tool => "●",
            Kind::Result => "  └",
            Kind::Failed => "  ✗",
            Kind::Thought => "✻",
            Kind::Message => "▸",
            Kind::Plan => "   ",
        }
    }
}

pub trait Out {
    fn line(&mut self, kind: Kind, head: &str, detail: &str);
    /// What is happening now; the clock restarts when `label` changes.
    fn status(&mut self, label: &str, note: &str);
}

pub fn plain(kind: Kind, head: &str, detail: &str) -> String {
    let text = [kind.glyph(), head, detail]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    text.trim_end().to_owned()
}

fn styled(kind: Kind, head: &str, detail: &str) -> String {
    let (glyph, head_style, detail_style) = match kind {
        Kind::Step => ("32", "1", "2"),
        Kind::Tool => ("36", "1", ""),
        Kind::Branch | Kind::Result | Kind::Plan => ("2", "2", "2"),
        Kind::Stopped => ("31", "1", "2"),
        Kind::Failed | Kind::StoppedAt => ("31", "31", "31"),
        Kind::Thought => ("35", "2;3", "2;3"),
        Kind::Message => ("2", "", ""),
    };
    let paint = |style: &str, s: &str| match (style, s) {
        (_, "") => String::new(),
        ("", s) => s.to_owned(),
        (style, s) => format!("\x1b[{style}m{s}\x1b[0m"),
    };
    [
        paint(glyph, kind.glyph()),
        paint(head_style, head),
        paint(detail_style, detail),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
}

#[derive(Clone)]
pub struct Term {
    bar: Option<ProgressBar>,
    label: Arc<Mutex<String>>,
}

impl Term {
    pub fn stderr() -> Self {
        let bar = io::stderr().is_terminal().then(|| {
            let bar = ProgressBar::new_spinner();
            let style = ProgressStyle::with_template("{spinner:.cyan} {wide_msg} {elapsed:.dim}")
                .expect("a valid template")
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]);
            bar.set_style(style);
            bar.enable_steady_tick(Duration::from_millis(100));
            bar
        });
        Term {
            bar,
            label: Arc::default(),
        }
    }

    pub fn clear(&self) {
        if let Some(bar) = &self.bar {
            bar.finish_and_clear();
        }
    }
}

impl Out for Term {
    fn line(&mut self, kind: Kind, head: &str, detail: &str) {
        match &self.bar {
            Some(bar) => {
                let (_, cols) = Console::stderr().size();
                bar.println(fit(&styled(kind, head, detail), cols.into()));
            }
            None => {
                let _ = writeln!(io::stderr(), "{}", plain(kind, head, detail));
            }
        }
    }

    fn status(&mut self, label: &str, note: &str) {
        let Some(bar) = &self.bar else { return };
        let mut current = self.label.lock().unwrap_or_else(PoisonError::into_inner);
        if *current != label {
            label.clone_into(&mut current);
            bar.reset_elapsed();
        }
        bar.set_message(format!("{label}{note}"));
    }
}

/// `line` cut to `cols` terminal columns, its styling kept: wide characters count as two.
fn fit(line: &str, cols: usize) -> String {
    truncate_str(line, cols, "…").into_owned()
}

pub fn secs(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s}s"),
        s => format!("{}m {:02}s", s / 60, s % 60),
    }
}

#[cfg(test)]
mod tests {
    use console::{measure_text_width, strip_ansi_codes};

    use super::*;

    #[test]
    fn a_line_is_cut_to_the_columns_it_has_and_keeps_its_colour() {
        let line = styled(Kind::Failed, "ls: cannot access '/nope'", "");
        let cut = fit(&line, 12);
        assert_eq!(strip_ansi_codes(&cut), "  ✗ ls: can…");
        assert!(cut.starts_with("\x1b[31m  ✗\x1b[0m \x1b[31mls"), "{cut:?}");
        assert!(
            cut.ends_with("\x1b[0m"),
            "the red leaks past the cut: {cut:?}"
        );
        assert_eq!(fit(&line, 80), line);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let cut = fit(&plain(Kind::Message, "東京の天気は晴れです", ""), 9);
        assert_eq!(cut, "▸ 東京の…");
        assert!(measure_text_width(&cut) <= 9);
    }
}
