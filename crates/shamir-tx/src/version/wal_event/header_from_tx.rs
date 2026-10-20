//! Build the WAL entry's journal-event header from a committing tx.

use shamir_storage::types::KvOp;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalEventHeader, WalExtraChange};

use crate::tx_context::TxContext;

/// Build the format-3 event header for a committing [`TxContext`].
///
/// Called by the engine's commit pipeline at entry construction (BEFORE
/// the WAL append), so the journal event stays a pure function of the
/// entry (S5c-X I1). The walk below iterates `tx.write_set` exactly like
/// `project_event` does - same map, same per-table `iter_ops` order - and
/// assigns each staged change its 0-based projection position, so
/// `event_from_wal_entry` can reproduce the projected change order
/// byte-for-byte.
///
/// Cost: one pass over the staged ops (no store I/O, no locks). The only
/// allocations are the strings the entry must own (repo name, table
/// names, actor) and `extra_changes` - empty (no allocation) for the
/// common all-`RecordId` commit.
pub fn event_header_from_tx(tx: &TxContext, repo: &str) -> WalEventHeader {
    let mut table_names: Vec<(u64, String)> = Vec::new();
    let mut extra_changes: Vec<WalExtraChange> = Vec::new();
    let mut change_index: u32 = 0;
    for (token, staging) in &tx.write_set {
        // Only resolvable names are recorded; an absent token falls back
        // to `token:{t}` at derivation, matching `project_event`.
        if let Some(name) = tx.table_tokens.get(token) {
            table_names.push((*token, name.clone()));
        }
        for kv in staging.iter_ops() {
            match kv {
                KvOp::Set(key, value) => {
                    if RecordId::try_from_bytes(&key).is_none() {
                        extra_changes.push(WalExtraChange {
                            table_id_interned: *token,
                            change_index,
                            key: bytes::Bytes::copy_from_slice(&key),
                            value: Some(value),
                        });
                    }
                }
                KvOp::Remove(key) => {
                    if RecordId::try_from_bytes(&key).is_none() {
                        extra_changes.push(WalExtraChange {
                            table_id_interned: *token,
                            change_index,
                            key: bytes::Bytes::copy_from_slice(&key),
                            value: None,
                        });
                    }
                }
            }
            change_index += 1;
        }
    }

    WalEventHeader {
        repo: repo.to_string(),
        actor: tx.actor.clone(),
        // PUBLIC contract: implicit batch txs report tx_id == 0
        // ("0 = non-tx write", LIVE_SUBSCRIPTIONS.md) - same rule the
        // live projection applies.
        tx_id: if tx.implicit { 0 } else { tx.tx_id.0 },
        timestamp_ns: now_nanos(),
        table_names,
        extra_changes,
    }
}

/// Wall-clock nanoseconds since the UNIX epoch (best-effort; 0 if the
/// clock is before the epoch). Same sampling rule `project_event` used.
fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
