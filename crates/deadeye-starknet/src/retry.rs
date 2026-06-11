//! Retrying [`Provider`] wrapper — backoff with jitter for flaky/throttled
//! RPCs.
//!
//! Public Starknet RPC endpoints rate-limit by returning empty bodies (which
//! surface as `expected value at line 1 column 1` from serde), `429`s, or
//! transient `5xx`s. Issue #14 documented the failure mode: every error
//! surfaced instantly, the caller retried in a tight loop, and the throttle
//! deepened into a retry storm.
//!
//! [`RetryingProvider`] absorbs that class of failure:
//!
//! * **Classification** — only *retryable* errors (rate-limit signatures,
//!   timeouts, connection drops, 5xx) are retried; contract reverts and
//!   malformed-call errors surface immediately.
//! * **Backoff with jitter** — exponential delay between attempts with a
//!   deterministic-per-attempt jitter, never a hot loop.
//! * **Rate-limit-aware errors** — after exhausting attempts the error says
//!   what happened and what to do (`likely rate-limited — wait before
//!   retrying`), instead of leaking a raw serde parse error.

use std::time::Duration;

use async_trait::async_trait;
use starknet_core::types::{BlockId, Felt, FunctionCall};

use crate::{
    error::{ContractError, ContractResult},
    provider::Provider,
};

/// Maximum attempts per call (1 initial + 3 retries).
const MAX_ATTEMPTS: u32 = 4;
/// Base backoff before the first retry; doubles per attempt.
const BASE_BACKOFF: Duration = Duration::from_millis(600);

/// Signatures of a throttled / transiently-failing RPC. String-matched
/// because `starknet-providers` flattens transport errors into strings.
const RETRYABLE_SIGNATURES: &[&str] = &[
    // serde choking on an empty body — the classic rate-limit signature.
    "expected value at line 1 column 1",
    "eof while parsing",
    "429",
    "too many requests",
    "rate limit",
    "502",
    "503",
    "504",
    "bad gateway",
    "service unavailable",
    "gateway timeout",
    "timed out",
    "timeout",
    "connection reset",
    "connection closed",
    "error sending request",
];

/// True when an error message looks like throttling / transient transport
/// failure rather than a real contract error.
#[must_use]
pub fn is_retryable_provider_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    RETRYABLE_SIGNATURES.iter().any(|sig| lower.contains(sig))
}

/// True specifically for the empty-body / parse-error shape that public
/// endpoints produce when rate-limiting.
#[must_use]
pub fn is_rate_limit_signature(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("expected value at line 1 column 1")
        || lower.contains("eof while parsing")
        || lower.contains("429")
        || lower.contains("too many requests")
        || lower.contains("rate limit")
}

/// Backoff before retry `attempt` (1-based), with deterministic jitter so two
/// concurrent CLIs don't sync their retries.
fn backoff_delay(attempt: u32) -> Duration {
    let exp = BASE_BACKOFF.saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)));
    // ±25% jitter from a cheap hash of the attempt; deterministic for tests.
    let jitter_pct = i64::from((attempt.wrapping_mul(2_654_435_761)) % 51) - 25;
    let jitter = (exp.as_millis() as i64 * jitter_pct) / 100;
    let millis = (exp.as_millis() as i64 + jitter).max(50) as u64;
    Duration::from_millis(millis)
}

/// [`Provider`] wrapper adding bounded exponential backoff to every call.
#[derive(Debug)]
pub struct RetryingProvider<P> {
    inner: P,
}

impl<P> RetryingProvider<P> {
    /// Wrap a provider with retry/backoff semantics.
    #[must_use]
    pub const fn new(inner: P) -> Self {
        Self { inner }
    }

    /// Access the wrapped provider.
    #[must_use]
    pub const fn inner(&self) -> &P {
        &self.inner
    }
}

