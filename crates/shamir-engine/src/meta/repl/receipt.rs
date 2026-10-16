//! Pure, versioned codec for the replication redo receipt (S5c-X PR6b-1).
//!
//! The receipt is the durable record of one translated replication batch:
//! identity, the explicit upstream→local version mapping, the translated
//! operations, the exact downstream journal event bytes, timestamp updates,
//! counter deltas, and the apply phase. Persisting it before apply and
//! deleting it after makes follower resume retry-safe.
//!
//! This module is a codec only — no production path writes or reads
//! receipts in this PR; PR6b-2 activates the writer/reader.

use crate::meta::{MetaEnvelope, MetaKey, ENVELOPE_MAGIC, ENVELOPE_VERSION};
use bytes::Bytes;
use rustc_hash::FxHasher;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Structural cap on `mapping`, `downstream_events` and `timestamps`:
/// at most this many events per receipt batch.
pub const MAX_RECEIPT_EVENTS: usize = 4096;
/// Structural cap on translated operations per receipt.
pub const MAX_RECEIPT_OPS: usize = 65536;
/// Approximate encoded-size cap. NOT enforced by this codec: the storage
/// layer enforces it; this codec enforces only the structural bounds above
/// (plus the combined-event bound checked in `validate`).
pub const MAX_RECEIPT_BYTES: usize = 8 * 1024 * 1024;

/// Where the batch is in the apply pipeline. Advances strictly
/// `Prepared` → `Materialized` → `Checkpointed`; each advance is a full
/// receipt rewrite (records are not updated in place).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ReceiptPhase {
    /// Recorded before anything is applied; resume starts (or restarts)
    /// materialization.
    Prepared,
    /// History rows + journal events written, published floor not yet
    /// advanced; resume re-publishes idempotently.
    Materialized,
    /// Bookmark/floor advanced and the receipt is about to be deleted;
    /// observed only transiently.
    Checkpointed,
}

/// One upstream → local version translation. The mapping is an EXPLICIT
/// list (not a range) so skipped or non-contiguous upstream inputs remain
/// representable; entries may legitimately be empty or non-contiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReceiptVersionMapping {
    pub upstream_version: u64,
    pub local_version: u64,
}

/// One translated operation. `value: None` is a delete/tombstone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReceiptOp {
    pub table: String,
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

/// A field-name binding used by the batch's translated records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReceiptBinding {
    pub interned_id: u64,
    pub name: String,
}

/// A local-history timestamp update performed by the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReceiptTimestamp {
    pub local_version: u64,
    pub ts_ms: u64,
}

/// Counter before/after pair for one counter table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReceiptCounterDelta {
    pub table: String,
    pub before: u64,
    pub after: u64,
}

/// The durable record of one outstanding replication redo batch
/// (S5c-X PR6b-1). Identity = lineage + upstream epoch + predecessor
/// bookmark + upstream version range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct ReplicationReceipt {
    /// Upstream lineage this batch belongs to.
    pub lineage_id: [u8; 16],
    /// Upstream history epoch at batch production.
    pub upstream_epoch: u64,
    /// Bookmark value that was current BEFORE this batch: the resume
    /// rollback point if the whole batch is discarded.
    pub predecessor_bookmark: u64,
    /// First upstream `commit_version` covered by this batch (inclusive).
    pub upstream_start: u64,
    /// Last upstream `commit_version` covered by this batch (inclusive).
    pub upstream_end: u64,
    /// Explicit upstream→local version translations (see
    /// [`ReceiptVersionMapping`]).
    pub mapping: Vec<ReceiptVersionMapping>,
    /// Immutable translated operations (see [`ReceiptOp`]).
    pub ops: Vec<ReceiptOp>,
    /// Number of entries of `ops` belonging to each event, in order
    /// (parallel to `mapping`; the entries sum to `ops.len()`).
    pub ops_per_event: Vec<u32>,
    /// Field-name bindings referenced by the translated records.
    pub bindings: Vec<ReceiptBinding>,
    /// The exact downstream journal event bytes produced for this batch;
    /// re-append on resume without re-translation.
    pub downstream_events: Vec<Vec<u8>>,
    /// Local timestamp updates performed by the batch.
    pub timestamps: Vec<ReceiptTimestamp>,
    /// Absolute before/after counter values per touched counter table.
    pub counters: Vec<ReceiptCounterDelta>,
    /// Current phase in the `Prepared → Materialized → Checkpointed`
    /// pipeline.
    pub phase: ReceiptPhase,
}

