use std::future::Future;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};

use shamir_storage::error::{DbError, DbResult};

/// Batches concurrent durability flushes so the underlying flush+fsync runs
/// once per group, not once per caller — without weakening durability: every
/// caller returns only after a flush that BEGAN after it registered, so its
/// already-buffered writes are persisted.
#[derive(Default)]
pub struct GroupCommit {
    state: Mutex<GcState>,
}

#[derive(Default)]
struct GcState {
    leader_busy: bool,
    waiters: Vec<oneshot::Sender<Result<(), String>>>,
}

impl GroupCommit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `flush` (the per-repo flush+fsync) under group-commit batching.
    /// `flush` may be invoked multiple times (one per batch round) but NEVER
    /// concurrently. Concurrent callers piling up during a flush are served by
    /// the next single flush.
    ///
    /// **Cancellation safety (audit §2.1, top-5 #5):** the leader loop runs in
    /// a **detached** `tokio::task` spawned here, NOT inline on the caller's
    /// task. If the caller's future is dropped (client disconnect, `select!`
    /// race, graceful shutdown), only THIS caller's wait on its `oneshot` is
    /// abandoned — the spawned leader task keeps running to completion,
    /// correctly resetting `leader_busy` and serving every other waiter
    /// (including ones that arrive after the original caller is gone). Before
    /// this fix, an inline `flush().await` that got cancelled stranded
    /// `leader_busy = true` forever, hanging every subsequent `synced_flush`
    /// on this repo (a durability-flush DoS).
    ///
    /// Requires `self: Arc<Self>` so the spawned task can own a reference
    /// without borrowing the caller's task. `flush` must be `'static + Send`
    /// for the same reason — callers capture owned state (e.g. a cloned
    /// `RepoInstance`, whose fields are all `Arc`-shared so the clone is cheap)
    /// rather than borrowing `&self`.
    pub async fn run<F, Fut>(self: Arc<Self>, flush: F) -> DbResult<()>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = DbResult<()>> + Send + 'static,
    {
        let rx = {
            let mut s = self.state.lock().await;
            let (tx, rx) = oneshot::channel();
            s.waiters.push(tx);
            if s.leader_busy {
                // A leader is already running; it (or a later round) serves me.
                // Drop the lock BEFORE awaiting — holding a tokio::Mutex guard
                // across `.await` deadlocks the leader_loop (it can never
                // acquire the lock to drain waiters).
                drop(s);
                return recv(rx).await;
            }
            s.leader_busy = true; // I am the leader.
            rx
        };

        // Spawn the leader loop so caller cancellation can't abandon it
        // mid-flush. The task owns an `Arc<Self>` (not borrowed from the
        // caller's task) and the `'static` closure, so dropping the caller's
        // future only abandons ITS `rx` — the leader runs to completion.
        tokio::spawn(leader_loop(Arc::clone(&self), flush));

        // Wait for the leader to flush a batch that BEGAN after I registered
        // (structural durability invariant). If my caller is dropped, only
        // this `recv` is abandoned — the spawned leader still serves every
        // other waiter.
        recv(rx).await
    }
}

/// The leader loop body, factored out so it can be `tokio::spawn`'d as a
/// detached task. Flushes for the current batch, then for any newcomers that
/// arrived during the flush, until no waiters remain — then releases
/// leadership (`leader_busy = false`) under the same lock as the empty
/// observation so a late pusher is either seen by us or wins leadership
/// itself (never stranded).
async fn leader_loop<F, Fut>(this: Arc<GroupCommit>, flush: F)
where
    F: Fn() -> Fut,
    Fut: Future<Output = DbResult<()>>,
{
    loop {
        let batch: Vec<_> = {
            let mut s = this.state.lock().await;
            std::mem::take(&mut s.waiters)
        };

        let res = flush().await;
        let err_msg = res.as_ref().err().map(|e| e.to_string());

        for w in batch {
            let _ = w.send(match &err_msg {
                Some(m) => Err(m.clone()),
                None => Ok(()),
            });
        }

        let mut s = this.state.lock().await;
        if s.waiters.is_empty() {
            s.leader_busy = false;
            break;
        }
        // else: keep leadership, loop to flush for the newcomers.
    }
}

async fn recv(rx: oneshot::Receiver<Result<(), String>>) -> DbResult<()> {
    match rx.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(msg)) => Err(DbError::Internal(msg)),
        Err(_) => Err(DbError::Internal("group-commit flush task dropped".into())),
    }
}

#[cfg(test)]
mod tests;
