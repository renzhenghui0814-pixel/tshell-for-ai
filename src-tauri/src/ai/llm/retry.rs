//! Tries a failed completion again, for the failures that are about the moment
//! rather than about the request.
//!
//! A rate limit, a dropped connection and a gateway that timed out all describe
//! conditions that pass; a bad key or a malformed request describes one that does
//! not, and asking twice only wastes the user's time. So the list is short and
//! closed, and everything not on it fails on the first answer as it always did.
//!
//! Wrapped around the provider rather than folded into the loop, because it is
//! the transport's business and not the loop's. The loop keeps its own one-shot
//! retry for an empty reply: that one is about the shape of the answer.

use std::time::Duration;

use super::{Completion, CompletionRequest, LlmError, LlmFailure, LlmProvider};

fn is_retryable(kind: LlmFailure) -> bool {
    matches!(
        kind,
        LlmFailure::RateLimited | LlmFailure::Network | LlmFailure::Timeout
    )
}

/// Quadrupling, from one second. Jittered so several panels do not return
/// together.
///
/// The jitter is drawn from the clock rather than from a random number
/// generator: this decides how long to wait, not anything anyone could game, and
/// a dependency for it would be a dependency to read one number.
fn backoff_ms(attempt: u32) -> u64 {
    let base = 1000u64 * 4u64.saturating_pow(attempt.saturating_sub(1));
    let spread = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos() as u64 % 1000)
        .unwrap_or(500);
    // 0.75x to 1.25x, the same window the original used.
    base * (750 + spread / 2) / 1000
}

/// How many times the request may be made in total, including the first. Two
/// retries is the point where a rate limit has usually cleared and before the
/// wait is longer than a user will sit through.
const ATTEMPTS: u32 = 3;

pub struct WithRetry<P> {
    inner: P,
    attempts: u32,
}

impl<P: LlmProvider> WithRetry<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            attempts: ATTEMPTS,
        }
    }

    #[cfg(test)]
    fn with_attempts(inner: P, attempts: u32) -> Self {
        Self { inner, attempts }
    }
}

impl<P: LlmProvider> LlmProvider for WithRetry<P> {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn complete<'a>(&'a self, request: CompletionRequest) -> Completion<'a> {
        Box::pin(async move {
            for attempt in 1.. {
                // Each attempt needs its own request, because the watcher and the
                // token are shared handles but the struct itself is consumed.
                let one = CompletionRequest {
                    system: request.system.clone(),
                    fallback_system: request.fallback_system.clone(),
                    messages: request.messages.clone(),
                    timeout_ms: request.timeout_ms,
                    cancel: request.cancel.clone(),
                    watcher: request.watcher.clone(),
                    tools: request.tools.clone(),
                };
                let error = match self.inner.complete(one).await {
                    Ok(reply) => return Ok(reply),
                    Err(error) => error,
                };

                let last = attempt >= self.attempts;
                if last || request.cancel.is_cancelled() || !is_retryable(error.kind) {
                    return Err(error);
                }

                request.watcher.retry(attempt, error.kind);
                let waited = tokio::time::sleep(Duration::from_millis(backoff_ms(attempt)));
                tokio::select! {
                    biased;
                    () = request.cancel.cancelled() => {
                        return Err(LlmError::new(LlmFailure::Cancelled, "cancelled"));
                    }
                    () = waited => {}
                }
            }
            unreachable!("the loop returns on the last attempt")
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::ai::llm::{Reply, Silent, Watcher};

    struct Failing {
        kind: LlmFailure,
        calls: AtomicU32,
        succeed_on: u32,
    }

    impl Failing {
        fn new(kind: LlmFailure, succeed_on: u32) -> Self {
            Self {
                kind,
                calls: AtomicU32::new(0),
                succeed_on,
            }
        }
    }

    impl LlmProvider for Failing {
        fn describe(&self) -> String {
            "a test".into()
        }
        fn complete<'a>(&'a self, _request: CompletionRequest) -> Completion<'a> {
            let seen = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            let kind = self.kind;
            let succeed_on = self.succeed_on;
            Box::pin(async move {
                if seen >= succeed_on {
                    Ok(Reply::text("ok"))
                } else {
                    Err(LlmError::new(kind, "nope"))
                }
            })
        }
    }

    #[derive(Default)]
    struct Counting {
        retries: Mutex<Vec<(u32, LlmFailure)>>,
    }

    impl Watcher for Counting {
        fn retry(&self, attempt: u32, kind: LlmFailure) {
            self.retries.lock().unwrap().push((attempt, kind));
        }
    }

    fn request(cancel: crate::ai::cancel::Cancel, watcher: Arc<dyn Watcher>) -> CompletionRequest {
        CompletionRequest {
            system: "s".into(),
            fallback_system: "json".into(),
            messages: Vec::new(),
            timeout_ms: 0,
            cancel,
            watcher,
            tools: Vec::new(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_passing_condition_is_tried_again_and_succeeds() {
        let watcher = Arc::new(Counting::default());
        let provider = WithRetry::new(Failing::new(LlmFailure::RateLimited, 3));
        let reply = provider
            .complete(request(Default::default(), watcher.clone()))
            .await
            .unwrap();
        assert_eq!(reply.text, "ok");
        let retries = watcher.retries.lock().unwrap();
        assert_eq!(retries.len(), 2);
        assert_eq!(retries[0], (1, LlmFailure::RateLimited));
        assert_eq!(retries[1], (2, LlmFailure::RateLimited));
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_key_is_not_asked_about_twice() {
        let watcher = Arc::new(Counting::default());
        let provider = WithRetry::new(Failing::new(LlmFailure::Unauthorized, 99));
        let error = provider
            .complete(request(Default::default(), watcher.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.kind, LlmFailure::Unauthorized);
        assert!(watcher.retries.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_reply_is_left_to_the_loop() {
        let provider = WithRetry::new(Failing::new(LlmFailure::Empty, 99));
        let error = provider
            .complete(request(Default::default(), Arc::new(Silent)))
            .await
            .unwrap_err();
        assert_eq!(error.kind, LlmFailure::Empty);
    }

    #[tokio::test(start_paused = true)]
    async fn the_last_attempt_reports_rather_than_waiting_again() {
        let inner = Failing::new(LlmFailure::Network, 99);
        let provider = WithRetry::with_attempts(inner, 2);
        let watcher = Arc::new(Counting::default());
        let error = provider
            .complete(request(Default::default(), watcher.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.kind, LlmFailure::Network);
        assert_eq!(watcher.retries.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancelled_request_stops_rather_than_backing_off() {
        let cancel = crate::ai::cancel::Cancel::new();
        cancel.cancel();
        let provider = WithRetry::new(Failing::new(LlmFailure::Network, 99));
        let error = provider
            .complete(request(cancel, Arc::new(Silent)))
            .await
            .unwrap_err();
        assert_eq!(error.kind, LlmFailure::Network);
    }

    #[test]
    fn the_backoff_grows_and_stays_inside_its_jitter_window() {
        for attempt in 1..=3 {
            let base = 1000u64 * 4u64.pow(attempt - 1);
            let value = backoff_ms(attempt);
            assert!(value >= base * 3 / 4, "attempt {attempt}: {value}");
            assert!(value <= base * 5 / 4, "attempt {attempt}: {value}");
        }
    }
}
