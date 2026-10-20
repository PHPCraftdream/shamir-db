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
//!
//! §5b floor (#61): staged Set/Remove operate on id-keyed storage bytes —
//! recovery anchor. See `docs/dev-artifacts/perf/innervalue-floor.md` (Category 3).

use bytes::Bytes;
use shamir_collections::TMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::value::InnerValue;
use std::borrow::Cow;
use std::sync::Arc;

/// Serialized staged row payload — always holds already-encoded msgpack
/// `Bytes` (the W2c write-path cutover eliminated the `InnerValue` tree
/// from the insert path; every insert now encodes via
/// `query_value_to_storage_bytes` before staging).
///
/// `as_inner` decodes on demand (cold read-your-own-write / commit remap).
#[derive(Debug, Clone)]
pub struct StagedRow(Bytes);

impl StagedRow {
    /// Identity — return the held bytes.
    pub fn as_bytes(&self) -> Bytes {
        self.0.clone()
    }

    /// Exact serialized byte length.
    pub fn len_bytes(&self) -> usize {
        self.0.len()
    }

    /// Borrow the decoded value (always deserializes — no live variant).
    pub fn as_inner(&self) -> Cow<'_, InnerValue> {
        Cow::Owned(InnerValue::from_bytes(&self.0).expect("StagedRow always holds valid msgpack"))
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
    /// #1205 (A8 amortization): `true` once every `Set` staged into this
    /// store came from a `TableManager` write path that captured its
    /// `InternerKey` ids into `TxContext::referenced_interner_ids` at stage
    /// time (see that field's doc comment). `pre_commit_prelock`'s A8 scan
    /// trusts this flag to skip its decode-based fallback for this table.
    ///
    /// Starts `false` — a `StagingStore` populated directly (low-level
    /// tests, or any future raw-apply path that bypasses `TableManager`'s
    /// insert/update methods) is conservatively treated as uncaptured until
    /// [`mark_referenced_ids_captured`](Self::mark_referenced_ids_captured)
    /// proves otherwise, so A8's correctness never depends on every staging
    /// call site remembering to opt in.
    referenced_ids_captured: bool,
}

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self {
        Self {
            base,
            writes: shamir_collections::new_map(),
            referenced_ids_captured: false,
        }
    }

    /// Opt back in: mark this store's MOST RECENT `set`/`set_many` call as
    /// having had its `InternerKey` ids captured (by the caller) into
    /// `TxContext::referenced_interner_ids`.
    ///
    /// Must be called IMMEDIATELY after the `set`/`set_many` it vouches
    /// for — any staging call in between (this store has no other mutator
    /// per the single-writer-per-tx invariant) would silently reset the
    /// flag back to `false` first. Any call site that does NOT call this
    /// leaves A8 on its decode fallback for the whole table, which is
    /// always correct, just not free — so this must only be called when
    /// the caller is certain it captured every id.
    pub fn mark_referenced_ids_captured(&mut self) {
        self.referenced_ids_captured = true;
    }

    /// Whether every `Set` staged so far into this store had its
    /// referenced `InternerKey` ids captured at stage time (see
    /// [`mark_referenced_ids_captured`](Self::mark_referenced_ids_captured)).
    pub fn referenced_ids_captured(&self) -> bool {
        self.referenced_ids_captured
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
    ///
    /// #1205: resets [`referenced_ids_captured`](Self::referenced_ids_captured)
    /// to `false` — this plain path does not know whether the caller
    /// captured `v`'s referenced ids. A caller that DID capture them calls
    /// [`mark_referenced_ids_captured`](Self::mark_referenced_ids_captured)
    /// immediately afterward to opt back in; any staging call that skips
    /// that (a raw/uninstrumented caller, or a later plain `set`/`set_many`
    /// on the same store) conservatively lands A8 back on its decode
    /// fallback for the whole table.
    pub fn set(&mut self, k: RecordKey, v: Bytes) {
        self.writes.insert(k, StagedOp::Set(StagedRow(v)));
        self.referenced_ids_captured = false;
    }

    /// Stage multiple sets in a single synchronous pass — no `.await` per key.
    ///
    /// Equivalent to calling `set(k, v)` for each `(k, v)` in `items`,
    /// including the [`referenced_ids_captured`](Self::referenced_ids_captured)
    /// reset — see `set`'s doc.
    pub fn set_many(&mut self, items: impl IntoIterator<Item = (RecordKey, Bytes)>) {
        for (k, v) in items {
            self.writes.insert(k, StagedOp::Set(StagedRow(v)));
        }
        self.referenced_ids_captured = false;
    }

    /// Stage a remove.
    pub fn remove(&mut self, k: RecordKey) {
        self.writes.insert(k, StagedOp::Remove);
    }

    /// Snapshot of all staged ops without consuming.
    ///
    /// Used by `commit_tx` Phase 4 to emit data ops into the WAL
    /// entry, separate from Phase 5's `drain()` that actually applies
    /// them.
    ///
    /// # Calling contract
    ///
    /// The `StagingStore` is tx-scoped: only the owning `TxContext`
    /// mutates it, and the tx is single-threaded across its commit
    /// pipeline (the `&mut TxContext` borrow taken by `wal_ops_from_tx`
    /// statically excludes any other mutator). So no synchronisation is
    /// needed regardless of whether the caller holds `commit_lock`. In
    /// practice the Serializable branch of `commit_tx_lockfree` DOES
    /// hold `commit_lock` when `wal_ops_from_tx` runs (CRIT-4), and the
    /// Snapshot branch does NOT — but for THIS method the distinction
    /// is irrelevant because the borrow rules already provide the
    /// safety the old "must be called under commit_lock" comment
    /// claimed the lock provided.
    ///
    /// # When to prefer [`iter_ops`](Self::iter_ops)
    ///
    /// This materializes a fresh `Vec<KvOp>` on every call — `O(staged)`
    /// allocation + copy, paid in full even if the caller only wants to
    /// scan for a single match. A caller invoked once per staged-op set
    /// (Phase 4 WAL emission, changefeed projection) pays this once, which
    /// is fine. A caller invoked once PER RECORD being validated against
    /// the SAME staged set (e.g. a unique/FK probe run once per row of a
    /// batch insert) turns this `O(staged)` allocation into `O(staged²)`
    /// total work across the batch — use `iter_ops()` there instead, which
    /// yields lazily straight from the underlying map with no `Vec`
    /// allocation and lets `.any()`/`.find()` short-circuit without first
    /// materializing every staged op.
    pub fn snapshot_ops(&self) -> Vec<KvOp> {
        self.iter_ops().collect()
    }

    /// Borrowed, non-materializing iteration over all staged ops.
    ///
    /// Same logical content as [`snapshot_ops`](Self::snapshot_ops) (every
    /// staged key projected to a `KvOp`), but yields lazily from the
    /// underlying map instead of eagerly collecting into a `Vec`. `KvOp`'s
    /// payload types (`RecordKey`/`Bytes`) are cheap refcounted clones, so
    /// each yielded item still pays a per-item clone — what this avoids is
    /// the `Vec` allocation/copy and, critically, forcing every staged op
    /// to be visited before a short-circuiting caller (`.any()`, `.find()`)
    /// can return.
    ///
    /// Prefer this over `snapshot_ops()` for any probe invoked repeatedly
    /// against the same staging set (e.g. once per record validated in a
    /// batch) — see the perf note on `snapshot_ops` for why that pattern is
    /// quadratic when it collects a fresh `Vec` on every call.
    pub fn iter_ops(&self) -> impl Iterator<Item = KvOp> + '_ {
        self.writes.iter().map(|(k, v)| match v {
            StagedOp::Set(row) => KvOp::Set(k.clone(), row.as_bytes()),
            StagedOp::Remove => KvOp::Remove(k.clone()),
        })
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

    /// Iterate the bytes of every staged `Set` op (borrowed, no clone).
    ///
    /// #1205: `pre_commit_prelock`'s A8 scan uses this ONLY as a fallback
    /// for a table whose [`referenced_ids_captured`](Self::referenced_ids_captured)
    /// is `false` — i.e. bytes staged outside the instrumented
    /// `TableManager` write paths. The common case (every table staged via
    /// `TableManager` insert/update) never calls this.
    pub fn iter_set_bytes(&self) -> impl Iterator<Item = &Bytes> {
        self.writes.values().filter_map(|op| match op {
            StagedOp::Set(row) => Some(&row.0),
            StagedOp::Remove => None,
        })
    }

    /// cancel-safe: NO — iterates staged keys then transforms each.
    /// Cancellation mid-iteration leaves a subset of staged values rewritten
    /// and others not, breaking the invariant that all overlay ids are remapped.
    /// Caller must abort the tx on cancellation (drop the StagingStore).
    ///
    /// Rewrite all staged `Set` values via a byte transform.
    ///
    /// Used by `TxContext::apply_id_remap` and `pre_commit` Phase 1 to
    /// replace overlay interner ids with stable base ids in staged
    /// record bytes before they reach `transact()`.
    pub async fn rewrite_set_bytes<F>(&mut self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&Bytes) -> Result<Bytes, String>,
    {
        for op in self.writes.values_mut() {
            if let StagedOp::Set(row) = op {
                let bytes = row.as_bytes();
                *row = StagedRow(f(&bytes)?);
            }
        }
        Ok(())
    }
}
