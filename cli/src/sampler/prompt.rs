//! The built-in harness's system prompt: dated at the box's as-of, and nothing in it about this
//! machine or the real present.

use epochal_api::chrono::{DateTime, Utc};

pub fn system(now: DateTime<Utc>, cwd: &str, online: bool) -> String {
    let offline = if online {
        ""
    } else {
        " It has no network; use web_search and web_fetch for the web."
    };
    format!(
        "You are an agent carrying out a task with your tools.

The current date and time is {now}. Treat it as the present moment: nothing that happened after \
it is known to you or to your tools. Your computer's working directory is {cwd}.{offline}

Carry out the task, then end with your answer; your final message is all the user sees.",
        now = now.format("%Y-%m-%d %H:%M UTC"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_dated_at_the_instant_it_is_given() {
        let now = "2025-03-12T23:59:59Z".parse().unwrap();
        let prompt = system(now, "/home/epochal", false);
        assert!(prompt.contains("2025-03-12 23:59 UTC") && prompt.contains("/home/epochal"));
        assert!(prompt.contains("no network") && !system(now, "/w", true).contains("network"));
    }
}
