//! Answers as text. The agent's carry no timestamps at all: a date it sees could lie past the
//! instant it believes is today, or tell it what today is. `dated` adds each published date, for a
//! reader who is not held to the instant.

use std::fmt::Write;

use epochal_api::time_portal::web::{FetchResponse, PageStatus, SearchResponse};

/// Each result numbered, with its title, URL, published date if `dated`, and snippet.
pub fn search(reply: &SearchResponse, dated: bool) -> String {
    let mut out = String::new();
    for (i, hit) in reply.results.iter().enumerate() {
        let _ = writeln!(out, "{}. {}\n   {}", i + 1, hit.title, hit.url);
        if let Some(when) = hit.published_at.as_ref().filter(|_| dated) {
            let _ = writeln!(out, "   published {when}");
        }
        let _ = writeln!(out, "   {}\n", hit.snippet.trim());
    }
    if out.is_empty() {
        "No results.".to_owned()
    } else {
        out
    }
}

/// The page's title and URL, its published date if `dated`, then its content; `Err` with the
/// site's answer when it could not be read.
pub fn fetch(page: &FetchResponse, dated: bool) -> Result<String, String> {
    if page.status != PageStatus::Success {
        let mut out = format!("could not read {}", page.url);
        if let Some(error) = &page.page_error {
            let _ = write!(out, ": {error}");
        }
        return Err(out);
    }
    let mut out = format!("{}\n{}\n", page.title, page.final_url);
    if let Some(when) = page.published_at.as_ref().filter(|_| dated) {
        let _ = writeln!(out, "published {when}");
    }
    let _ = write!(out, "\n{}", page.content);
    if page.truncated {
        out.push_str("\n[truncated]");
    }
    Ok(out)
}
