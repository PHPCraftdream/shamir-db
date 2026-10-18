use std::time::Duration;

use shamir_db::repl_binding::SubscriptionLease;
use shamir_db::{DbResult, SeedRefusal, ShamirDb};
use tokio_util::sync::CancellationToken;

use crate::replication::ReplError;

/// Bounded, cancellation-aware retry for terminal catalogue marking (round 5,
/// B3). Up to 5 attempts; backoff 100 ms doubling, capped at 1600 ms. Each
/// wait is a `tokio::select!` against `tok`'s cancellation, and the retry
/// aborts early when `tok` is cancelled, the `lease` went invalid (the
/// subscription's lease was superseded — a stale generation's mark is a
/// no-op anyway), or the mark succeeded. Every failed attempt logs `warn!`
/// with the attempt number and error; exhaustion logs
/// "terminal marking retries exhausted".
///
/// LOCK CONSTRAINT: the retry runs AFTER the follower loop returned (its
/// apply-gate read guard is dropped), so taking the replication control lock
/// inside `mark_*` cannot overlap an in-flight apply of THIS loop. NEVER call
/// this while any apply-gate read guard is held.
pub(crate) async fn mark_with_retry<F, Fut>(
    name: &str,
    op: &str,
    tok: &CancellationToken,
    lease: &SubscriptionLease,
    mut mark: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = DbResult<()>>,
{
    const MAX_ATTEMPTS: u32 = 5;
    let mut delay_ms = 100u64;
    for attempt in 1..=MAX_ATTEMPTS {
        if tok.is_cancelled() || !lease.valid.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        match mark().await {
            Ok(()) => return,
            Err(error) => {
                if attempt == MAX_ATTEMPTS {
                    tracing::warn!(
                        subscription = %name,
                        op = %op,
                        error = %error,
                        "terminal marking retries exhausted"
                    );
                    return;
                }
                tracing::warn!(
                    subscription = %name,
                    op = %op,
                    attempt,
                    error = %error,
                    "terminal marking failed; retrying"
                );
                tokio::select! {
                    _ = tok.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                }
                delay_ms = (delay_ms * 2).min(1600);
            }
        }
    }
}

/// [`persist_terminal`] with the round-5 B3 bounded retry (see
/// [`mark_with_retry`] for the retry shape and the lock constraint).
pub(crate) async fn persist_terminal_with_retry(
    home: &ShamirDb,
    name: &str,
    error: &ReplError,
    generation: u64,
    tok: &CancellationToken,
    lease: &SubscriptionLease,
) {
    let (state, message) = match error {
        ReplError::Seed(reason) => (reason.code(), error.to_string()),
        ReplError::IdentityMismatch { code, .. } => (code.code(), error.to_string()),
        _ => return,
    };
    mark_with_retry(name, "persist_terminal", tok, lease, || {
        let home = home.clone();
        let name = name.to_string();
        let state = state.to_string();
        let message = message.clone();
        async move {
            home.mark_subscription_terminal_generation(&name, &state, &message, Some(generation))
                .await
        }
    })
    .await;
}

/// Preserve the typed seed refusal code instead of leaving an identity/schema
/// failure as a log line only. The S5c-X PR7 universal identity fence
/// (`ReplError::IdentityMismatch`) is persisted the same way, so the recorded
/// and observed identities remain available in per-repository ReplicationStatus.
/// Other terminal errors keep their prior handling. The mark is
/// generation-aware (`mark_subscription_terminal_generation(..., Some(generation))`):
/// a terminal callback from a task started under an older binding generation
/// is a logged no-op once the row was replaced under a new generation.
// Test-only (loop_tests.rs, `#![cfg(unix)]`): unused on non-Unix test builds.
#[cfg(all(test, unix))]
pub(crate) async fn persist_terminal(
    home: &ShamirDb,
    name: &str,
    error: &ReplError,
    generation: u64,
) {
    let (state, message) = match error {
        ReplError::Seed(reason) => (reason.code(), error.to_string()),
        ReplError::IdentityMismatch { code, .. } => (code.code(), error.to_string()),
        _ => return,
    };
    if let Err(mark_error) = home
        .mark_subscription_terminal_generation(name, state, &message, Some(generation))
        .await
    {
        tracing::warn!(error = %mark_error, "failed to persist terminal replication reason");
    }
}

pub(crate) async fn validate_target(
    home: &ShamirDb,
    streams: &[shamir_query_types::admin::ReplStream],
    db: &str,
    repo: &str,
) -> Result<(), ReplError> {
    if let Some(receipt) = super::fence::load_receipt(home, db, repo).await? {
        if streams.len() != 1
            || streams[0].direction != shamir_query_types::admin::ReplDirection::Pull
            || streams[0].mode != shamir_query_types::admin::ReplMode::ReadOnly
            || streams[0].scope.db != receipt.source_db
            || streams[0].scope.repo.as_deref() != Some(receipt.source_repo.as_str())
            || streams[0].scope.table.is_some()
        {
            return Err(SeedRefusal::WrongScope.into());
        }
    }
    Ok(())
}
