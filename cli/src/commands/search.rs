//! `search`: the web as it stood at `--as-of`.

use anyhow::Result;
use clap::Args as ClapArgs;
use epochal_api::Client;
use epochal_api::time_portal::web::{SearchRequest, SearchResponse};
use epochal_mcp::text;
use serde::Serialize;

use super::{Command, Done};
use crate::cli::AsOfArgs;
use crate::output::Report;
use crate::output::trace::Trace;

const OP: &str = "search";

#[derive(ClapArgs)]
pub struct Args {
    query: String,
    #[command(flatten)]
    as_of: AsOfArgs,
    /// The most results, 1 to 10
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=10))]
    num_results: u32,
    /// Only pages from these sites, comma-separated
    #[arg(long, value_delimiter = ',')]
    domains: Vec<String>,
}

#[derive(Serialize)]
pub struct Found {
    query: String,
    #[serde(flatten)]
    response: SearchResponse,
}

impl Command for Args {
    type Report = Found;

    async fn run(self, trace: Option<Trace>) -> Result<Done<Found>> {
        let mut request = SearchRequest::new(&self.query);
        request.num_results = Some(self.num_results);
        request.domains = self.domains;
        let response = Client::builder()
            .build()?
            .time_portal(self.as_of.as_of)
            .search(&request)
            .await?;
        if let Some(mut trace) = trace {
            trace.answer(OP, &response);
        }
        Ok(Done::ok(Found {
            query: self.query,
            response,
        }))
    }
}

impl Report for Found {
    fn text(&self) -> String {
        text::search(&self.response, true)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn found(results: Value) -> Found {
        Found {
            query: "fed rate".into(),
            response: serde_json::from_value(
                json!({"as_of": "2025-03-01T23:59:59Z", "request_id": "r", "results": results, "entities": []}),
            )
            .unwrap(),
        }
    }

    #[test]
    fn each_result_is_numbered_with_its_url_date_and_snippet() {
        let found = found(
            json!([{"title": "Fed [holds] *rates*", "url": "https://a.example/x",
            "published_at": "2025-02-26", "snippet": "  the\nFed  held  "}]),
        );
        assert_eq!(
            found.text(),
            "1. Fed [holds] *rates*\n   https://a.example/x\n   published 2025-02-26\n   the\nFed  held\n\n"
        );
    }

    #[test]
    fn no_results_is_said() {
        assert_eq!(found(json!([])).text(), "No results.");
    }

    #[test]
    fn json_keeps_the_query_and_its_instant_beside_the_results() {
        let v = serde_json::to_value(found(json!([]))).unwrap();
        assert_eq!(
            v,
            json!({"query": "fed rate", "as_of": "2025-03-01T23:59:59Z", "request_id": "r", "results": [], "entities": []})
        );
    }
}
