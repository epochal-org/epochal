//! Retries for the calls this crate makes. Every call it makes is safe to repeat: reads and polls
//! change nothing, writes land at an offset, stdin goes by `seq`, and creates and starts carry an
//! idempotency key. So a failure that may pass on its own is retried, with backoff, and anything
//! else is returned at once.

use std::future::Future;
use std::hash::{BuildHasher, RandomState};
use std::time::Duration;

use epochal_api::Error as ApiError;

const MAX_ATTEMPTS: u32 = 6;
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
/// The most one call waits across its retries; a `Retry-After` past it ends the call instead.
const BUDGET: Duration = Duration::from_secs(60);

/// `call` until it succeeds, fails for good, or the attempts or the wait budget run out.
pub(crate) async fn retry<T, F, Fut>(mut call: F) -> Result<T, ApiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
{
    let mut waited = Duration::ZERO;
    for attempt in 1.. {
        let err = match call().await {
            Ok(value) => return Ok(value),
            Err(err) => err,
        };
        let Some(asked) = transient(&err) else {
            return Err(err);
        };
        let wait = asked.unwrap_or_default().max(backoff(attempt));
        if attempt >= MAX_ATTEMPTS || waited + wait > BUDGET {
            return Err(err);
        }
        tokio::time::sleep(wait).await;
        waited += wait;
    }
    unreachable!("the loop returns")
}

/// Whether `err` may pass on its own, and the wait the API asked for if it did. Any other error
/// is a refusal: the call was not carried out.
pub(crate) fn transient(err: &ApiError) -> Option<Option<Duration>> {
    match err {
        ApiError::Transport(_) => Some(None),
        ApiError::Api {
            status: 408 | 429 | 500 | 502..=504 | 520..=524,
            retry_after,
            ..
        } => Some(*retry_after),
        _ => None,
    }
}

/// Exponential from [`FIRST_BACKOFF`] to [`MAX_BACKOFF`], each wait drawn from its upper half so
/// clients that failed together do not retry together.
fn backoff(attempt: u32) -> Duration {
    let ceiling = FIRST_BACKOFF
        .saturating_mul(1 << (attempt - 1).min(16))
        .min(MAX_BACKOFF);
    ceiling.mul_f64(0.5 + random() as f64 / u64::MAX as f64 / 2.0)
}

/// 64 random bits from the standard library's per-process seeded hasher.
pub(crate) fn random() -> u64 {
    RandomState::new().hash_one(0u8)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn dropped() -> ApiError {
        ApiError::Transport(Box::new(io::Error::from(io::ErrorKind::ConnectionReset)))
    }

    #[tokio::test(start_paused = true)]
    async fn a_dropped_connection_is_retried_until_the_call_passes() {
        let calls = AtomicU32::new(0);
        let got = retry(|| async {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => Err(dropped()),
                n => Ok(n),
            }
        })
        .await;
        assert_eq!(got.unwrap(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_is_returned_at_once_and_retries_stop_after_the_last_attempt() {
        let calls = AtomicU32::new(0);
        let refused: Result<(), _> = retry(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(ApiError::InvalidArgument("no".into()))
        })
        .await;
        assert!(matches!(refused, Err(ApiError::InvalidArgument(_))));
        assert_eq!(calls.swap(0, Ordering::SeqCst), 1);

        let down: Result<(), _> = retry(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(dropped())
        })
        .await;
        assert!(matches!(down, Err(ApiError::Transport(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn the_first_wait_is_under_a_second_and_none_passes_eight() {
        let first = backoff(1);
        assert!(first >= Duration::from_millis(250) && first <= Duration::from_millis(500));
        for attempt in 2..40 {
            assert!(backoff(attempt) <= Duration::from_secs(8), "{attempt}");
        }
        assert!(backoff(30) >= Duration::from_secs(4));
    }
}
