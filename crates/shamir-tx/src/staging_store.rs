//! In-memory write buffer for a single transaction.
//!
//! All writes go into a local `scc::HashMap`. Reads check the local
//! buffer first (serving staged writes / staged removes), then fall
//! through to the base `Store`.
//!
//! On commit: `drain()` returns `Vec<KvOp>` for an atomic
//! `base.transact(ops)` call. On abort: just drop the `StagingStore`.

use bytes::Bytes;
use scc::HashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::value::InnerValue;
use std::borrow::Cow;
use std::sync::Arc;

/// Wrapper for staged row payload. Initial implementation holds only
/// already-serialized Bytes; future cycle (18c) adds a Live(InnerValue)
/// variant for lazy serialization on the hot insert path.
#[derive(Debug, Clone)]
pub enum StagedRow {
    /// Already-serialized msgpack bytes.
    Bytes(Bytes),
    /// Decoded value. Serialization deferred until commit/WAL emit.
    Live(InnerValue),
}

impl StagedRow {
    /// Serialize to msgpack Bytes. Identity for the Bytes variant; allocates
    /// for the Live variant.
    pub fn as_bytes(&self) -> Bytes {
        match self {
            StagedRow::Bytes(b) => b.clone(),
            StagedRow::Live(v) => v
                .to_bytes()
                .expect("InnerValue::to_bytes never fails on valid data"),
        }
    }

    /// Exact serialized byte length.
    pub fn len_bytes(&self) -> usize {
        match self {
            StagedRow::Bytes(b) => b.len(),
            StagedRow::Live(_) => self.as_bytes().len(),
        }
    }

