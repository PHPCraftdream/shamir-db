use shamir_db::{SeedRefusal, ShamirDb};

use crate::replication::ReplError;

/// Preserve the typed seed refusal code instead of leaving an identity/schema
/// failure as a log line only. The S5c-X PR7 universal identity fence
/// (`ReplError::IdentityMismatch`) is persisted the same way, so the recorded
/// and observed identities remain available in per-repository ReplicationStatus.
/// Other terminal errors keep their prior handling.
pub(crate) async fn persist_terminal(home: &ShamirDb, name: &str, error: &ReplError) {
    let (state, message) = match error {
        ReplError::Seed(reason) => (reason.code(), error.to_string()),
        ReplError::IdentityMismatch { code, .. } => (code.code(), error.to_string()),
        _ => return,
    };
    if let Err(mark_error) = home.mark_subscription_terminal(name, state, &message).await {
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
