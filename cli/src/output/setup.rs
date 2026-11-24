//! Getting the box ready, as one group: a spinner names the step under way, and once the box is
//! ready (or isn't) its steps are printed together under one line.

use std::time::Instant;

use crate::output::progress::{Kind, Out, secs};

pub struct Setup {
    t0: Instant,
    done: Vec<(String, String)>,
    doing: String,
    /// Shown beside every later step, so a box left behind can still be found.
    id: Option<String>,
}

impl Setup {
    pub fn new() -> Self {
        Setup {
            t0: Instant::now(),
            done: Vec::new(),
            doing: String::new(),
            id: None,
        }
    }

    /// The step under way, e.g. "Creating sandbox".
    pub fn now(&mut self, out: &mut impl Out, doing: &str) {
        doing.clone_into(&mut self.doing);
        let note = self
            .id
            .as_ref()
            .map_or(String::new(), |id| format!(" · {id}"));
        out.status(doing, &note);
    }

    pub fn created(&mut self, id: &str) {
        self.id = Some(id.to_owned());
    }

    pub fn step(&mut self, head: impl Into<String>, detail: impl Into<String>) {
        self.done.push((head.into(), detail.into()));
    }

    pub fn ready(self, out: &mut impl Out) {
        out.line(
            Kind::Step,
            "Sandbox ready",
            &format!("· {}", secs(self.t0.elapsed())),
        );
        let last = self.done.len().saturating_sub(1);
        for (i, (head, detail)) in self.done.iter().enumerate() {
            let kind = if i == last {
                Kind::Result
            } else {
                Kind::Branch
            };
            out.line(kind, head, detail);
        }
    }

    /// The steps that finished, then the one under way, marked failed.
    pub fn failed(self, out: &mut impl Out) {
        out.line(
            Kind::Stopped,
            "Sandbox not ready",
            &format!("· {}", secs(self.t0.elapsed())),
        );
        for (head, detail) in &self.done {
            out.line(Kind::Branch, head, detail);
        }
        out.line(Kind::StoppedAt, &self.doing, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::progress::plain;

    #[derive(Default)]
    struct Record(Vec<String>);

    impl Out for Record {
        fn line(&mut self, kind: Kind, head: &str, detail: &str) {
            self.0.push(plain(kind, head, detail));
        }
        fn status(&mut self, label: &str, note: &str) {
            self.0.push(format!("~ {label}{note}"));
        }
    }

    fn steps(setup: &mut Setup, out: &mut Record) {
        setup.now(out, "Creating sandbox at 2025-03-01");
        setup.created("sbx_1");
        setup.step("Created sbx_1", "· as of 2025-03-01T23:59:59Z · 1s");
        setup.step("Installed claude-code", "2.1.300");
        setup.step("Installed relay", "5eaf314f98");
        setup.now(out, "Connecting model and web tools");
    }

    fn lines(out: &Record) -> Vec<&str> {
        out.0
            .iter()
            .filter(|l| !l.starts_with('~'))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn a_ready_box_shows_its_steps_as_one_group() {
        let (mut setup, mut out) = (Setup::new(), Record::default());
        steps(&mut setup, &mut out);
        setup.step("Connected model and web tools", "· 0s");
        setup.ready(&mut out);
        assert_eq!(
            lines(&out),
            [
                "✓ Sandbox ready · 0s",
                "  ├ Created sbx_1 · as of 2025-03-01T23:59:59Z · 1s",
                "  ├ Installed claude-code 2.1.300",
                "  ├ Installed relay 5eaf314f98",
                "  └ Connected model and web tools · 0s",
            ]
        );
    }

    #[test]
    fn a_box_that_fails_shows_where_it_stopped() {
        let (mut setup, mut out) = (Setup::new(), Record::default());
        steps(&mut setup, &mut out);
        setup.failed(&mut out);
        assert_eq!(
            lines(&out),
            [
                "✗ Sandbox not ready · 0s",
                "  ├ Created sbx_1 · as of 2025-03-01T23:59:59Z · 1s",
                "  ├ Installed claude-code 2.1.300",
                "  ├ Installed relay 5eaf314f98",
                "  └ ✗ Connecting model and web tools",
            ]
        );
    }

    #[test]
    fn a_box_that_fails_to_be_created_shows_only_that() {
        let (mut setup, mut out) = (Setup::new(), Record::default());
        setup.now(&mut out, "Creating sandbox");
        setup.failed(&mut out);
        assert_eq!(
            lines(&out),
            ["✗ Sandbox not ready · 0s", "  └ ✗ Creating sandbox"]
        );
    }

    #[test]
    fn one_step_closes_the_group_at_once() {
        let (mut setup, mut out) = (Setup::new(), Record::default());
        setup.step("Created sbx_1", "");
        setup.ready(&mut out);
        assert_eq!(lines(&out), ["✓ Sandbox ready · 0s", "  └ Created sbx_1"]);
    }

    #[test]
    fn the_spinner_names_each_step_as_given_with_the_box_once_there_is_one() {
        let (mut setup, mut out) = (Setup::new(), Record::default());
        setup.now(&mut out, "Creating sandbox at 2025-03-01");
        setup.created("sbx_1");
        setup.now(&mut out, "Uploading /Users/Ann/MyRepo");
        assert_eq!(
            out.0,
            [
                "~ Creating sandbox at 2025-03-01",
                "~ Uploading /Users/Ann/MyRepo · sbx_1",
            ]
        );
    }
}