#[async_trait]
impl<P: Provider> Provider for RetryingProvider<P> {
    async fn call(&self, call: FunctionCall, block: BlockId) -> ContractResult<Vec<Felt>> {
        let mut last_error: Option<ContractError> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            match self.inner.call(call.clone(), block).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let message = err.to_string();
                    if !is_retryable_provider_error(&message) {
                        return Err(err);
                    }
                    if attempt < MAX_ATTEMPTS {
                        let delay = backoff_delay(attempt);
                        tracing::warn!(
                            target: "deadeye::rpc",
                            attempt,
                            max_attempts = MAX_ATTEMPTS,
                            delay_ms = delay.as_millis() as u64,
                            rate_limited = is_rate_limit_signature(&message),
                            "retryable RPC failure; backing off",
                        );
                        tokio::time::sleep(delay).await;
                    }
                    last_error = Some(err);
                },
            }
        }
        let raw = last_error.map_or_else(String::new, |e| e.to_string());
        let hint = if is_rate_limit_signature(&raw) {
            "RPC returned an empty/invalid response (likely rate-limited)"
        } else {
            "RPC kept failing transiently"
        };
        Err(ContractError::Provider(format!(
            "{hint}; retried {MAX_ATTEMPTS} times with backoff — wait before retrying \
             (the endpoint is a shared resource). Last error: {raw}",
        )))
    }

    fn default_block(&self) -> BlockId {
        self.inner.default_block()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    struct ScriptedProvider {
        /// Errors to emit before succeeding (popped front-to-back).
        script: Mutex<Vec<Result<Vec<Felt>, String>>>,
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl Provider for ScriptedProvider {
        async fn call(&self, _call: FunctionCall, _block: BlockId) -> ContractResult<Vec<Felt>> {
            *self.calls.lock().expect("mutex") += 1;
            let mut script = self.script.lock().expect("mutex");
            let next = if script.is_empty() {
                Err("expected value at line 1 column 1".to_owned())
            } else {
                script.remove(0)
            };
            next.map_err(ContractError::Provider)
        }
    }

    fn call() -> FunctionCall {
        FunctionCall {
            contract_address: Felt::ZERO,
            entry_point_selector: Felt::ZERO,
            calldata: vec![],
        }
    }

    #[test]
    fn classifies_rate_limit_signatures() {
        assert!(is_retryable_provider_error(
            "provider error: expected value at line 1 column 1"
        ));
        assert!(is_retryable_provider_error("HTTP 429 Too Many Requests"));
        assert!(is_retryable_provider_error("operation timed out"));
        assert!(!is_retryable_provider_error(
            "contract error: VERIFICATION_FAILED"
        ));
        assert!(is_rate_limit_signature("expected value at line 1 column 1"));
        assert!(!is_rate_limit_signature("503 service unavailable"));
    }

    #[test]
    fn backoff_grows_and_never_zero() {
        let d1 = backoff_delay(1);
        let d3 = backoff_delay(3);
        assert!(d1 >= Duration::from_millis(50));
        assert!(d3 > d1);
    }

    #[tokio::test]
    async fn recovers_after_transient_failures() {
        let provider = RetryingProvider::new(ScriptedProvider {
            script: Mutex::new(vec![
                Err("expected value at line 1 column 1".to_owned()),
                Err("503 service unavailable".to_owned()),
                Ok(vec![Felt::from(42_u64)]),
            ]),
            calls: Mutex::new(0),
        });
        let out = provider
            .call(call(), provider.default_block())
            .await
            .expect("recovers");
        assert_eq!(out, vec![Felt::from(42_u64)]);
        assert_eq!(*provider.inner().calls.lock().expect("mutex"), 3);
    }

    #[tokio::test]
    async fn exhausts_attempts_with_rate_limit_hint() {
        let provider = RetryingProvider::new(ScriptedProvider {
            script: Mutex::new(vec![]),
            calls: Mutex::new(0),
        });
        let err = provider
            .call(call(), provider.default_block())
            .await
            .expect_err("keeps failing");
        let message = err.to_string();
        assert!(message.contains("likely rate-limited"), "{message}");
        assert!(message.contains("wait before retrying"), "{message}");
        assert_eq!(*provider.inner().calls.lock().expect("mutex"), 4);
    }

    #[tokio::test]
    async fn non_retryable_errors_surface_immediately() {
        let provider = RetryingProvider::new(ScriptedProvider {
            script: Mutex::new(vec![Err("contract error: VERIFICATION_FAILED".to_owned())]),
            calls: Mutex::new(0),
        });
        let err = provider
            .call(call(), provider.default_block())
            .await
            .expect_err("surfaces");
        assert!(err.to_string().contains("VERIFICATION_FAILED"));
        assert_eq!(*provider.inner().calls.lock().expect("mutex"), 1);
    }
}
