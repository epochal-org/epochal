//! What of a model response may reach a box held to `as_of`: only `content-type` among headers,
//! and every `created` / `created_at` epoch in the body rewritten to the as-of.

use std::mem;
use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};

const KEPT_HEADERS: [&str; 1] = ["content-type"];

static CREATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(created|created_at)"(\s*):(\s*)\d+"#).expect("valid regex"));

/// A body end that a later chunk could complete into a [`CREATED`] match.
static UNFINISHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#""(?:c(?:r(?:e(?:a(?:t(?:e(?:d(?:_(?:at?)?)?)?)?)?)?)?)?)?$|"created(?:_at)?"\s*(?::\s*\d*)?$"#,
    )
    .expect("valid regex")
});

pub fn headers<'a>(headers: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<(String, String)> {
    headers
        .filter(|(name, _)| KEPT_HEADERS.iter().any(|k| k.eq_ignore_ascii_case(name)))
        .map(|(n, v)| (n.to_owned(), v.to_owned()))
        .collect()
}

/// Rewrites a streamed body, holding back a tail that may be the start of an epoch match.
pub struct Scrub {
    as_of: String,
    pending: Vec<u8>,
}

impl Scrub {
    pub fn new(as_of_epoch: i64) -> Self {
        Scrub {
            as_of: as_of_epoch.to_string(),
            pending: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let hold = UNFINISHED
            .find(&self.pending)
            .map_or(self.pending.len(), |m| m.start());
        let rest = self.pending.split_off(hold);
        let done = mem::replace(&mut self.pending, rest);
        self.rewrite(&done)
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let rest = mem::take(&mut self.pending);
        self.rewrite(&rest)
    }

    fn rewrite(&self, data: &[u8]) -> Vec<u8> {
        CREATED
            .replace_all(data, |c: &Captures| {
                let mut out = Vec::new();
                out.push(b'"');
                out.extend_from_slice(&c[1]);
                out.push(b'"');
                out.extend_from_slice(&c[2]);
                out.push(b':');
                out.extend_from_slice(&c[3]);
                out.extend_from_slice(self.as_of.as_bytes());
                out
            })
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_content_type_survives() {
        let got = headers(
            [
                ("Content-Type", "text/event-stream"),
                ("Date", "Sat, 07 Nov 2026 00:06:53 GMT"),
                ("anthropic-ratelimit-tokens-reset", "2026-11-07T00:07:00Z"),
            ]
            .into_iter(),
        );
        assert_eq!(
            got,
            vec![("Content-Type".to_owned(), "text/event-stream".to_owned())]
        );
    }

    fn scrubbed(chunks: &[&[u8]]) -> String {
        let mut body = Scrub::new(42);
        let mut out: Vec<u8> = chunks.iter().flat_map(|c| body.push(c)).collect();
        out.extend(body.finish());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn epochs_are_pinned_wherever_the_stream_splits() {
        let body = "data: {\"id\":\"r\",\"created_at\" :\n 1794010015,\"x\":1}\n\ndata: {\"created\":1794010016}\n";
        let want = "data: {\"id\":\"r\",\"created_at\" :\n 42,\"x\":1}\n\ndata: {\"created\":42}\n";
        let bytes = body.as_bytes();
        for i in 0..bytes.len() {
            for j in i..bytes.len() {
                assert_eq!(
                    scrubbed(&[&bytes[..i], &bytes[i..j], &bytes[j..]]),
                    want,
                    "split at {i},{j}"
                );
            }
        }
    }

    #[test]
    fn unrelated_text_streams_through_and_lookalikes_are_kept() {
        let mut body = Scrub::new(42);
        assert_eq!(body.push(b"hello, "), b"hello, ");
        assert_eq!(
            scrubbed(&[br#"{"created_tokens":5,"created":"#, b"\"soon\"}"]),
            r#"{"created_tokens":5,"created":"soon"}"#
        );
    }
}
