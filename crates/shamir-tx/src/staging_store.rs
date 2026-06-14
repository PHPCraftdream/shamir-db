//! In-memory write buffer for a single transaction.
//!
//! All writes go into a local `IndexMap` (Fx-hashed). Reads check the local
//! buffer first (serving staged writes / staged removes), then fall
//! through to the base `Store`.
//!
//! On commit: `drain()` returns `Vec<KvOp>` for an atomic
//! `base.transact(ops)` call. On abort: just drop the `StagingStore`.
//!
//! Single-writer-per-tx invariant: only the owning `TxContext` task
//! may call mutating methods. There is no concurrent access — sharding
//! and atomics from `scc::HashMap` added pure overhead with zero benefit.

use bytes::Bytes;
use shamir_collections::TMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::value::InnerValue;
use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

/// Wrapper for staged row payload. Initial implementation holds only
/// already-serialized Bytes; future cycle (18c) adds a Live(InnerValue)
/// variant for lazy serialization on the hot insert path.
/// `StagedRow` with `OnceLock`-based serialization cache for the `Live` variant.
///
/// `Clone` resets the cache so the cloned row re-serializes on first access —
/// this is correct because `OnceLock` is not `Clone`. Resetting is cheaper than
/// the alternative (serializing eagerly on every clone), and `StagedRow` clones
/// are rare (only in `StagedKind::Set` reads and tests).
#[derive(Debug)]
pub enum StagedRow {
    /// Already-serialized msgpack bytes.
    Bytes(Bytes),
    /// Decoded value. Serialization is deferred and cached via `OnceLock`.
    Live {
        inner: InnerValue,
        /// Cached msgpack encoding. Populated on the first `as_bytes()` call
        /// and reused for all subsequent calls — `OnceLock::get_or_init` is
        /// lock-free after initialization.
        encoded: OnceLock<Bytes>,
    },
}

impl Clone for StagedRow {
    fn clone(&self) -> Self {
        match self {
            StagedRow::Bytes(b) => StagedRow::Bytes(b.clone()),
            // Reset the cache: the clone is a fresh value.
            StagedRow::Live { inner, .. } => StagedRow::Live {
                inner: inner.clone(),
                encoded: OnceLock::new(),
            },
        }
    }
}

impl StagedRow {
    /// Serialize to msgpack Bytes. Identity for the Bytes variant; serializes
    /// once and caches for the Live variant.
    pub fn as_bytes(&self) -> Bytes {
        match self {
            StagedRow::Bytes(b) => b.clone(),
            StagedRow::Live { inner, encoded } => encoded
                .get_or_init(|| {
                    inner
                        .to_bytes()
                        .expect("InnerValue::to_bytes never fails on valid data")
                })
                .clone(),
        }
    }

    /// Exact serialized byte length.
    pub fn len_bytes(&self) -> usize {
        match self {
            StagedRow::Bytes(b) => b.len(),
            StagedRow::Live { .. } => self.as_bytes().len(),
        }
    }

    /// Borrow the decoded value. Live is zero-copy; Bytes deserializes.
    pub fn as_inner(&self) -> Cow<'_, InnerValue> {
        match self {
            StagedRow::Live { inner, .. } => Cow::Borrowed(inner),
            StagedRow::Bytes(b) => Cow::Owned(
                InnerValue::from_bytes(b).expect("StagedRow::Bytes always holds valid msgpack"),
            ),
        }
    }
}

#[derive(Debug, Clone)]
enum StagedOp {
    Set(StagedRow),
    Remove,
}

/// Result of a targeted per-key staging probe ([`StagingStore::staged_op`]).
///
/// Reports *only* what this tx has staged for the key, never touching the
/// base store: a staged set (`Set`), a staged remove (`Removed`), or — when
/// the variant is absent from the return — nothing staged for this key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedKind {
    /// The tx staged a value for this key (read-your-own-write).
    Set(Bytes),
    /// The tx staged a remove for this key (read-your-own-delete).
    Removed,
}

