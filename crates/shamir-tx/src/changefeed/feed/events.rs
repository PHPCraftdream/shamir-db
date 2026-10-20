//! Changelog event types, key/metadata codecs, and event projection helpers.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_types::access::Actor;

/// One record-level change inside a committed transaction.
///
/// `key` is the raw storage key the tx staged (for data tables this is the
/// 16-byte `RecordId`). `value` carries the FULL new record bytes for a
/// [`ChangeOp::Put`] — the same bytes the WAL entry serialises, taken from
/// the tx's staging snapshot at projection time — and is `None` for a
/// [`ChangeOp::Delete`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordChange {
    /// Human-readable table name (resolved from the tx's `table_tokens`).
    pub table: String,
    /// Raw storage key (16-byte `RecordId` for data tables).
    #[serde(with = "serde_bytes_compat")]
    pub key: Bytes,
    /// The mutation kind.
    pub op: ChangeOp,
    /// New record bytes for `Put`; `None` for `Delete`.
    #[serde(with = "serde_opt_bytes_compat")]
    pub value: Option<Bytes>,
}

/// The mutation kind of a [`RecordChange`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangeOp {
    /// Insert or update — `RecordChange::value` carries the new bytes.
    Put,
    /// Delete — `RecordChange::value` is `None`.
    Delete,
}

/// One committed transaction, projected for the changefeed.
///
/// Journal keys order events by `commit_version` (monotonic per repo), but
/// enqueue and live-broadcast order need not match version order. The engine
/// enqueues immediately BEFORE version publication and broadcasts AFTER it.
/// Engine journal pulls confirm and bound the settled prefix before exposing
/// events; live subscribers observe published versions in completion order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogEvent {
    /// Repository name this commit belongs to.
    pub repo: String,
    /// MVCC commit version assigned to this tx (monotonic per repo).
    pub commit_version: u64,
    /// The committing transaction's id.
    pub tx_id: u64,
    /// The actor that initiated the transaction.
    pub actor: Actor,
    /// Wall-clock nanoseconds at projection time (best-effort, for
    /// observability — NOT used for ordering; `commit_version` is).
    pub timestamp_ns: u64,
    /// Per-record changes carried by this commit.
    pub changes: Vec<RecordChange>,
}

/// `bytes::Bytes` <-> serde adapter (serialises as a byte sequence).
mod serde_bytes_compat {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let v = Vec::<u8>::deserialize(d)?;
        Ok(Bytes::from(v))
    }
}

