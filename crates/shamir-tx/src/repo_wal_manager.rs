//! Repo-level WAL manager — one `WalEntryV2` per transaction/batch.
//!
//! Every transactional write goes through a single path:
//! [`WalGroupCommit`] over a [`shamir_wal::WalSink`] (`File` for disk
//! repos, `Mem` for in-memory repos). There is no KV-marker fallback —
//! the group is always present.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_storage::error::DbResult;
use shamir_wal::{WalDurability, WalEntryV2, WalGroupCommit};

/// Repo-scoped WAL — one entry per tx/batch, covering all tables.
///
/// All writes funnel through [`WalGroupCommit`]; recovery replays the
/// group's sink ([`recover`](Self::recover)).
pub struct RepoWalManager {
    next_txn_id: AtomicU64,
    /// Group-commit coordinator over the repo's [`shamir_wal::WalSink`]
    /// (always present — `File` for disk repos, `Mem` for in-memory repos).
    group: Arc<WalGroupCommit>,
}

impl RepoWalManager {
    /// Create a new `RepoWalManager` over a [`WalGroupCommit`].
    ///
    /// `initial_txn_id` is seeded from recovery:
    /// `max(persisted_next_tx_id, max_inflight_txn_id + 1)`.
    pub fn new(initial_txn_id: u64, group: Arc<WalGroupCommit>) -> Self {
        Self {
            next_txn_id: AtomicU64::new(initial_txn_id),
            group,
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
    /// lag the txn_id of an *inflight* (uncommitted) WAL entry left by a
    /// crash. Re-seeding the counter solely from the stale snapshot would
    /// let [`fresh_txn_id`](Self::fresh_txn_id) hand out an id a
    /// crashed-and-about-to-be-recovered entry already used — two WAL
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

    /// # Cancel-safety — read carefully
    ///
    /// "cancel-safe: yes" in the narrow `tokio` sense: the future parks
    /// on the group-commit waiter and dropping it releases the wait
    /// without acquiring any lock — no resource leak, no poisoned state.
    ///
    /// BUT cancellation is NOT a semantic undo. Once
    /// [`WalGroupCommit::append`] has completed (the entry was encoded
    /// and appended to the segment, advancing `max_committed`), the
    /// entry is **durable in the WAL** and WILL be replayed by recovery
    /// on restart — even though the caller that was `.await`ing this
    /// future believes it was cancelled. The cancellation window is the
    /// park-on-the-waiter phase only: if the future is dropped BEFORE
    /// `append` returned Ok, the append may or may not have landed
    /// (the caller cannot tell). Mirrors the honest phrasing already on
    /// the `commit_tx` call path: a tx whose `begin_grouped` future is
    /// cancelled may resurrect as a committed tx on restart.
    ///
    /// Encode `entry` and append it to the group at the requested
    /// durability tier. Borrows the entry (mirroring `begin_grouped_many`)
    /// so callers holding an `Arc<WalEntryV2>` can serialize from the borrow
    /// instead of cloning the entry for every commit.
    pub async fn begin_grouped(
        &self,
        entry: &WalEntryV2,
        durability: WalDurability,
    ) -> DbResult<()> {
        let commit_version = entry.commit_version;
        let encoded = entry.encode()?;
        self.group.append(encoded, commit_version, durability).await
    }

    /// Batch WAL begin — appends each entry through the group at the
    /// requested tier (one append per entry; this path is the non-hot
    /// AsyncIndex leader, correctness over batching). Accepts any
    /// iterator of `&WalEntryV2` so callers holding `Arc<WalEntryV2>` can
    /// serialize from borrows instead of cloning into a `Vec`.
    pub async fn begin_grouped_many(
        &self,
        entries: impl IntoIterator<Item = &WalEntryV2>,
        durability: WalDurability,
    ) -> DbResult<()> {
        for entry in entries {
            let commit_version = entry.commit_version;
            let encoded = entry.encode()?;
            self.group
                .append(encoded, commit_version, durability)
                .await?;
        }
        Ok(())
    }

    /// Force a durable `fsync` of the WAL (level 2 → level 3). For the
    /// `Mem` sink this is a no-op.
    pub async fn sync_wal(&self) -> DbResult<()> {
        self.group.sync_now().await
    }

    /// Commit a transaction. No-op: there are no per-entry markers to
    /// remove — entries live in the segment until F6 truncation, and
    /// recovery replays them idempotently. Signature retained so
    /// `recover_inflight_v2` can call it in a loop.
    pub async fn commit(&self, _txn_id: u64) -> DbResult<()> {
        Ok(())
    }

    /// Replay-based recovery source: returns every entry from the group's
    /// sink (segment `replay()` in file mode, decoded frames in Mem mode).
    pub async fn recover(&self) -> DbResult<Vec<WalEntryV2>> {
        self.group.replay().await
    }

    /// F6b: truncate the WAL below `durable` — reclaim every record whose
    /// `commit_version` is in `(0, durable]` (deleted sealed segments for
    /// disk repos, dropped frames for Mem). Returns the count reclaimed.
    ///
    /// Truncation is by VERSION, not txn_id: the drainer advances
    /// `durable_watermark` as it replays entries into history, and the data
    /// in a sealed segment is durable iff its highest version is at or below
    /// that watermark (I1). The caller (drainer) must flush history before
    /// invoking this (I2). Replaces the old per-txn `commit` truncation —
    /// `commit(txn_id)` stays a no-op.
    pub async fn truncate_below(&self, durable: u64) -> DbResult<usize> {
        self.group.truncate_below(durable).await
    }

    /// F6b: cheap probe — is there anything truncatable at `durable`? The
    /// drainer gates the (relatively expensive) history-flush + truncate on
    /// this so that work fires only on a segment/frame boundary, never
    /// per-commit (I2).
    pub fn has_truncatable(&self, durable: u64) -> bool {
        self.group.has_truncatable(durable)
    }
}