/// Per-transaction staging buffer with read-through semantics.
///
/// Created at tx begin, consumed at commit (via `drain`), or dropped
/// at abort. NOT `Clone` — ownership is single (the `TxContext`).
///
/// Uses `IndexMap<RecordKey, StagedOp, THasher>` (plain hash map, Fx hasher)
/// because the single-writer-per-tx invariant means `scc::HashMap`'s
/// sharding and CAS machinery added pure overhead with zero concurrency
/// benefit.
pub struct StagingStore {
    base: Arc<dyn Store>,
    writes: TMap<RecordKey, StagedOp>,
}

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self {
        Self {
            base,
            writes: shamir_collections::new_map(),
        }
    }

    /// Borrow the base store this staging buffer wraps.
    ///
    /// Used by `commit_tx` Phase 5 to apply drained ops via
    /// `base.transact(ops)` — atomic batch publish per table.
    pub fn base(&self) -> &Arc<dyn Store> {
        &self.base
    }

    /// Read-through: staged value first, then base store.
    /// Staged `Remove` returns `NotFound` even if base has the key.
    pub async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        if let Some(op) = self.writes.get(&k) {
            return match op {
                StagedOp::Set(row) => Ok(row.as_bytes()),
                StagedOp::Remove => Err(DbError::NotFound(format!("staged remove: {:?}", k))),
            };
        }
        self.base.get(k).await
    }

    /// Targeted, alloc-free probe of this tx's own staging for `key`.
    ///
    /// Unlike [`get`], this consults **only** the local staging map and
    /// never falls through to the base store, and it distinguishes a
    /// staged `Remove` ([`StagedKind::Removed`]) from "nothing staged"
    /// (`None`). It is the per-key counterpart of [`snapshot_ops`]: callers
    /// that need to overlay staging for a single key (e.g. point reads doing
    /// read-your-own-writes) use this instead of allocating + cloning the
    /// whole op vector and linearly scanning it.
    ///
    /// Returns:
    ///   - `Some(StagedKind::Set(bytes))` — the tx staged this value;
    ///   - `Some(StagedKind::Removed)`    — the tx staged a remove;
    ///   - `None`                         — the key is not staged in this tx
    ///     (caller should fall through to the snapshot base).
    pub fn staged_op(&self, key: &[u8]) -> Option<StagedKind> {
        self.writes.get(key as &[u8]).map(|v| match v {
            StagedOp::Set(row) => StagedKind::Set(row.as_bytes()),
            StagedOp::Remove => StagedKind::Removed,
        })
    }

    /// Stage a set (creates or overwrites).
    pub fn set(&mut self, k: RecordKey, v: Bytes) {
        self.writes.insert(k, StagedOp::Set(StagedRow::Bytes(v)));
    }

    /// Stage multiple sets in a single synchronous pass — no `.await` per key.
    ///
    /// Equivalent to calling `set(k, v)` for each `(k, v)` in `items`.
    pub fn set_many(&mut self, items: impl IntoIterator<Item = (RecordKey, Bytes)>) {
        for (k, v) in items {
            self.writes.insert(k, StagedOp::Set(StagedRow::Bytes(v)));
        }
    }

    /// Stage multiple `InnerValue` rows in a single synchronous pass (lazy
    /// serialization). msgpack encoding is deferred to commit Phase 4/5.
    /// Aborted txs skip encoding entirely.
    pub fn set_many_live(&mut self, items: impl IntoIterator<Item = (RecordKey, InnerValue)>) {
        for (k, v) in items {
            self.writes.insert(
                k,
                StagedOp::Set(StagedRow::Live {
                    inner: v,
                    encoded: OnceLock::new(),
                }),
            );
        }
    }

    /// Stage a remove.
    pub fn remove(&mut self, k: RecordKey) {
        self.writes.insert(k, StagedOp::Remove);
    }

    /// Snapshot of all staged ops without consuming.
    ///
    /// Used by `commit_tx` Phase 4 to emit data ops into the WAL
    /// entry, separate from Phase 5's `drain()` that actually applies
    /// them. Must be called under `RepoTxGate::commit_lock` — caller
    /// guarantees no concurrent writers.
    pub fn snapshot_ops(&self) -> Vec<KvOp> {
        self.writes
            .iter()
            .map(|(k, v)| match v {
                StagedOp::Set(row) => KvOp::Set(k.clone(), row.as_bytes()),
                StagedOp::Remove => KvOp::Remove(k.clone()),
            })
            .collect()
    }

    /// Drain all staged writes into a `Vec<KvOp>` suitable for
    /// `Store::transact`. Consumes `self`.
    ///
    /// The caller (TxContext commit phase) combines ops from all
    /// per-table StagingStores and feeds them to a single
    /// `store.transact(all_ops)` for atomic publish.
    pub fn drain(self) -> Vec<KvOp> {
        self.writes
            .into_iter()
            .map(|(k, v)| match v {
                StagedOp::Set(row) => KvOp::Set(k, row.as_bytes()),
                StagedOp::Remove => KvOp::Remove(k),
            })
            .collect()
    }

    /// Approximate in-memory byte footprint of all currently staged ops.
    ///
    /// `O(N)` over the staged keys. `Bytes::len()` is O(1), so each visit
    /// is constant work.
    ///
    /// Counts `key.len() + value.len()` for [`StagedOp::Set`] and `key.len()`
    /// for [`StagedOp::Remove`].
    pub fn staged_bytes(&self) -> usize {
        self.writes.iter().fold(0usize, |acc, (k, v)| match v {
            StagedOp::Set(row) => acc.saturating_add(k.len()).saturating_add(row.len_bytes()),
            StagedOp::Remove => acc.saturating_add(k.len()),
        })
    }

    /// Number of unique keys with staged writes.
    pub fn len(&self) -> usize {
        self.writes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// Iterate keys staged in this store (without cloning the values).
    pub fn keys(&self) -> impl Iterator<Item = &RecordKey> {
        self.writes.keys()
    }

    /// cancel-safe: NO — iterates staged keys then transforms each.
    /// Cancellation mid-iteration leaves a subset of staged values rewritten
    /// and others not, breaking the invariant that all overlay ids are remapped.
    /// Caller must abort the tx on cancellation (drop the StagingStore).
    ///
    /// Rewrite all staged `Set` values via a byte transform.
    ///
    /// Used by `TxContext::apply_id_remap` during commit phase 1 to
    /// replace overlay interner ids with stable base ids in staged
    /// record bytes before they reach `transact()`.
    pub async fn rewrite_set_bytes<F>(&mut self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&Bytes) -> Result<Bytes, String>,
    {
        for op in self.writes.values_mut() {
            if let StagedOp::Set(row) = op {
                let bytes = row.as_bytes();
                *row = StagedRow::Bytes(f(&bytes)?);
            }
        }
        Ok(())
    }

    /// cancel-safe: NO — same reasoning as [`rewrite_set_bytes`].
    ///
    /// Rewrite all staged `Set` values via an `InnerValue`-level transform.
    ///
    /// **Fast path for `Live` rows:** the closure receives the already-decoded
    /// `InnerValue` directly — no msgpack round-trip. `Bytes` rows are
    /// decoded once, transformed, and stored back as `Live` (so subsequent
    /// access also skips re-serialization until commit/WAL emit).
    ///
    /// Used by `TxContext::apply_id_remap` in place of [`rewrite_set_bytes`]
    /// to save one full msgpack deserialize + one full msgpack reserialize
    /// per `Live` row during the intern-id remap commit phase.
    pub async fn rewrite_set_inner<F>(&mut self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&mut InnerValue) -> Result<(), String>,
    {
        for op in self.writes.values_mut() {
            if let StagedOp::Set(row) = op {
                let mut inner = match row {
                    StagedRow::Live { inner, .. } => std::mem::replace(inner, InnerValue::Null),
                    StagedRow::Bytes(b) => {
                        InnerValue::from_bytes(b).map_err(|e| format!("remap decode: {e}"))?
                    }
                };
                f(&mut inner)?;
                // Reset encoded cache: inner was mutated by f.
                *row = StagedRow::Live {
                    inner,
                    encoded: OnceLock::new(),
                };
            }
        }
        Ok(())
    }
}
