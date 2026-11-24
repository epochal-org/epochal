//! A byte stream split into lines, whatever its chunking: a harness's events, or a model's SSE.

use std::mem;

use anyhow::{Result, bail};

/// The longest unfinished line kept; past it the stream is refused rather than buffered.
const MAX_LINE: usize = 16 << 20;

#[derive(Default)]
pub struct Lines {
    pending: Vec<u8>,
}

impl Lines {
    /// The non-empty lines `data` finishes. An unfinished line over [`MAX_LINE`] is an error and
    /// is dropped.
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>> {
        let start = self.pending.len();
        self.pending.extend_from_slice(data);
        let done = match data.iter().rposition(|b| *b == b'\n') {
            Some(end) => {
                let rest = self.pending.split_off(start + end + 1);
                mem::replace(&mut self.pending, rest)
            }
            None => Vec::new(),
        };
        if self.pending.len() > MAX_LINE {
            self.pending = Vec::new();
            bail!("a line ran past {} MiB without ending", MAX_LINE >> 20);
        }
        Ok(done
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .map(<[u8]>::to_vec)
            .collect())
    }

    pub fn finish(&mut self) -> Option<Vec<u8>> {
        let rest = mem::take(&mut self.pending);
        (!rest.is_empty()).then_some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_survive_arbitrary_splits() {
        let mut lines = Lines::default();
        assert!(lines.push(b"{\"a\":").unwrap().is_empty());
        assert_eq!(lines.push(b"1}\n{\"b\"").unwrap(), [b"{\"a\":1}"]);
        assert_eq!(
            lines.push(b":2}\n\n{\"c\":3}\n").unwrap(),
            [b"{\"b\":2}", b"{\"c\":3}"]
        );
        assert_eq!(lines.finish(), None);
        lines.push(b"tail").unwrap();
        assert_eq!(lines.finish(), Some(b"tail".to_vec()));
    }

    #[test]
    fn a_line_that_never_ends_is_refused_and_the_stream_goes_on() {
        let mut lines = Lines::default();
        let chunk = vec![b'x'; 1 << 20];
        for _ in 0..16 {
            lines.push(&chunk).unwrap();
        }
        assert!(lines.push(b"x").is_err());
        assert_eq!(lines.push(b"ok\n").unwrap(), [b"ok"]);
    }
}