/// Domain-separated, deterministic NON-cryptographic idempotency id.
///
/// It deduplicates retries and binds a receipt's identity: a receipt's
/// bytes always hash to the id it is stored under. Corruption detection is
/// the envelope's job, not this hash's.
pub fn receipt_id(receipt: &ReplicationReceipt) -> [u8; 16] {
    let mut first = FxHasher::default();
    "SDB2-RRC-V1-A".hash(&mut first);
    receipt.hash(&mut first);
    let mut second = FxHasher::default();
    "SDB2-RRC-V1-B".hash(&mut second);
    receipt.hash(&mut second);
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&first.finish().to_le_bytes());
    id[8..].copy_from_slice(&second.finish().to_le_bytes());
    id
}

/// Structural + range sanity validation. Deliberately NOT over-restrictive:
/// the mapping list may be empty or non-contiguous and the lineage is not
/// checked for emptiness; only the structural bounds and
/// `upstream_end >= upstream_start` are enforced.
fn validate(receipt: &ReplicationReceipt) -> DbResult<()> {
    if receipt.mapping.len() > MAX_RECEIPT_EVENTS
        || receipt.downstream_events.len() > MAX_RECEIPT_EVENTS
        || receipt.timestamps.len() > MAX_RECEIPT_EVENTS
        || receipt.ops.len() > MAX_RECEIPT_OPS
    {
        return Err(DbError::Storage(
            "replication receipt exceeds structural size bounds".into(),
        ));
    }
    let combined = receipt.mapping.len()
        + receipt.ops.len()
        + receipt.downstream_events.len()
        + receipt.timestamps.len()
        + receipt.counters.len();
    if combined > MAX_RECEIPT_EVENTS {
        return Err(DbError::Storage(
            "replication receipt exceeds combined event bound".into(),
        ));
    }
    if receipt.upstream_end < receipt.upstream_start {
        return Err(DbError::Storage("invalid upstream range".into()));
    }
    if receipt.ops_per_event.len() != receipt.mapping.len() {
        return Err(DbError::Storage(
            "replication receipt ops_per_event must parallel mapping".into(),
        ));
    }
    if receipt
        .ops_per_event
        .iter()
        .map(|&count| count as usize)
        .sum::<usize>()
        != receipt.ops.len()
    {
        return Err(DbError::Storage(
            "replication receipt ops_per_event must sum to ops.len()".into(),
        ));
    }
    Ok(())
}

/// Encode a receipt under tuple version 2 after validation. Version 1 never
/// shipped (no production writer existed before the format stabilized), so
/// the tuple version jumps straight to 2: bincode is not self-describing,
/// and a strict bump guarantees the same binary that writes a receipt can
/// always recover it.
pub fn encode_receipt(receipt: &ReplicationReceipt) -> DbResult<Vec<u8>> {
    validate(receipt)?;
    MetaEnvelope::new((2u16, receipt.clone()))
        .encode()
        .map_err(|e| DbError::Storage(format!("replication receipt codec: {e}")))
}

