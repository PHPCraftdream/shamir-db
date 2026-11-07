//! P0 index promote order (stage L3b) — type-erased promote-turnstile turn.
//!
//! A transaction appends its durable delta chunk (one per vector backend)
//! BEFORE publish and promotes into the in-RAM HNSW graph AFTER publish.
//! Without ordering, two transactions touching the same rid can promote
//! out of chunk order and the stale mutation wins. The turnstile lives on
//! each backend; this module only type-erases the per-chunk ticket so a
//! `TxContext` (this crate) can carry tickets for backends owned by
//! `shamir-index` without a dependency inversion.

use shamir_types::types::record_id::RecordId;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// One backend's live delta chunk inside the promote turnstile.
pub trait VectorPromoteTurn: Send + Sync {
    /// Block until every earlier-reserved chunk on this backend has finished
    /// its promote (or released its turn). Called BEFORE `enter_write`.
    fn wait_turn<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
    /// Apply this chunk's deletes + inserts as ONE ordered unit inside the
    /// turn (single write lease, compaction mirroring included).
    fn promote<'a>(
        &'a self,
        vecs: &'a [(RecordId, Vec<f32>)],
        deleted: &'a [RecordId],
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;
    /// Release the turn: finished-set insert + advance + notify. Sync so a
    /// ticket `Drop` can call it (covers failed promotes and cancelled
    /// commit futures). Releasing does NOT acknowledge the chunk — the
    /// prune prefix stays below it.
    fn release_turn(&self);
}

/// Single-owner Drop-guard ticket for one chunk's promote turn.
///
/// NOT `Clone` (RV finding 4): the ticket is the sole owner of its turn, so
/// `Drop` releases exactly once and an explicit [`Self::release_turn`]
/// consumes the turn (the `released` flag), making the later `Drop` a
/// no-op. State is two atomics, NO mutex (RV finding 5 — no lock may sit
/// per commit in the ticket): a `OnceLock` holds the turn (set exactly once
/// in [`Self::new`], read-only afterwards) and an `AtomicBool` `released`
/// flag consumed exactly once by `release_turn` via `swap`. A released
/// ticket's `promote` returns an ERROR ("promote ticket already released")
/// — NEVER a silent `Ok(())` (RV finding 1): a caller that retries a failed
/// promote with an already-released ticket must see the failure propagate,
/// not a phantom success that acknowledges a chunk whose adapter apply
/// never ran. `wait_turn` on a released ticket is a benign no-op (the flow
/// releases only AFTER the retry loop ends, so this is defensive-only).
pub struct VectorPromoteTicket {
    turn: OnceLock<Arc<dyn VectorPromoteTurn>>,
    released: AtomicBool,
}

impl VectorPromoteTicket {
    pub fn new(turn: impl VectorPromoteTurn + 'static) -> Self {
        let cell: OnceLock<Arc<dyn VectorPromoteTurn>> = OnceLock::new();
        let _ = cell.set(Arc::new(turn));
        Self {
            turn: cell,
            released: AtomicBool::new(false),
        }
    }

    /// Wait for this chunk's promote turn (ordered, no missed wakeups).
    /// A released ticket no-ops — unreachable in the engine flow (the turn
    /// is released only after the promote retry loop ends) — defensive only.
    pub async fn wait_turn(&self) {
        if self.released.load(Ordering::Acquire) {
            return;
        }
        if let Some(t) = self.turn.get() {
            t.wait_turn().await;
        }
    }

    /// Apply this chunk's deletes + inserts as one ordered unit in-turn.
    /// A released/consumed ticket returns an ERROR — never a silent success
    /// (RV finding 1: a retried promote on a consumed ticket must not be
    /// mistaken for a successful adapter apply and acknowledged).
    pub async fn promote(
        &self,
        vecs: &[(RecordId, Vec<f32>)],
        deleted: &[RecordId],
    ) -> Result<(), String> {
        if self.released.load(Ordering::Acquire) {
            return Err("promote ticket already released".into());
        }
        match self.turn.get() {
            Some(t) => t.promote(vecs, deleted).await,
            None => Err("promote ticket has no turn".into()),
        }
    }

    /// Release this chunk's turn NOW (RV finding 2: a tx must never hold an
    /// unreleased turn across a wait on another backend's turn). Consumes
    /// the turn (`released` flag); the later `Drop` is a no-op.
    pub fn release_turn(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            if let Some(t) = self.turn.get() {
                t.release_turn();
            }
        }
    }
}

impl std::ops::Drop for VectorPromoteTicket {
    fn drop(&mut self) {
        self.release_turn();
    }
}
