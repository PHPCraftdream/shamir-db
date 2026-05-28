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
pub const WAL_V2_VERSION: u8 = 1;

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
    pub fn encode(&self) -> DbResult<Vec<u8>> {
        let body = bincode::serialize(self)
            .map_err(|e| DbError::Internal(format!("wal_v2 encode: {e}")))?;
        let mut out = Vec::with_capacity(4 + 1 + body.len());
        out.extend_from_slice(&WAL_V2_MAGIC);
        out.push(WAL_V2_VERSION);
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// Decode from `[magic][version][bincode body]`.
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
        if version != WAL_V2_VERSION {
            return Err(DbError::Internal(format!(
                "wal_v2 decode: unsupported version {version}"
            )));
        }
        bincode::deserialize(&bytes[5..])
            .map_err(|e| DbError::Internal(format!("wal_v2 decode: {e}")))
    }

    /// Returns true if these bytes start with the V2 magic prefix.
    /// Used by `WalManager` (stage 0.8) to dispatch between V1 and V2
    /// without fully decoding.
    pub fn looks_like_v2(bytes: &[u8]) -> bool {
        bytes.len() >= 5 && bytes[..4] == WAL_V2_MAGIC
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(n: u8) -> RecordId {
        let mut a = [0u8; 16];
        a[15] = n;
        RecordId(a)
    }

    fn sample_entry() -> WalEntryV2 {
        WalEntryV2 {
            txn_id: 42,
            repo_id_interned: 7,
            started_at_ns: 1_234_567_890,
            commit_version: 123,
            ops: vec![
                WalOpV2::Put {
                    table_id_interned: 7,
                    rid: rid(1),
                    body: Bytes::from_static(b"hello"),
                },
                WalOpV2::Delete {
                    table_id_interned: 7,
                    rid: rid(2),
                },
                WalOpV2::IndexPut {
                    table_id_interned: 7,
                    idx_id: 11,
                    key: Bytes::from_static(b"k"),
                    value: Bytes::from_static(b"v"),
                },
                WalOpV2::IndexDel {
                    table_id_interned: 7,
                    idx_id: 11,
                    key: Bytes::from_static(b"k2"),
                },
                WalOpV2::InternerOverlayMerge {
                    entries: vec![(100, "email".into()), (101, "score".into())],
                },
                WalOpV2::CounterDelta {
                    table_id_interned: 5,
                    delta: -3,
                },
            ],
        }
    }

    #[test]
    fn round_trip_all_op_variants() {
        let entry = sample_entry();
        let encoded = entry.encode().unwrap();
        let decoded = WalEntryV2::decode(&encoded).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn encode_has_magic_and_version() {
        let bytes = sample_entry().encode().unwrap();
        assert_eq!(&bytes[..4], &WAL_V2_MAGIC);
        assert_eq!(bytes[4], WAL_V2_VERSION);
    }

    #[test]
    fn decode_rejects_short_input() {
        assert!(WalEntryV2::decode(b"").is_err());
        assert!(WalEntryV2::decode(b"WAL").is_err());
        assert!(WalEntryV2::decode(b"WAL2").is_err());
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut bytes = sample_entry().encode().unwrap();
        bytes[0] = b'X';
        assert!(WalEntryV2::decode(&bytes).is_err());
    }

    #[test]
    fn decode_rejects_unknown_version() {
        let mut bytes = sample_entry().encode().unwrap();
        bytes[4] = 99;
        assert!(WalEntryV2::decode(&bytes).is_err());
    }

    #[test]
    fn looks_like_v2_sniff() {
        let bytes = sample_entry().encode().unwrap();
        assert!(WalEntryV2::looks_like_v2(&bytes));
        assert!(!WalEntryV2::looks_like_v2(b""));
        assert!(!WalEntryV2::looks_like_v2(b"SDB2\x01")); // wrong magic
        let v1_bytes = bincode::serialize(&"some v1 entry").unwrap();
        // V1 entries don't carry magic prefix — start with bincode varints.
        // Very unlikely to start with "WAL2".
        assert!(!WalEntryV2::looks_like_v2(&v1_bytes));
    }

    #[test]
    fn size_bound_on_large_batch() {
        // 100 small Put ops, each 50 bytes body — roughly 5KB raw.
        // Bincode adds per-field overhead (variant tag + length prefix).
        // Acceptance: encoded fits in 10KB.
        let ops: Vec<_> = (0..100u8)
            .map(|i| WalOpV2::Put {
                table_id_interned: 0,
                rid: rid(i),
                body: Bytes::from(vec![b'x'; 50]),
            })
            .collect();
        let entry = WalEntryV2::new(1, 0, ops);
        let encoded = entry.encode().unwrap();
        assert!(
            encoded.len() < 10240,
            "encoded size {} should be < 10KB",
            encoded.len()
        );
    }
}
