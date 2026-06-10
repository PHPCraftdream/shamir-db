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
use std::sync::Arc;

#[derive(Debug, Clone)]
enum StagedOp {
    Set(Bytes),
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
                StagedOp::Set(b) => Ok(b),
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
            StagedOp::Set(b) => StagedKind::Set(b.clone()),
            StagedOp::Remove => StagedKind::Removed,
        })
    }

    /// Stage a set (creates or overwrites).
    ///
    /// cancel-safe: yes — `upsert_async` either completes the upsert or leaves
    /// the map unchanged on cancellation (CAS-based, no partial state).
    pub async fn set(&self, k: RecordKey, v: Bytes) {
        let _ = self.writes.upsert_async(k, StagedOp::Set(v)).await;
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
            StagedOp::Set(bytes) => ops.push(KvOp::Set(k.clone(), bytes.clone())),
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
            StagedOp::Set(bytes) => ops.push(KvOp::Set(k.clone(), bytes.clone())),
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
            StagedOp::Set(b) => total = total.saturating_add(k.len()).saturating_add(b.len()),
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
                    if let StagedOp::Set(bytes) = op {
                        match f(bytes) {
                            Ok(new_bytes) => *bytes = new_bytes,
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
