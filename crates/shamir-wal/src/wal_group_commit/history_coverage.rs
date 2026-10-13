//! Preserve exact history-backed evidence before unlink, off the append path.

use shamir_storage::error::DbResult;

use super::WalGroupCommit;

pub(super) async fn truncate(group: &WalGroupCommit, durable: u64) -> DbResult<usize> {
    // The drainer has flushed history before calling this seam. Snapshot the
    // exact versions before deletion; a maximum is never predecessor proof.
    // Serialize replay, unlink and evidence publication with snapshots. Sink
    // replay/truncate do not acquire this gate, so no recursive locking.
    let _sync = match group.sink.as_ref() {
        crate::wal_sink::WalSink::File(segments) => Some(segments.sync_gate.lock().await),
        crate::wal_sink::WalSink::Mem(_) => None,
    };
    let entries = group.sink.replay().await?;
    // History is already durable, so publish before unlink: partial unlink
    // failure must not strand evidence for segments already claimed/deleted.
    for entry in entries {
        if entry.commit_version > 0 && entry.commit_version <= durable {
            group.settle(entry.commit_version);
        }
    }
    group.sink.truncate_below(durable).await
}