/// Decode and fully verify a receipt: unknown tuple versions fail closed,
/// structural bounds and range sanity are re-checked, and the decoded
/// bytes must hash to `id` (identity binding).
pub fn decode_receipt(id: &[u8; 16], bytes: &[u8]) -> DbResult<ReplicationReceipt> {
    // bincode trusts Vec lengths, so deserialization of arbitrary garbage
    // could request an enormous allocation before the envelope's own magic
    // check runs. Reject a bad magic up front, allocation-free.
    if bytes.len() < ENVELOPE_MAGIC.len() || bytes[..ENVELOPE_MAGIC.len()] != ENVELOPE_MAGIC[..] {
        return Err(DbError::Storage(
            "replication receipt codec: bad envelope magic".into(),
        ));
    }
    let envelope: MetaEnvelope<(u16, ReplicationReceipt)> = bincode::deserialize(bytes)
        .map_err(|e| DbError::Storage(format!("replication receipt codec: {e}")))?;
    if envelope.magic != ENVELOPE_MAGIC {
        return Err(DbError::Storage(
            "replication receipt codec: bad envelope magic".into(),
        ));
    }
    if envelope.version != ENVELOPE_VERSION {
        return Err(DbError::Storage(format!(
            "replication receipt codec: bad envelope version {}",
            envelope.version
        )));
    }
    // bincode::deserialize allows trailing bytes (bincode 1.3 default
    // options); re-encoding the decoded envelope is deterministic, so it
    // reproduces the input length IFF nothing trailed. This rejects torn /
    // appended records.
    if envelope
        .encode()
        .map_err(|e| DbError::Storage(format!("replication receipt codec: {e}")))?
        .len()
        != bytes.len()
    {
        return Err(DbError::Storage(
            "replication receipt codec: trailing bytes after envelope".into(),
        ));
    }
    let (version, receipt) = envelope.payload;
    if version != 2 {
        return Err(DbError::Storage(format!(
            "replication receipt codec: unsupported version {version}"
        )));
    }
    validate(&receipt)?;
    if receipt_id(&receipt) != *id {
        return Err(DbError::Storage(
            "replication receipt codec: receipt id mismatch".into(),
        ));
    }
    Ok(receipt)
}

// ── Writer helpers (S5c-X PR6b-2) ─────────────────────────────────────
//
// Production, follower-path-only accessors around the codec above. None of
// them flushes: callers either flush explicitly or stage the write inside a
// repository-wide atomic batch.

/// The `__tx__` info-store key the outstanding receipt lives under.
pub(crate) fn receipt_key() -> RecordKey {
    RecordKey::from_slice(MetaKey::ReplicationReceipt.as_record_id().as_bytes())
}

/// Encode and write the receipt WITHOUT flushing — callers flush or ride an
/// atomic batch.
#[allow(dead_code)] // wired by the PR6b-2 receipt-apply chunk (W3+)
pub(crate) async fn put_receipt(
    info: &Arc<dyn Store>,
    receipt: &ReplicationReceipt,
) -> DbResult<()> {
    let bytes = encode_receipt(receipt)?;
    info.set(receipt_key(), Bytes::from(bytes))
        .await
        .map(|_| ())
}

/// Strictly decode the receipt stored under `receipt_key()` and bound to
/// `id`. `Ok(None)` when no receipt is stored.
#[allow(dead_code)] // wired by the PR6b-2 resume pipeline (W3+)
pub(crate) async fn load_outstanding_receipt(
    info: &Arc<dyn Store>,
    id: &[u8; 16],
) -> DbResult<Option<ReplicationReceipt>> {
    let bytes = match info.get(receipt_key()).await {
        Ok(bytes) => bytes,
        Err(DbError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error),
    };
    decode_receipt(id, &bytes).map(Some)
}

/// Remove the stored receipt (callers flush or ride an atomic batch).
#[allow(dead_code)] // direct-retire variant; the atomic path deletes in-batch
pub(crate) async fn delete_receipt(info: &Arc<dyn Store>) -> DbResult<()> {
    info.remove(receipt_key()).await.map(|_| ())
}
