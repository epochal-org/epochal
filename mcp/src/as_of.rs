//! The instant the tools are held to, as a person names it.

use chrono::{DateTime, NaiveDate, Utc};

/// An instant as a person names it: RFC 3339, an HTTP-date, or a `YYYY-MM-DD` date, which means
/// the last second of that UTC day, as the API reads one. The error says which forms it takes.
pub fn parse(text: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(text)
        .or_else(|_| DateTime::parse_from_rfc2822(text))
        .map(|t| t.to_utc())
        .ok()
        .or_else(|| {
            let day = NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
            Some(day.and_hms_opt(23, 59, 59)?.and_utc())
        })
        .ok_or_else(|| "use RFC 3339, an HTTP-date or YYYY-MM-DD".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_names_the_same_instant() {
        let midnight = parse("2026-03-01T00:00:00Z").unwrap();
        for form in ["2026-03-01T05:30:00+05:30", "Sun, 01 Mar 2026 00:00:00 GMT"] {
            assert_eq!(parse(form), Ok(midnight), "{form}");
        }
        assert_eq!(
            parse("2026-03-01"),
            parse("2026-03-01T23:59:59Z"),
            "a bare date is the end of that day"
        );
        assert!(
            parse("Sat, 01 Mar 2026 00:00:00 GMT").is_err(),
            "wrong weekday"
        );
        assert!(parse("yesterday").is_err());
    }
}
