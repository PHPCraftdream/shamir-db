//! Transactional WAL entry format (V2).
//!
//! Coexists with the V1 [`super::wal_entry::WalEntry`] used by the
//! non-transactional write path. V1 records only `record_id` and
//! relies on the data_store as the source of truth for the actual
//! bytes; that works only if data_store was successfully updated
//! before the crash.
//!
//! V2 carries **inline body** for every mutating op so MVCC recovery
//! can replay uncommitted transactions after a crash where the
//! data_store update never landed. The body bytes are the exact
//! payload the tx intended to write.
//!
//! Both V1 and V2 entries live under the same `WalActiveKey` prefix
//! in info_store; recovery distinguishes them by sniffing the
//! magic prefix on each value (stage 0.8 will wire this).
//!
//! ## Envelope
//!
//! V2 entries are wrapped in a tiny inline envelope so future schema
//! migrations can dispatch on `version`:
//!
//!   [magic: 4 bytes "WAL2"] [version: u8] [bincode body...]
//!
//! Yes, this is the same shape as `shamir_engine::meta::MetaEnvelope`.
//! We do not depend on shamir-engine from shamir-wal (engine already
//! depends on us); inlining 10 lines of magic-byte handling is
//! cleaner than reverse-extracting MetaEnvelope into shamir-types.
//! See architectural-decisions.md if we revisit.

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;

pub const WAL_V2_MAGIC: [u8; 4] = *b"WAL2";
/// Current version written by `encode`. Bumped from 1 → 2 to carry
/// `interner_delta` (WAL v3 scaffolding, Stage A2).
pub const WAL_V2_VERSION: u8 = 2;

/// Previous version — decoded for backward compatibility.
pub(crate) const WAL_V2_VERSION_LEGACY: u8 = 1;

/// One mutating operation buffered inside a transactional WAL entry.
///
/// Each variant carries the full payload required for forward-fix
/// recovery — no read from data_store / info_store needed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalOpV2 {
    /// Insert or overwrite a record. `body` is the exact bytes
    /// the data_store should hold after this op is applied.
    Put {
        /// Interned identifier of the table this rid belongs to.
        /// Recovery resolves the target data_store via this token
        /// (RepoInstance::per_table_mvcc or similar lookup).
        table_id_interned: u64,
        rid: RecordId,
        #[serde(with = "serde_bytes_bytes")]
        body: Bytes,
    },

    /// Delete a record.
    Delete {
        /// Interned identifier of the table — see [`Put::table_id_interned`].
        table_id_interned: u64,
        rid: RecordId,
    },

    /// Insert/overwrite an index posting under `(idx_id, key)`.
    ///
    /// **`idx_id` invariant (Stage 4):** currently emitted as `0` by
    /// `commit_tx`'s `wal_ops_from_tx` because `shamir_tx::IndexWriteOp`
    /// (the pure-data type) doesn't carry index identity. Recovery
    /// code (Stage 7) can decode `idx_id` from the `key` byte prefix:
    /// posting keys are layout `[idx_id_be: 4 bytes][rest_of_key]`.
    /// Stage 5 reconciliation may either:
    ///   (a) thread `idx_id` through `IndexWriteOp` and emit it here,
    ///   (b) keep this field as 0 and rely on key-prefix decode.
    /// Decision deferred to the recovery implementation.
    IndexPut {
        /// Interned table identifier so recovery can resolve the
        /// table's info_store. Indexes live alongside data — per-table
        /// info_store hosts all postings.
        table_id_interned: u64,
        idx_id: u32,
        #[serde(with = "serde_bytes_bytes")]
        key: Bytes,
        #[serde(with = "serde_bytes_bytes")]
        value: Bytes,
    },

    /// Remove an index posting.
    ///
    /// Same `idx_id` invariant as [`IndexPut`](WalOpV2::IndexPut).
    IndexDel {
        table_id_interned: u64,
        idx_id: u32,
        #[serde(with = "serde_bytes_bytes")]
        key: Bytes,
    },

    /// Merge tx-local interner overlay entries into the base
    /// interner. `(id, name)` tuples committed under the same
    /// id-namespace the tx allocated against.
    InternerOverlayMerge { entries: Vec<(u64, String)> },

    /// Net counter delta to apply (e.g. +N for batch insert, -K for
    /// batch delete). Per (interned) table id.
    CounterDelta { table_id_interned: u64, delta: i64 },
}

/// Helper that serializes `bytes::Bytes` through serde_bytes
/// (because `Bytes` does not impl Serialize directly).
mod serde_bytes_bytes {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(b.as_ref()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let bb = serde_bytes::ByteBuf::deserialize(d)?;
        Ok(Bytes::from(bb.into_vec()))
    }
}

