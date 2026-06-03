use std::future::Future;
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
    pub async fn run<F, Fut>(&self, flush: F) -> DbResult<()>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = DbResult<()>>,
    {
        let rx = {
            let mut s = self.state.lock().await;
            let (tx, rx) = oneshot::channel();
            s.waiters.push(tx);
            if s.leader_busy {
                // A leader is already running; it (or a later round) serves me.
                drop(s);
                return recv(rx).await;
            }
            s.leader_busy = true; // I am the leader.
            rx
        };

        // Leader loop: flush for the current batch, then for any newcomers that
        // arrived during the flush, until none remain.
        loop {
            let batch: Vec<_> = {
                let mut s = self.state.lock().await;
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

            let mut s = self.state.lock().await;
            if s.waiters.is_empty() {
                s.leader_busy = false;
                break;
            }
            // else: keep leadership, loop to flush for the newcomers.
        }

        // My own sender was in the first batch — my result is ready.
        recv(rx).await
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
