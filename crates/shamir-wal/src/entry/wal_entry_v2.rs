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
//! Frames are self-describing: every V2 entry is prefixed with the
//! `WAL2` magic, so recovery identifies a frame without decoding its body.
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

use bincode::Options;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use shamir_storage::error::{DbError, DbResult};
use shamir_types::types::record_id::RecordId;

use crate::event_header::WalEventHeader;

pub const WAL_V2_MAGIC: [u8; 4] = *b"WAL2";
/// Current version written by `encode`. Bumped from 2 to 3 to carry the
/// journal-event header (`event`, S5c-X I1): every journal event is a pure
/// function of the entry via `shamir_tx::event_from_wal_entry`.
/// (Version 2 had bumped from 1 to carry `interner_delta` — Stage A2.)
pub const WAL_V2_VERSION: u8 = 3;

/// Version 2 - decoded for backward compatibility (no `event` field;
/// carries `interner_delta`, Stage A2).
pub(crate) const WAL_V2_VERSION_PRE_EVENT: u8 = 2;

/// Version 1 - decoded for backward compatibility in both historical layouts.
/// Neither layout carries `interner_delta`.
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
/// same order the original commit pipeline did — `txn_id` is NOT a
/// safe proxy because `txn_id != commit_version`.
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
    /// Journal-event header (format version 3+). `None` for legacy
    /// entries (versions 1-2): no journal event can be derived from them
    /// (`shamir_tx::event_from_wal_entry` returns `None`; recovery treats
    /// the commit conservatively as "unknown").
    #[serde(default)]
    pub event: Option<WalEventHeader>,
}

/// Version 2 shape - no `event` field. Used only for decoding old entries.
#[derive(Debug, Deserialize)]
struct WalEntryV2PreEvent {
    pub txn_id: u64,
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    #[serde(default)]
    pub commit_version: u64,
    pub ops: Vec<WalOpV2>,
    #[serde(default)]
    pub interner_delta: Vec<(u64, String, u64)>,
}

/// Legacy v1 shape before HIGH-5 added commit_version.
#[derive(Debug, Deserialize)]
struct WalEntryV2LegacyPreHigh5 {
    pub txn_id: u64,
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    pub ops: Vec<WalOpV2>,
}

/// Legacy v1 shape after HIGH-5 and before interner_delta.
#[derive(Debug, Deserialize)]
struct WalEntryV2LegacyPostHigh5 {
    pub txn_id: u64,
    pub repo_id_interned: u64,
    pub started_at_ns: u64,
    pub commit_version: u64,
    pub ops: Vec<WalOpV2>,
}

fn decode_legacy_v1(bytes: &[u8]) -> DbResult<WalEntryV2> {
    let options = || {
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
    };
    let pre = options().deserialize::<WalEntryV2LegacyPreHigh5>(bytes);
    let post = options().deserialize::<WalEntryV2LegacyPostHigh5>(bytes);
    match (pre, post) {
        (Ok(old), Err(_)) => Ok(WalEntryV2 {
            txn_id: old.txn_id,
            repo_id_interned: old.repo_id_interned,
            started_at_ns: old.started_at_ns,
            commit_version: 0,
            ops: old.ops,
            interner_delta: vec![],
            event: None,
        }),
        (Err(_), Ok(new)) => Ok(WalEntryV2 {
            txn_id: new.txn_id,
            repo_id_interned: new.repo_id_interned,
            started_at_ns: new.started_at_ns,
            commit_version: new.commit_version,
            ops: new.ops,
            interner_delta: vec![],
            event: None,
        }),
        (Ok(_), Ok(_)) => Err(DbError::Codec(
            "wal_v2 decode v1: ambiguous legacy shape".into(),
        )),
        (Err(old), Err(new)) => Err(DbError::Codec(format!("wal_v2 decode v1: {old}; {new}"))),
    }
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
            event: None,
        }
    }

    /// Builder-style setter for the journal-event header (format version
    /// 3+). The commit pipeline stamps this BEFORE persisting the entry so
    /// the journal event stays derivable from the WAL alone (S5c-X I1).
    pub fn with_event_header(mut self, event: WalEventHeader) -> Self {
        self.event = Some(event);
        self
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
    /// The output is a complete encoded entry; validating only its header does not validate its body.
    pub fn encode(&self) -> DbResult<Vec<u8>> {
        // 5-byte header + bincode body.  Start with a reasonable
        // capacity guess; bincode will grow if needed but one alloc
        // is the common case.
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&WAL_V2_MAGIC);
        out.push(WAL_V2_VERSION);
        bincode::serialize_into(&mut out, self)
            .map_err(|e| DbError::Codec(format!("wal_v2 encode: {e}")))?;
        Ok(out)
    }

    /// Validate only `[magic][version]`; the body is not inspected.
    pub fn validate_frame_header(bytes: &[u8]) -> DbResult<()> {
        if bytes.len() < 5 {
            return Err(DbError::Internal("wal_v2 header: too short".into()));
        }
        if bytes[..4] != WAL_V2_MAGIC {
            return Err(DbError::Internal(format!(
                "wal_v2 header: bad magic {:?}",
                &bytes[..4]
            )));
        }
        match bytes[4] {
            WAL_V2_VERSION_LEGACY | WAL_V2_VERSION_PRE_EVENT | WAL_V2_VERSION => Ok(()),
            version => Err(DbError::Internal(format!(
                "wal_v2 header: unsupported version {version}"
            ))),
        }
    }

    /// Decode from `[magic][version][bincode body]`.
    ///
    /// Supports both historical version-1 layouts: pre-HIGH-5 (without
    /// `commit_version`) and post-HIGH-5 (with it), plus version 2 and 3.
    /// Trailing bytes are rejected for version 1; versions 2 and 3 retain
    /// their historical trailing-byte behavior.
    pub fn decode(bytes: &[u8]) -> DbResult<Self> {
        if bytes.len() < 5 {
            return Err(DbError::Codec("wal_v2 decode: too short".into()));
        }
        if bytes[..4] != WAL_V2_MAGIC {
            return Err(DbError::Codec(format!(
                "wal_v2 decode: bad magic {:?}",
                &bytes[..4]
            )));
        }
        let version = bytes[4];
        match version {
            WAL_V2_VERSION_LEGACY => decode_legacy_v1(&bytes[5..]),
            WAL_V2_VERSION_PRE_EVENT => {
                let pre: WalEntryV2PreEvent = bincode::deserialize(&bytes[5..])
                    .map_err(|e| DbError::Codec(format!("wal_v2 decode v2: {e}")))?;
                Ok(Self {
                    txn_id: pre.txn_id,
                    repo_id_interned: pre.repo_id_interned,
                    started_at_ns: pre.started_at_ns,
                    commit_version: pre.commit_version,
                    ops: pre.ops,
                    interner_delta: pre.interner_delta,
                    event: None,
                })
            }
            WAL_V2_VERSION => bincode::deserialize(&bytes[5..])
                .map_err(|e| DbError::Codec(format!("wal_v2 decode v3: {e}"))),
            _ => Err(DbError::Codec(format!(
                "wal_v2 decode: unsupported version {version}"
            ))),
        }
    }
}