/// `Option<bytes::Bytes>` <-> serde adapter.
mod serde_opt_bytes_compat {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Option<Bytes>, s: S) -> Result<S::Ok, S::Error> {
        match b {
            Some(b) => s.serialize_some(b.as_ref()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Bytes>, D::Error> {
        let v = Option::<Vec<u8>>::deserialize(d)?;
        Ok(v.map(Bytes::from))
    }
}

/// Capacity of the per-repo live broadcast ring. A subscriber lagging past
/// this many events sees `RecvError::Lagged` and catches up via the journal.
pub const BROADCAST_CAPACITY: usize = 1024;

/// Bounded depth of the journal-writer channel. `try_send` past this drops
/// the event with a warning (see [`RepoChangefeed::emit`]).
pub const JOURNAL_CHANNEL_CAPACITY: usize = 4096;

/// Versioned clean-stop / cold-certification metadata, never a journal event.
pub fn clean_horizon_key() -> Bytes {
    Bytes::from_static(b"\0journal-clean-v1")
}

/// Format: ASCII JCLN, format version 1, BE u64 horizon (13 bytes).
pub fn encode_clean_horizon(through: u64) -> Bytes {
    let mut bytes = Vec::with_capacity(13);
    bytes.extend_from_slice(b"JCLN\x01");
    bytes.extend_from_slice(&through.to_be_bytes());
    Bytes::from(bytes)
}

pub fn decode_clean_horizon(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() != 13 || &bytes[..5] != b"JCLN\x01" {
        return Err("invalid clean horizon format/version/length".into());
    }
    Ok(u64::from_be_bytes(
        bytes[5..].try_into().map_err(|_| "invalid clean horizon")?,
    ))
}

/// Dedicated metadata key, sorting before every eight-byte journal key.
/// This key is never an event and must be excluded from journal ranges.
pub fn gap_marker_key() -> Bytes {
    Bytes::from_static(b"\0")
}

/// Encode a valid conservative interval. Point markers retain the legacy BE u64
/// encoding; ranges append a BE u64 upper endpoint (16 bytes total).
pub fn encode_gap_marker((first, through): (u64, u64)) -> Bytes {
    assert!(first != 0 && through >= first);
    if first == through {
        return version_key(first);
    }
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&first.to_be_bytes());
    bytes.extend_from_slice(&through.to_be_bytes());
    Bytes::from(bytes)
}

/// Decode legacy point metadata or the extended interval, rejecting corruption.
pub fn decode_gap_marker(bytes: &[u8]) -> Result<(u64, u64), String> {
    if bytes.len() != 8 && bytes.len() != 16 {
        return Err("invalid gap marker length".into());
    }
    let first = u64::from_be_bytes(bytes[..8].try_into().map_err(|_| "invalid first")?);
    let through = if bytes.len() == 8 {
        first
    } else {
        u64::from_be_bytes(bytes[8..].try_into().map_err(|_| "invalid through")?)
    };
    if first == 0 || through < first {
        return Err("invalid gap marker interval".into());
    }
    Ok((first, through))
}

/// Big-endian 8-byte key for a commit version — lexicographic order on the
/// store matches numeric order.
pub fn version_key(commit_version: u64) -> Bytes {
    Bytes::copy_from_slice(&commit_version.to_be_bytes())
}

/// Project a committed [`crate::TxContext`] into a [`ChangelogEvent`].
///
/// Must be called on the commit-path BEFORE `write_set` is drained
/// (Phase 5a `collect_data_batches` consumes it), so the staged values are
/// still present. Reads each per-table `StagingStore` via `iter_ops`
/// (borrowed, no intermediate `Vec<KvOp>`) and resolves the human-readable
/// table name from `tx.table_tokens`.
///
/// Returns `None` when the tx staged no data writes — an empty footprint
/// emits nothing (index-only / counter-only commits do not produce a
/// record-level changefeed event in this MVP; see module docs).
///
/// `repo` is supplied by the engine (the `TxContext` only carries the
/// interned `repo_id`). `timestamp_ns` is captured here, best-effort.
pub fn project_event(
    tx: &crate::TxContext,
    repo: &str,
    commit_version: u64,
) -> Option<ChangelogEvent> {
    let total_ops: usize = tx.write_set.values().map(|s| s.len()).sum();
    let mut changes: Vec<RecordChange> = Vec::with_capacity(total_ops);
    for (token, staging) in &tx.write_set {
        let table = tx
            .table_tokens
            .get(token)
            .cloned()
            .unwrap_or_else(|| format!("token:{token}"));
        for kv in staging.iter_ops() {
            match kv {
                shamir_storage::types::KvOp::Set(key, value) => changes.push(RecordChange {
                    table: table.clone(),
                    // Boundary: KvOp keys are `RecordKey`; the changelog event
                    // key is `Bytes` (byte-identical conversion).
                    key: key.into(),
                    op: ChangeOp::Put,
                    value: Some(value),
                }),
                shamir_storage::types::KvOp::Remove(key) => changes.push(RecordChange {
                    table: table.clone(),
                    key: key.into(),
                    op: ChangeOp::Delete,
                    value: None,
                }),
            }
        }
    }

    if changes.is_empty() {
        return None;
    }

    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    Some(ChangelogEvent {
        repo: repo.to_string(),
        commit_version,
        // EXTERNAL contract: implicit batch txs (non-tx writes routed through
        // the tx pipeline, F4b-1) report tx_id == 0 — "0 = non-tx write" per
        // docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md. The INTERNAL tx.tx_id stays real.
        tx_id: if tx.implicit { 0 } else { tx.tx_id.0 },
        actor: tx.actor.clone(),
        timestamp_ns,
        changes,
    })
}

/// Assemble a [`ChangelogEvent`] from pre-built record changes for a
/// **non-transactional** write batch.
///
/// The transactional [`project_event`] reads its changes out of a
/// committed [`crate::TxContext`]'s staging snapshot. Non-tx writes
/// (`execute_insert` / `execute_update` / `execute_set` / `execute_delete`)
/// never build a `TxContext`; they apply mutations directly to the table
/// and already hold the `(key, value)` pairs they wrote. This constructor
/// lets that path emit an identically-shaped event without a tx.
///
/// `commit_version` MUST be allocated from the SAME per-repo
/// [`crate::RepoTxGate`] the commit pipeline uses, so non-tx and tx events
/// share one monotonic version sequence per repo. `tx_id` is `0` — a non-tx
/// write has no transaction id (the field is retained for shape parity).
///
/// Returns `None` for an empty `changes` vector — an empty footprint emits
/// nothing, matching [`project_event`].
pub fn nontx_event(
    repo: &str,
    commit_version: u64,
    actor: Actor,
    changes: Vec<RecordChange>,
) -> Option<ChangelogEvent> {
    if changes.is_empty() {
        return None;
    }
    let timestamp_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    Some(ChangelogEvent {
        repo: repo.to_string(),
        commit_version,
        tx_id: 0,
        actor,
        timestamp_ns,
        changes,
    })
}
