//! Reconcile pending per-alias receipts through `WaitForVisibility`.

use std::future::Future;

use shamir_collections::TMap;
use shamir_query_types::batch::VisibilityState;

use super::alias_receipt::AliasReceipt;
use crate::ClientError;

/// One repository observation produced by
/// [`reconcile_receipts`](super::reconcile_receipts).
#[derive(Debug, Clone, PartialEq)]
pub struct RepoObservation {
    /// Repository the observation was issued against.
    pub repo: String,
    /// The reconciled position: the highest pending `commit_version`
    /// among that repository's receipts.
    pub version: u64,
    /// The fresh visibility observation.
    pub visibility: VisibilityState,
}

/// Reconcile pending receipts: group them by repository and issue ONE
/// observation per repository, at the highest pending position. Visible
/// receipts are skipped. Receipts with no derivable repository and
/// pending receipts without a commit position are refused with a typed
/// error rather than guessed against some repository.
///
/// NO write is ever resent and there is no retry loop: `observe` is
/// called exactly once per distinct repository, and a transport failure
/// propagates as-is (the acknowledgement of the underlying writes is
/// unchanged — they are committed regardless).
pub async fn reconcile_receipts<F, Fut>(
    receipts: &[AliasReceipt],
    observe: F,
) -> Result<Vec<RepoObservation>, ClientError>
where
    F: Fn(String, u64) -> Fut,
    Fut: Future<Output = Result<VisibilityState, ClientError>>,
{
    let mut targets: TMap<String, u64> = TMap::default();
    for receipt in receipts {
        if receipt.visibility == VisibilityState::Visible {
            continue;
        }
        let repo = receipt.repo.clone().ok_or_else(|| {
            ClientError::Protocol(format!(
                "receipt for alias '{}' has no derivable repository; refusing to guess",
                receipt.alias
            ))
        })?;
        let version = receipt.transaction.commit_version.ok_or_else(|| {
            ClientError::Protocol(format!(
                "pending receipt for alias '{}' carries no commit_version",
                receipt.alias
            ))
        })?;
        let is_new_max = match targets.get(&repo) {
            Some(current) => *current < version,
            None => true,
        };
        if is_new_max {
            targets.insert(repo, version);
        }
    }
    let mut observations = Vec::with_capacity(targets.len());
    for (repo, version) in targets {
        let visibility = observe(repo.clone(), version).await?;
        observations.push(RepoObservation {
            repo,
            version,
            visibility,
        });
    }
    Ok(observations)
}
