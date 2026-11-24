use std::future::Future;
use std::time::Duration;

use crate::LlmError;

/// Exponential backoff with a cap.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base: Duration,
    pub cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, base: Duration::from_millis(500), cap: Duration::from_secs(8) }
    }
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self { max_attempts: 1, ..Self::default() }
    }

    fn delay(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let exp = self.base.saturating_mul(1u32 << attempt.min(10));
        retry_after.unwrap_or(exp).min(self.cap)
    }

    /// Run `op` until it succeeds, fails permanently, or attempts run out.
    pub(crate) async fn run<T, F, Fut>(&self, mut op: F) -> Result<T, LlmError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, (LlmError, Option<Duration>)>>,
    {
        let mut attempt = 0;
        loop {
            match op().await {
                Ok(v) => return Ok(v),
                Err((e, retry_after)) => {
                    attempt += 1;
                    if !e.is_transient() || attempt >= self.max_attempts.max(1) {
                        return Err(e);
                    }
                    let wait = self.delay(attempt - 1, retry_after);
                    tracing::warn!(attempt, ?wait, error = %e, "model call failed; retrying");
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    fn fast() -> RetryPolicy {
        RetryPolicy { max_attempts: 3, base: Duration::from_millis(1), cap: Duration::from_millis(5) }
    }

    #[tokio::test]
    async fn retries_transient_then_succeeds() {
        let calls = AtomicU32::new(0);
        let out = fast()
            .run(|| async {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err((LlmError::Http { status: 503, body: String::new() }, None))
                } else {
                    Ok("fine")
                }
            })
            .await;
        assert_eq!((out.unwrap(), calls.load(Ordering::SeqCst)), ("fine", 3));
    }

    #[tokio::test]
    async fn permanent_errors_are_not_retried() {
        let calls = AtomicU32::new(0);
        let out: Result<(), _> = fast()
            .run(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err((LlmError::Http { status: 401, body: "no".into() }, None))
            })
            .await;
        assert!(matches!(out, Err(LlmError::Http { status: 401, .. })) && calls.load(Ordering::SeqCst) == 1);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let calls = AtomicU32::new(0);
        let out: Result<(), _> = fast()
            .run(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err((LlmError::Transport("down".into()), None))
            })
            .await;
        assert!(out.is_err() && calls.load(Ordering::SeqCst) == 3);
    }

    #[test]
    fn delay_is_exponential_capped_and_honours_retry_after() {
        let p = RetryPolicy { max_attempts: 5, base: Duration::from_millis(100), cap: Duration::from_millis(500) };
        assert_eq!(p.delay(0, None), Duration::from_millis(100));
        assert_eq!(p.delay(2, None), Duration::from_millis(400));
        assert_eq!(p.delay(9, None), Duration::from_millis(500));
        assert_eq!(p.delay(0, Some(Duration::from_millis(250))), Duration::from_millis(250));
        assert_eq!(
            p.delay(0, Some(Duration::from_mins(1))),
            Duration::from_millis(500),
            "a hostile Retry-After is capped"
        );
    }
}
