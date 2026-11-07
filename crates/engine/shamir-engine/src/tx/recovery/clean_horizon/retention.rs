use shamir_storage::error::DbResult;

use crate::repo::RepoInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetentionAction {
    NoOverflow,
    Refused,
}

pub(crate) async fn enforce_retention(
    repo: &RepoInstance,
    safe_ceiling: u64,
) -> DbResult<RetentionAction> {
    let group = repo.wal_group().await?;
    if group.is_mem_sink() {
        return Ok(RetentionAction::NoOverflow);
    }
    let policy = repo.durability_policy();
    let cap = policy.wal_retention_max_bytes();
    let mode = policy.on_wal_retention_exceeded();
    drop(policy);

    let gate = repo.wal_retention_gate();
    // Measure on each pass, including following a successful unlink.
    let retained = group.sealed_retained_bytes().await?;
    if retained <= cap {
        gate.clear_refuse_if_set();
        return Ok(RetentionAction::NoOverflow);
    }
    gate.record_overflow_event();
    if mode == shamir_query_types::RetentionOverflow::RefuseWrites {
        gate.record_refusal();
        return Ok(RetentionAction::Refused);
    }
    let feed = match repo.changefeed().await {
        Ok(handle) => handle.feed,
        Err(error) => {
            log::warn!("clean_horizon: changefeed unavailable; refusing eviction: {error}");
            gate.record_refusal();
            return Ok(RetentionAction::Refused);
        }
    };
    let first = feed.clean_horizon().saturating_add(1);
    let last = group.max_committed_version().min(safe_ceiling);
    if first <= last {
        // Persist a recovery allocation floor before certifying the durable GAP;
        // this makes the interval safe across restart before any WAL unlink.
        let info = match repo.tx_info_store().await {
            Ok(info) => info,
            Err(error) => {
                log::warn!(
                    "clean_horizon: recovery floor store unavailable; refusing eviction: {error}"
                );
                gate.record_refusal();
                return Ok(RetentionAction::Refused);
            }
        };
        if let Err(error) = super::recovery_floor::reserve_recovery_floor(repo, &info, last).await {
            log::warn!("clean_horizon: recovery floor flush failed; refusing eviction: {error}");
            gate.record_refusal();
            return Ok(RetentionAction::Refused);
        }
        if let Err(error) = feed.record_recovery_gap(first, last).await {
            log::warn!("clean_horizon: gap certification failed; refusing eviction: {error}");
            gate.record_refusal();
            return Ok(RetentionAction::Refused);
        }
        if let Err(error) = feed.record_clean_horizon(last).await {
            log::warn!(
                "clean_horizon: clean horizon persistence failed; refusing eviction: {error}"
            );
            gate.record_refusal();
            return Ok(RetentionAction::Refused);
        }
        gate.record_declare_gap();
        // Durable gap + allocation floor certify loss for versions through last.
        // Unlink only through the history/interner-safe ceiling supplied by drainer.
        if safe_ceiling >= last {
            let wal = repo.repo_wal().await?;
            repo.flush_all_history().await?;
            wal.truncate_below(last).await?;
            let remaining = group.sealed_retained_bytes().await?;
            if remaining <= cap {
                gate.clear_refuse_if_set();
                return Ok(RetentionAction::NoOverflow);
            }
        }
    }
    // No covered retained range is currently safe to unlink; retry later.
    gate.record_refusal();
    Ok(RetentionAction::Refused)
}