    /// Borrow the decoded value. Live is zero-copy; Bytes deserializes.
    pub fn as_inner(&self) -> Cow<'_, InnerValue> {
        match self {
            StagedRow::Live(v) => Cow::Borrowed(v),
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
pub struct StagingStore {
    base: Arc<dyn Store>,
    writes: HashMap<RecordKey, StagedOp>,
}

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self {
        Self {
            base,
            writes: HashMap::new(),
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
    ///
    /// cancel-safe: yes — single `.await` per branch; `scc::HashMap::read_async`
    /// is cancel-safe (no partial state on drop) and `base.get` may not be,
    /// but cancellation there leaves no local state modified.
    pub async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        if let Some(op) = self.writes.read_async(&k, |_, v| v.clone()).await {
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
    ///
    /// Alloc-free probe: `key` is borrowed as `&[u8]` and matched directly
    /// against the `Bytes`-keyed map via `scc::HashMap::read`. scc 2.x's
    /// `read<Q>` is bounded by `Q: Equivalent<K> + Hash`, and scc ships the
    /// blanket `impl<Q, K> Equivalent<K> for Q where Q: Eq, K: Borrow<Q>`;
    /// since `bytes::Bytes: Borrow<[u8]>` and `[u8]: Eq`, `&[u8]` is an
    /// accepted probe key, and `<Bytes as Hash>` is byte-identical to
    /// `<[u8] as Hash>`, so the lookup hash lines up for any key length. No
    /// `Bytes` is allocated to probe (mirrors the III.2 `current_version`
    /// fix); the only allocation is the O(1) refcount bump when cloning the
    /// staged `Bytes` into a returned `Set`. Probe key length is arbitrary —
    /// not restricted to 16-byte record ids.
    pub fn staged_op(&self, key: &[u8]) -> Option<StagedKind> {
        self.writes.read(key, |_, v| match v {
            StagedOp::Set(row) => StagedKind::Set(row.as_bytes()),
            StagedOp::Remove => StagedKind::Removed,
        })
    }

    /// Stage a set (creates or overwrites).
    ///
    /// cancel-safe: yes — `upsert_async` either completes the upsert or leaves
    /// the map unchanged on cancellation (CAS-based, no partial state).
    pub async fn set(&self, k: RecordKey, v: Bytes) {
        let _ = self
            .writes
            .upsert_async(k, StagedOp::Set(StagedRow::Bytes(v)))
            .await;
    }

    /// Stage multiple sets in a single synchronous pass — no `.await` per key.
    ///
    /// Equivalent to calling `set(k, v).await` for each `(k, v)` in `items`,
    /// but avoids the async-fn scheduling overhead on every entry.
    /// Uses `scc::HashMap::upsert` (sync, CAS-based) which is safe because
    /// the StagingStore is per-tx and only the owning task writes to it.
    pub fn set_many(&self, items: impl IntoIterator<Item = (RecordKey, Bytes)>) {
        for (k, v) in items {
            self.writes.upsert(k, StagedOp::Set(StagedRow::Bytes(v)));
        }
    }

    /// Stage multiple `InnerValue` rows in a single synchronous pass (lazy
    /// serialization). msgpack encoding is deferred to commit Phase 4/5.
    /// Aborted txs skip encoding entirely.
    pub fn set_many_live(&self, items: impl IntoIterator<Item = (RecordKey, InnerValue)>) {
        for (k, v) in items {
            self.writes.upsert(k, StagedOp::Set(StagedRow::Live(v)));
        }
    }

    /// Stage a remove.
    ///
    /// cancel-safe: yes — same reasoning as `set`.
    pub async fn remove(&self, k: RecordKey) {
        let _ = self.writes.upsert_async(k, StagedOp::Remove).await;
    }

    /// Snapshot of all staged ops without consuming.
    ///
    /// Used by `commit_tx` Phase 4 to emit data ops into the WAL
    /// entry, separate from Phase 5's `drain()` that actually applies
    /// them. Must be called under `RepoTxGate::commit_lock` — caller
    /// guarantees no concurrent writers.
    pub fn snapshot_ops(&self) -> Vec<KvOp> {
        let mut ops = Vec::new();
        self.writes.scan(|k, v| match v {
            StagedOp::Set(row) => ops.push(KvOp::Set(k.clone(), row.as_bytes())),
            StagedOp::Remove => ops.push(KvOp::Remove(k.clone())),
        });
        ops
    }

    /// Drain all staged writes into a `Vec<KvOp>` suitable for
    /// `Store::transact`. Consumes `self`.
    ///
    /// The caller (TxContext commit phase) combines ops from all
    /// per-table StagingStores and feeds them to a single
    /// `store.transact(all_ops)` for atomic publish.
    pub fn drain(self) -> Vec<KvOp> {
        let mut ops = Vec::new();
        // scc::HashMap::scan is synchronous — closure receives (&K, &V).
        self.writes.scan(|k, v| match v {
            StagedOp::Set(row) => ops.push(KvOp::Set(k.clone(), row.as_bytes())),
            StagedOp::Remove => ops.push(KvOp::Remove(k.clone())),
        });
        ops
    }

    /// Approximate in-memory byte footprint of all currently staged ops.
    ///
    /// `O(N)` over the staged keys via a sync `scc::HashMap::scan` (the same
    /// pattern as [`snapshot_ops`] and [`len`]). `Bytes::len()` is O(1), so
    /// each visit is constant work.
    ///
    /// Counts `key.len() + value.len()` for [`StagedOp::Set`] and `key.len()`
    /// for [`StagedOp::Remove`]. Used by the server-side per-tx staging
    /// budget enforced on each `TxExecute` (Phase B Stage 8) to abort a tx
    /// with `tx_too_large` before its WAL entry — built from these very bytes
    /// in `wal_ops_from_tx` — grows unboundedly.
    ///
    /// Note: this measures the in-memory staging footprint, not the eventual
    /// `WalEntryV2` serialized length (which adds variant tags, lengths,
    /// interner overlay entries, counter deltas). The cap will trip somewhat
    /// earlier than the actual on-disk WAL size — fine for a protective
    /// budget, not equivalent to a WAL-byte cap.
    pub fn staged_bytes(&self) -> usize {
        let mut total: usize = 0;
        self.writes.scan(|k, v| match v {
            StagedOp::Set(row) => {
                total = total
                    .saturating_add(k.len())
                    .saturating_add(row.len_bytes())
            }
            StagedOp::Remove => total = total.saturating_add(k.len()),
        });
        total
    }

    /// Number of unique keys with staged writes.
    pub fn len(&self) -> usize {
        self.writes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// cancel-safe: NO — iterates the staged keys (snapshot via
    /// `scan_async`) then invokes `update_async` per key. Cancellation
    /// mid-iteration leaves some staged values rewritten and others not,
    /// breaking the invariant that all overlay ids are remapped. Caller
    /// must abort the tx on cancellation (drop the StagingStore).
    ///
    /// Rewrite all staged `Set` values via a byte transform.
    ///
    /// Used by `TxContext::apply_id_remap` during commit phase 1 to
    /// replace overlay interner ids with stable base ids in staged
    /// record bytes before they reach `transact()`.
    pub async fn rewrite_set_bytes<F>(&self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&Bytes) -> Result<Bytes, String>,
    {
        let keys: Vec<RecordKey> = {
            let mut out = Vec::new();
            self.writes.scan_async(|k, _v| out.push(k.clone())).await;
            out
        };
        for k in keys {
            let mut err: Option<String> = None;
            self.writes
                .update_async(&k, |_kk, op| {
                    if let StagedOp::Set(row) = op {
                        let bytes = row.as_bytes();
                        match f(&bytes) {
                            Ok(new_bytes) => *row = StagedRow::Bytes(new_bytes),
                            Err(e) => err = Some(e),
                        }
                    }
                })
                .await;
            if let Some(e) = err {
                return Err(e);
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
    pub async fn rewrite_set_inner<F>(&self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&mut InnerValue) -> Result<(), String>,
    {
        let keys: Vec<RecordKey> = {
            let mut out = Vec::new();
            self.writes.scan_async(|k, _v| out.push(k.clone())).await;
            out
        };
        for k in keys {
            let mut err: Option<String> = None;
            self.writes
                .update_async(&k, |_kk, op| {
                    if let StagedOp::Set(row) = op {
                        // Decode only if needed; Live rows skip this.
                        let mut inner = match row {
                            StagedRow::Live(v) => {
                                // Take value out temporarily; replaced below.
                                std::mem::replace(v, InnerValue::Null)
                            }
                            StagedRow::Bytes(b) => match InnerValue::from_bytes(b) {
                                Ok(v) => v,
                                Err(e) => {
                                    err = Some(format!("remap decode: {e}"));
                                    return;
                                }
                            },
                        };
                        match f(&mut inner) {
                            Ok(()) => *row = StagedRow::Live(inner),
                            Err(e) => err = Some(e),
                        }
                    }
                })
                .await;
            if let Some(e) = err {
                return Err(e);
            }
        }
        Ok(())
    }
}