/// One transactional WAL entry — a list of `WalOpV2`s that must
/// be applied atomically on recovery.
///
/// `commit_version` carries the MVCC commit version this tx was
/// assigned in Phase 3 of `commit_tx`. Recovery sorts inflight
/// entries by this field so multi-tx replay applies them in the
/// same order the original commit pipeline did — `txn_id` (the
/// `WalActiveKey` byte order) is NOT a safe proxy because
/// `txn_id != commit_version`.
///
/// Entries authored before Stage 7.1's HIGH-5 fix carry
/// `commit_version = 0`; mixed-version corpora sort the legacy
/// entries first which preserves the lexical-key behaviour
/// callers had under the previous scheme.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalEntryV2 {
    pub txn_id: u64,
    /// Interned repo identifier — same `Interner` that names the
    /// fields in `WalOpV2::Put.body`.
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    /// MVCC commit version assigned in commit Phase 3. Zero when
    /// unset (legacy entries / hand-built test fixtures that don't
    /// care about replay order).
    #[serde(default)]
    pub commit_version: u64,
    pub ops: Vec<WalOpV2>,
    /// Interner delta: `(table_token, field_name, intern_id)` triples
    /// added to the base interner during this tx commit. Empty for
    /// legacy (version 1) entries and for entries authored before A3
    /// plumbs the delta from `commit_interner_overlay`.
    #[serde(default)]
    pub interner_delta: Vec<(u64, String, u64)>,
}

/// Legacy shape (version 1) — no `interner_delta` field. Used only for
/// decoding old WAL entries.
#[derive(Debug, Deserialize)]
struct WalEntryV2Legacy {
    pub txn_id: u64,
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    #[serde(default)]
    pub commit_version: u64,
    pub ops: Vec<WalOpV2>,
}

impl WalEntryV2 {
    /// Construct an entry with `commit_version = 0` — callers that
    /// know their version (the commit pipeline) should set it via
    /// [`with_commit_version`](Self::with_commit_version) or
    /// assign the public field after construction. Recovery does
    /// not require `commit_version != 0` but replay order is
    /// undefined across legacy and newly-versioned entries when
    /// both kinds are mixed.
    pub fn new(txn_id: u64, repo_id_interned: u64, ops: Vec<WalOpV2>) -> Self {
        let started_at_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            txn_id,
            repo_id_interned,
            started_at_ns,
            commit_version: 0,
            ops,
            interner_delta: vec![],
        }
    }

    /// Builder-style setter for `commit_version`. Used by the
    /// commit pipeline to stamp the MVCC version assigned in
    /// Phase 3 onto the entry before
    /// [`shamir_tx::RepoWalManager::begin`] persists it.
    pub fn with_commit_version(mut self, commit_version: u64) -> Self {
        self.commit_version = commit_version;
        self
    }

    /// Encode as `[magic][version][bincode body]`.
    ///
    /// Writes the 5-byte header then serialises directly into the same
    /// buffer via `bincode::serialize_into`, avoiding the intermediate
    /// `Vec` that `bincode::serialize` would allocate.
    pub fn encode(&self) -> DbResult<Vec<u8>> {
        // 5-byte header + bincode body.  Start with a reasonable
        // capacity guess; bincode will grow if needed but one alloc
        // is the common case.
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&WAL_V2_MAGIC);
        out.push(WAL_V2_VERSION);
        bincode::serialize_into(&mut out, self)
            .map_err(|e| DbError::Internal(format!("wal_v2 encode: {e}")))?;
        Ok(out)
    }

    /// Decode from `[magic][version][bincode body]`.
    ///
    /// Supports version 1 (legacy, no interner_delta) and version 2
    /// (current, with interner_delta). Unknown versions are rejected.
    pub fn decode(bytes: &[u8]) -> DbResult<Self> {
        if bytes.len() < 5 {
            return Err(DbError::Internal("wal_v2 decode: too short".into()));
        }
        if bytes[..4] != WAL_V2_MAGIC {
            return Err(DbError::Internal(format!(
                "wal_v2 decode: bad magic {:?}",
                &bytes[..4]
            )));
        }
        let version = bytes[4];
        match version {
            WAL_V2_VERSION_LEGACY => {
                let legacy: WalEntryV2Legacy = bincode::deserialize(&bytes[5..])
                    .map_err(|e| DbError::Internal(format!("wal_v2 decode v1: {e}")))?;
                Ok(Self {
                    txn_id: legacy.txn_id,
                    repo_id_interned: legacy.repo_id_interned,
                    started_at_ns: legacy.started_at_ns,
                    commit_version: legacy.commit_version,
                    ops: legacy.ops,
                    interner_delta: vec![],
                })
            }
            WAL_V2_VERSION => bincode::deserialize(&bytes[5..])
                .map_err(|e| DbError::Internal(format!("wal_v2 decode v2: {e}"))),
            _ => Err(DbError::Internal(format!(
                "wal_v2 decode: unsupported version {version}"
            ))),
        }
    }

    /// Returns true if these bytes start with the V2 magic prefix.
    /// Used by `WalManager` (stage 0.8) to dispatch between V1 and V2
    /// without fully decoding.
    pub fn looks_like_v2(bytes: &[u8]) -> bool {
        bytes.len() >= 5 && bytes[..4] == WAL_V2_MAGIC
    }
}
