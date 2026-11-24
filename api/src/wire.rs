//! Encodings the API uses on the wire: durations as `90s` or `1h30m`, bytes as base64, ids as one
//! path segment.

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::de::Error as _;
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serializer};

use crate::Error;

/// Everything but RFC 3986's unreserved characters, so an id stays one path segment.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// `id` as one path segment; empty, `.` and `..` are refused, as URL parsing would drop them.
pub(crate) fn segment(id: &str) -> Result<String, Error> {
    if matches!(id, "" | "." | "..") {
        return Err(Error::InvalidArgument(format!("{id:?} is not an id")));
    }
    Ok(utf8_percent_encode(id, SEGMENT).to_string())
}

/// A duration as the API writes it: the largest unit that divides it exactly.
fn format_duration(d: Duration) -> Option<String> {
    let secs = d.as_secs();
    if secs == 0 || d.subsec_nanos() != 0 {
        return None;
    }
    Some(match secs {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    })
}

/// Whole `h`, `m` and `s`, in that order, each at most once: `"90s"`, `"1h30m"`; never zero.
fn parse_duration(text: &str) -> Option<Duration> {
    let mut rest = text;
    let mut secs: u64 = 0;
    for (unit, scale) in [('h', 3600), ('m', 60), ('s', 1)] {
        let Some((digits, after)) = rest.split_once(unit) else {
            continue;
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        secs = digits
            .parse::<u64>()
            .ok()?
            .checked_mul(scale)?
            .checked_add(secs)?;
        rest = after;
    }
    (rest.is_empty() && secs > 0).then(|| Duration::from_secs(secs))
}

fn unexpressible(d: Duration) -> String {
    format!("{d:?} is not a positive whole number of seconds")
}

/// A duration for a query string.
pub(crate) fn duration_param(d: Duration) -> Result<String, Error> {
    format_duration(d).ok_or_else(|| Error::InvalidArgument(unexpressible(d)))
}

/// A `Duration` or `Option<Duration>` field as the API writes it.
pub(crate) fn serialize_duration<S: Serializer>(
    d: &(impl Copy + Into<Option<Duration>>),
    s: S,
) -> Result<S::Ok, S::Error> {
    match (*d).into() {
        Some(d) => {
            s.serialize_str(&format_duration(d).ok_or_else(|| S::Error::custom(unexpressible(d)))?)
        }
        None => s.serialize_none(),
    }
}

pub(crate) fn deserialize_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
    let text = String::deserialize(d)?;
    parse_duration(&text).ok_or_else(|| D::Error::custom(format!("invalid duration {text:?}")))
}

pub(crate) mod bytes_b64 {
    use super::*;

    pub(crate) fn encode(bytes: &[u8]) -> String {
        STANDARD.encode(bytes)
    }

    pub(crate) fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        STANDARD
            .decode(text)
            .map_err(|e| D::Error::custom(format!("bad base64: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_use_the_largest_exact_unit_and_read_back() {
        for (secs, text) in [(90, "90s"), (600, "10m"), (5400, "90m"), (7200, "2h")] {
            let d = Duration::from_secs(secs);
            assert_eq!(format_duration(d).as_deref(), Some(text));
            assert_eq!(parse_duration(text), Some(d));
        }
        assert_eq!(format_duration(Duration::ZERO), None);
        assert_eq!(format_duration(Duration::from_millis(1500)), None);
        for (text, secs) in [("1h30m", 5400), ("2m30s", 150), ("1h0m1s", 3601)] {
            assert_eq!(
                parse_duration(text),
                Some(Duration::from_secs(secs)),
                "{text}"
            );
        }
        for bad in [
            "", "s", "0s", "0h0m0s", "1d", "-1m", "1.5h", "30m1h", "1s1m", "1h1h", "1h30", "1h 30m",
        ] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_id_stays_one_path_segment() {
        assert_eq!(segment("sbx_7f3k-9.q~").unwrap(), "sbx_7f3k-9.q~");
        assert_eq!(segment("a/b?c#d e").unwrap(), "a%2Fb%3Fc%23d%20e");
        assert_eq!(segment("..x").unwrap(), "..x");
        for dropped in ["", ".", ".."] {
            assert!(segment(dropped).is_err(), "{dropped:?}");
        }
    }
}
