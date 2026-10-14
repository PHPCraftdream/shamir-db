use crate::repo::RepoInstance;
use crate::tx::drainer::Drainer;
use shamir_storage::error::{DbError, DbResult};
use shamir_tx::RepoTxGate;

/// Settle registered orphans whose replay evidence disappeared from RAM and
/// WAL. Ordinary pending versions and any retained evidence are untouched.
pub(in crate::tx::drainer) async fn settle_missing_evidence(
    drainer: &Drainer,
    repo: &RepoInstance,
    gate: &RepoTxGate,
) -> DbResult<()> {
    for version in gate.orphaned_post_wal_versions() {
        let next_version = gate.durable_watermark().checked_add(1);
        let window_has_entry = {
            let guard = scc::Guard::new();
            drainer
                .window
                .range(version..=version, &guard)
                .next()
                .is_some()
        };
        if window_has_entry
            || gate.unresolved_wal(version).is_some()
            || gate.is_active_post_wal(version)
            || gate.orphan_entry(version).is_some()
            || next_version != Some(version)
        {
            continue;
        }
        log::warn!("orphan {version}: WAL replay evidence unavailable; recording minimal GAP");
        if let Err(error) = repo.mark_changefeed_gap(version).await {
            return Err(DbError::Storage(format!(
                "orphan {version} missing-evidence GAP failed: {error}"
            )));
        }
        gate.complete_orphaned_post_wal(version);
        gate.mark_durable(version);
        gate.forget_orphaned_post_wal(version);
        drainer.orphan_repair_metrics.gap();
    }
    Ok(())
}
