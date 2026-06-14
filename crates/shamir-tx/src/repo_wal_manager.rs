//! Repo-level WAL manager — one `WalEntryV2` per transaction/batch.
//!
//! Lives alongside the per-table [`shamir_wal::WalManager`] (V1 back-compat).
//! Transactional writes go through `RepoWalManager`; non-tx writes stay on V1.
//!
//! Shares the same [`WalActiveKey`] physical prefix (`__wal_active_`) — V1 and
//! V2 entries coexist in one keyspace, distinguished by value magic bytes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::error::DbResult;
use shamir_storage::types::Store;
use shamir_wal::{WalActiveKey, WalDurability, WalEntryV2, WalGroupCommit};

/// Repo-scoped WAL — one entry per tx/batch, covering all tables.
///
/// Markers live in the same `info_store` as per-table WAL (V1).
/// [`list_inflight`](Self::list_inflight) filters by V2 magic prefix,
/// skipping V1 entries silently.
pub struct RepoWalManager {
    info_store: Arc<dyn Store>,
    next_txn_id: AtomicU64,
    /// Optional file-backed group-commit coordinator. `Some` for
    /// disk-backed repos (constructed in `RepoInstance::repo_wal`), `None`
    /// for in-memory repos — in which case [`begin_grouped`](Self::begin_grouped)
    /// falls back to the KV-marker [`begin`](Self::begin) path. Wired but
    /// not yet driven by the live commit path (that cutover is W4/W5).
    group: Option<Arc<WalGroupCommit>>,
}

impl RepoWalManager {
    /// Create a new `RepoWalManager`.
    ///
    /// `initial_txn_id` is seeded from recovery:
    /// `max(persisted_next_tx_id, max_inflight_txn_id + 1)`.
    pub fn new(info_store: Arc<dyn Store>, initial_txn_id: u64) -> Self {
        Self {
            info_store,
            next_txn_id: AtomicU64::new(initial_txn_id),
            group: None,
        }
    }

    /// Like [`new`](Self::new), but attaches a file-backed
    /// [`WalGroupCommit`] so [`begin_grouped`](Self::begin_grouped) writes
    /// to the segment file instead of falling back to KV markers.
    pub fn new_with_group(
        info_store: Arc<dyn Store>,
        initial_txn_id: u64,
        group: Arc<WalGroupCommit>,
    ) -> Self {
        Self {
            info_store,
            next_txn_id: AtomicU64::new(initial_txn_id),
            group: Some(group),
        }
    }

    /// Allocate the next monotonic txn_id.
    pub fn fresh_txn_id(&self) -> u64 {
        self.next_txn_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Raise the txn_id floor to at least `floor`, returning the value the
    /// counter now sits at (`>= floor`).
    ///
    /// Recovery seam: the constructor seeds `next_txn_id` from the durable
    /// `NextTxId` snapshot, which is persisted only periodically and so can
    /// lag the txn_id of an *inflight* (uncommitted, uncleaned) WAL entry
    /// left by a crash. Re-seeding the counter solely from the stale
    /// snapshot would let [`fresh_txn_id`](Self::fresh_txn_id) hand out an
    /// id a crashed-and-about-to-be-recovered entry already used — two WAL
    /// entries sharing one txn_id (a diagnostic/correctness hazard, the
    /// txn_id mirror of the CRIT-B version-floor problem). The open path
    /// computes `max_inflight_txn_id + 1` and floors the counter here so
    /// every subsequently issued id is strictly greater than any inflight
    /// entry's. Monotonic `fetch_max` — idempotent and safe to call before
    /// any id is handed out.
    pub fn seed_floor_at_least(&self, floor: u64) -> u64 {
        let prev = self.next_txn_id.fetch_max(floor, Ordering::Relaxed);
        prev.max(floor)
    }

    /// cancel-safe: yes — single `info_store.set` after an in-memory
    /// encode. Cancellation either lands the marker durably (storage's
    /// own cancel-safety contract) or drops the future before the write.
    ///
    /// Write a V2 entry under its `WalActiveKey`.
    ///
    /// This is the intent marker — if a crash happens before
    /// [`commit`](Self::commit), recovery replays this entry.
    pub async fn begin(&self, entry: WalEntryV2) -> DbResult<()> {
        let encoded = entry.encode()?;
        self.info_store
            .set(WalActiveKey::new(entry.txn_id).to_bytes(), encoded.into())
            .await?;
        Ok(())
    }

    /// cancel-safe: yes — when a group is present, parks on the
    /// group-commit waiter (`WalGroupCommit::append`); cancellation drops
    /// the future. When absent, delegates to the cancel-safe
    /// [`begin`](Self::begin).
    ///
    /// Backend-agnostic WAL begin. When a file-backed [`WalGroupCommit`] is
    /// attached (disk repos), encodes `entry` and appends it to the segment
    /// at the requested durability tier. Otherwise falls back to the KV-marker
    /// [`begin`](Self::begin) path (in-memory repos).
    ///
    pub async fn begin_grouped(
        &self,
        entry: WalEntryV2,
        durability: WalDurability,
    ) -> DbResult<()> {
        match &self.group {
            Some(g) => {
                let encoded = entry.encode()?;
                g.append(encoded, durability).await
            }
            None => self.begin(entry).await,
        }
    }

    /// File-mode batch WAL begin. When a [`WalGroupCommit`] is attached
    /// (disk repos), appends each entry through the group at the requested
    /// tier (one append per entry — correctness over batching; this path is
    /// the non-hot AsyncIndex leader). Otherwise falls back to the KV
    /// [`begin_many`](Self::begin_many) (in-memory repos).
    pub async fn begin_grouped_many(
        &self,
        entries: &[WalEntryV2],
        durability: WalDurability,
    ) -> DbResult<()> {
        match &self.group {
            Some(g) => {
                for entry in entries {
                    let encoded = entry.encode()?;
                    g.append(encoded, durability).await?;
                }
                Ok(())
            }
            None => self.begin_many(entries).await,
        }
    }

    /// Force a durable `fsync` of the file WAL (level 2 → level 3). In file
    /// mode, syncs the group's sink; in non-file mode this is a no-op (the
    /// KV path is made durable by its own `flush`).
    pub async fn sync_wal(&self) -> DbResult<()> {
        match &self.group {
            Some(g) => g.sync_now().await,
            None => Ok(()),
        }
    }

    /// Batch-write N WAL entries in a single `set_many` + `flush`.
    ///
    /// Group-commit foundation: the caller collects entries from multiple
    /// concurrent transactions and lands them with one storage round-trip
    /// instead of N sequential `begin()` calls.
    ///
    /// Each entry is encoded independently — wire format is byte-identical
    /// to the single-entry [`begin`](Self::begin) path.
    pub async fn begin_many(&self, entries: &[WalEntryV2]) -> DbResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut items = Vec::with_capacity(entries.len());
        for entry in entries {
            let encoded = entry.encode()?;
            items.push((WalActiveKey::new(entry.txn_id).to_bytes(), encoded.into()));
        }
        self.info_store.set_many(items).await?;
        self.info_store.flush().await?;
        Ok(())
    }

