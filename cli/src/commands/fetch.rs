//! `fetch`: a page as it stood at `--as-of`.

use anyhow::Result;
use clap::Args as ClapArgs;
use epochal_api::Client;
use epochal_api::time_portal::web::{FetchRequest, FetchResponse, PageStatus};
use epochal_mcp::text;
use serde::Serialize;

use super::{Command, Done};
use crate::cli::AsOfArgs;
use crate::output::Report;
use crate::output::trace::Trace;

const OP: &str = "fetch";
/// The exit code when the request was answered but the page could not be read.
const UNREAD: i32 = 1;

#[derive(ClapArgs)]
pub struct Args {
    url: String,
    #[command(flatten)]
    as_of: AsOfArgs,
    /// The most characters of content, 1 to 1,000,000
    #[arg(long, default_value_t = 20_000, value_parser = clap::value_parser!(u32).range(1..=1_000_000))]
    max_characters: u32,
}

#[derive(Serialize)]
#[serde(transparent)]
pub struct Fetched(FetchResponse);

impl Command for Args {
    type Report = Fetched;

    async fn run(self, trace: Option<Trace>) -> Result<Done<Fetched>> {
        let mut request = FetchRequest::new(self.url);
        request.max_characters = Some(self.max_characters);
        let page = Client::builder()
            .build()?
            .time_portal(self.as_of.as_of)
            .fetch(&request)
            .await?;
        if let Some(mut trace) = trace {
            trace.answer(OP, &page);
        }
        let code = match page.status {
            PageStatus::Success => 0,
            _ => UNREAD,
        };
        Ok(Done {
            report: Fetched(page),
            code,
        })
    }
}

impl Report for Fetched {
    fn text(&self) -> String {
        text::fetch(&self.0, true).unwrap_or_else(|unread| unread)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn fetched() -> Fetched {
        Fetched(
            serde_json::from_value(json!({"as_of": "2025-03-01T23:59:59Z", "request_id": "r",
                "url": "https://a.example/x", "final_url": "https://a.example/y",
                "title": "Q4 *results*", "published_at": "2025-02-26",
                "content": "Revenue rose.\n",
                "truncated": true}))
            .unwrap(),
        )
    }

    #[test]
    fn text_of_a_page_that_could_not_be_read_is_the_sites_answer() {
        let unread: FetchResponse = serde_json::from_value(json!({"as_of": "2025-03-01T23:59:59Z",
            "request_id": "r", "status": "error",
            "page_error": {"http_status_code": null, "message": "Site unreachable."},
            "url": "https://a.example/x", "final_url": "https://a.example/x",
            "title": "", "content": "", "truncated": false}))
        .unwrap();
        assert_eq!(
            Fetched(unread).text(),
            "could not read https://a.example/x: Site unreachable."
        );
    }

    #[test]
    fn a_status_from_a_newer_server_still_writes_as_json() {
        let newer: FetchResponse = serde_json::from_value(json!({"as_of": "2025-03-01T23:59:59Z",
            "request_id": "r", "status": "partial", "url": "https://a.example/x",
            "final_url": "https://a.example/x", "title": "", "content": "", "truncated": false}))
        .unwrap();
        let written = serde_json::to_value(Fetched(newer)).unwrap();
        assert_eq!(written["status"], "unknown");
    }

    #[test]
    fn text_is_the_read_page_with_its_published_date() {
        assert_eq!(
            fetched().text(),
            "Q4 *results*\nhttps://a.example/y\npublished 2025-02-26\n\nRevenue rose.\n\n[truncated]"
        );
    }
}
