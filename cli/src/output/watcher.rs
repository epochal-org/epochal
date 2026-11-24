//! What `run` shows of a session: its progress, its harness events drawn live, and both kept in the
//! trace when there is one.

use std::sync::{Arc, Mutex, PoisonError};

use super::events::Screen;
use super::progress::{Kind, Out};
use super::trace::{Shared, Trace};
use crate::session::{Harness, Watch};

pub struct Watcher<O: Out> {
    screen: Screen<O>,
    trace: Option<Shared>,
}

impl<O: Out> Watcher<O> {
    pub fn new(harness: Harness, out: O, trace: Option<Trace>) -> Self {
        Watcher {
            screen: Screen::new(harness, out),
            trace: trace.map(|t| Arc::new(Mutex::new(t))),
        }
    }

    /// The agent's last message: the run's answer.
    pub fn answer(&self) -> &str {
        self.screen.answer()
    }
}

impl<O: Out> Out for Watcher<O> {
    fn line(&mut self, kind: Kind, head: &str, detail: &str) {
        self.screen.out().line(kind, head, detail);
    }

    fn status(&mut self, label: &str, note: &str) {
        self.screen.out().status(label, note);
    }
}

impl<O: Out> Watch for Watcher<O> {
    fn event(&mut self, raw: &[u8]) {
        if let Some(trace) = &self.trace {
            trace
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .event(raw);
        }
        self.screen.event(raw);
    }

    fn trace(&self) -> Option<Shared> {
        self.trace.clone()
    }
}