    /// cancel-safe: yes — single `info_store.remove`. Idempotent: a
    /// cancelled remove either lands or doesn't; re-issuing it converges
    /// (a missing key is a no-op).
    ///
    /// Remove the marker — tx writes have landed durably. Idempotent.
    pub async fn commit(&self, txn_id: u64) -> DbResult<()> {
        // File mode: there are no per-entry markers to remove — entries live
        // in the segment until F6 truncation, and recovery replays them
        // idempotently. The KV-marker removal below applies only to the
        // in-memory (no group) path.
        if self.group.is_some() {
            return Ok(());
        }
        self.info_store
            .remove(WalActiveKey::new(txn_id).to_bytes())
            .await?;
        Ok(())
    }

    /// Fire-and-forget commit. Returns a [`tokio::task::JoinHandle`].
    ///
    /// NOTE: not currently used by the commit path — `commit_tx` removes
    /// the marker synchronously via [`commit`](Self::commit). Kept as the
    /// V2 mirror of [`shamir_wal::WalManager::commit_async`] for a future
    /// non-blocking commit option; presently exercised only by its unit
    /// test (`commit_async_removes_marker`).
    pub fn commit_async(&self, txn_id: u64) -> tokio::task::JoinHandle<DbResult<()>> {
        let info_store = self.info_store.clone();
        let key = WalActiveKey::new(txn_id).to_bytes();
        tokio::spawn(async move {
            info_store.remove(key).await?;
            Ok(())
        })
    }

    /// Replay-based recovery source (file WAL). Returns the entries from
    /// the group's sink, or falls back to the KV `list_inflight` when no
    /// group is attached. Additive — the live recovery path still uses
    /// `list_inflight` until F3.
    pub async fn recover(&self) -> DbResult<Vec<WalEntryV2>> {
        match &self.group {
            Some(g) => g.replay().await,
            None => self.list_inflight().await,
        }
    }

    /// cancel-safe: yes — read-only prefix stream over `info_store`.
    /// Cancellation drops the stream with no state mutation.
    ///
    /// List V2 entries that survived a crash (no commit marker removed).
    ///
    /// Scans all `WalActiveKey` entries, sniffs V2 magic, decodes.
    /// V1 entries (belonging to per-table `WalManager`) are skipped.
    pub async fn list_inflight(&self) -> DbResult<Vec<WalEntryV2>> {
        let mut out = Vec::new();
        let prefix = WalActiveKey::scan_prefix();
        let stream = self.info_store.scan_prefix_stream(prefix, 1024);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            for (_k, v) in batch? {
                if WalEntryV2::looks_like_v2(&v) {
                    out.push(WalEntryV2::decode(&v)?);
                }
            }
        }
        Ok(out)
    }
}
