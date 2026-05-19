//! Single-writer, snapshot-reader actor base for stateful index backends.
//!
//! Pattern: writes go through a `tokio::sync::mpsc::UnboundedSender`
//! (lock-free MPSC). A spawned tokio task drains the channel and
//! applies ops sequentially, updating an `ArcSwap<Snap>` snapshot.
//! Readers grab the current snapshot with one atomic load — no locks.
//!
//! Used by FTS / Vector backends (Phase 2 / Phase 4). The Btree
//! backend is stateless on top of `Store` and doesn't need an actor.

use arc_swap::ArcSwap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct IndexActor<Op: Send + 'static, Snap: Send + Sync + 'static> {
    write_tx: mpsc::UnboundedSender<Op>,
    snapshot: Arc<ArcSwap<Snap>>,
    join: Option<JoinHandle<()>>,
}

impl<Op: Send + 'static, Snap: Send + Sync + 'static> IndexActor<Op, Snap> {
    /// Spawn an actor task. `applier` is invoked once per submitted
    /// `Op`; it receives the current snapshot handle so it can
    /// replace it atomically via `snapshot.store(Arc::new(new))`.
    pub fn spawn<F, Fut>(initial: Snap, mut applier: F) -> Self
    where
        F: FnMut(Op, Arc<ArcSwap<Snap>>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let snapshot = Arc::new(ArcSwap::from(Arc::new(initial)));
        let (write_tx, mut rx) = mpsc::unbounded_channel::<Op>();
        let snap_for_task = snapshot.clone();
        let join = tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                applier(op, snap_for_task.clone()).await;
            }
        });
        Self {
            write_tx,
            snapshot,
            join: Some(join),
        }
    }

    /// Submit a write op. Returns `Err` only if the actor task has
    /// already stopped (channel closed).
    pub fn submit(&self, op: Op) -> Result<(), mpsc::error::SendError<Op>> {
        self.write_tx.send(op)
    }

    /// Lock-free snapshot read.
    pub fn snapshot(&self) -> Arc<Snap> {
        self.snapshot.load_full()
    }

    /// Replace the snapshot directly (escape hatch for tests / cold
    /// restore). Most code paths should mutate via `submit`.
    pub fn replace_snapshot(&self, new_snap: Snap) {
        self.snapshot.store(Arc::new(new_snap));
    }

    /// Stop accepting new ops and wait for the applier task to drain.
    pub async fn shutdown(mut self) {
        drop(self.write_tx);
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn applier_sees_every_op_in_order() {
        // Snap = AtomicU64; each Op adds a value.
        let actor = IndexActor::<u64, AtomicU64>::spawn(AtomicU64::new(0), |op, snap| async move {
            snap.load().fetch_add(op, Ordering::Relaxed);
        });

        for i in 1..=100u64 {
            actor.submit(i).unwrap();
        }

        // Drain the applier deterministically: shutdown closes the
        // sender and `join` waits for every queued op to be applied.
        // Grab a snapshot handle *before* shutdown drops it.
        let snap = actor.snapshot.clone();
        actor.shutdown().await;
        assert_eq!(snap.load().load(Ordering::Relaxed), 5050);
    }

    #[tokio::test]
    async fn snapshot_replace_visible_to_readers() {
        let actor = IndexActor::<(), u64>::spawn(0, |_, _| async {});
        assert_eq!(*actor.snapshot(), 0);
        actor.replace_snapshot(42);
        assert_eq!(*actor.snapshot(), 42);
        actor.shutdown().await;
    }
}
