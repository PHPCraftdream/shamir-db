//! Immutable batch chain backing the in-memory WAL.
//!
//! The mutex protects only the head pointer and its eligibility mirror.
//! Appends prepend one batch; replay snapshots one Arc. Truncation rebuilds
//! outside the mutex, publishing only if the head is unchanged. A concurrent
//! append forces a retry, so no post-snapshot frame can be lost.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use shamir_storage::error::{DbError, DbResult};
use tokio::task::spawn_blocking;

use crate::wal_entry_v2::WalEntryV2;

type Entries = Vec<(u64, Arc<Vec<u8>>)>;
type Head = Option<Arc<Batch>>;

struct Batch {
    entries: Entries,
    previous: Head,
    min_positive: u64,
}

impl Drop for Batch {
    fn drop(&mut self) {
        // Avoid recursive Arc destruction for arbitrarily long chains.
        let mut next = self.previous.take();
        while let Some(node) = next {
            match Arc::into_inner(node) {
                Some(mut owned) => next = owned.previous.take(),
                None => break,
            }
        }
    }
}

fn min_positive(a: u64, b: u64) -> u64 {
    match (a, b) {
        (0, v) | (v, 0) => v,
        (a, b) => a.min(b),
    }
}

fn make_batch(entries: Entries, batch_min: u64, previous: Head) -> Arc<Batch> {
    let min_positive = min_positive(batch_min, previous.as_ref().map_or(0, |n| n.min_positive));
    Arc::new(Batch {
        entries,
        previous,
        min_positive,
    })
}

#[derive(Default)]
pub(super) struct MemFrames {
    // Contention model: the sole writer is the group-commit leader (one
    // `AtomicBool` CAS elects it), so appends never contend with each other;
    // replay and truncation only snapshot or swap the chain under this lock
    // and decode outside it, and the lock is never held across an `.await`.
    head: Mutex<Head>,
    min: AtomicU64,
    truncate_gate: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    replay_pause: Mutex<Option<Pause>>,
    #[cfg(test)]
    truncate_pause: Mutex<Option<Pause>>,
}

#[cfg(test)]
type Pause = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[cfg(test)]
fn pause(
    slot: &Mutex<Option<Pause>>,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (resume, wait) = std::sync::mpsc::channel();
    *slot.lock().unwrap() = Some((tx, wait));
    (rx, resume)
}

#[cfg(test)]
fn wait_for_resume(pause: Option<Pause>) {
    if let Some((entered, resume)) = pause {
        let _ = entered.send(());
        let _ = resume.recv();
    }
}

impl MemFrames {
    pub(super) fn append(&self, entries: Vec<(u64, Vec<u8>)>) {
        let entries: Entries = entries.into_iter().map(|(v, p)| (v, Arc::new(p))).collect();
        let batch_min = entries
            .iter()
            .map(|(v, _)| *v)
            .filter(|v| *v > 0)
            .min()
            .unwrap_or(0);
        let mut node = make_batch(entries, batch_min, None);
        let mut head = self.head.lock().expect("MemSink frames mutex poisoned");
        let node_mut = Arc::get_mut(&mut node).expect("new batch is uniquely owned");
        node_mut.previous = head.take();
        node_mut.min_positive = min_positive(
            batch_min,
            node_mut.previous.as_ref().map_or(0, |n| n.min_positive),
        );
        self.min.store(node.min_positive, Ordering::Release);
        *head = Some(node);
    }

    fn snapshot(&self) -> Head {
        self.head
            .lock()
            .expect("MemSink frames mutex poisoned")
            .clone()
    }

    pub(super) fn eligible(&self, durable: u64) -> bool {
        let min = self.min.load(Ordering::Acquire);
        min != 0 && min <= durable
    }

    pub(super) async fn replay(self: &Arc<Self>) -> DbResult<Vec<WalEntryV2>> {
        let shared = Arc::clone(self);
        #[cfg(test)]
        let pause = self.replay_pause.lock().unwrap().take();
        spawn_blocking(move || {
            let snapshot = shared.snapshot();
            #[cfg(test)]
            wait_for_resume(pause);
            let mut nodes = Vec::new();
            let mut cursor = snapshot.as_ref();
            while let Some(node) = cursor {
                nodes.push(node);
                cursor = node.previous.as_ref();
            }
            let mut out = Vec::new();
            for node in nodes.into_iter().rev() {
                for (_, payload) in &node.entries {
                    out.push(WalEntryV2::decode(payload)?);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| DbError::Internal(format!("MemSink replay join: {e}")))?
    }

    pub(super) async fn truncate(self: &Arc<Self>, durable: u64) -> DbResult<usize> {
        let gate = Arc::clone(&self.truncate_gate).lock_owned().await;
        let shared = Arc::clone(self);
        #[cfg(test)]
        let mut pause = self.truncate_pause.lock().unwrap().take();
        spawn_blocking(move || {
            let _gate = gate;
            loop {
                let snapshot = shared.snapshot();
                #[cfg(test)]
                wait_for_resume(pause.take());
                let mut nodes = Vec::new();
                let mut cursor = snapshot.as_ref();
                while let Some(node) = cursor {
                    nodes.push(node);
                    cursor = node.previous.as_ref();
                }
                let mut rebuilt = None;
                let mut removed = 0;
                for node in nodes.into_iter().rev() {
                    let entries: Entries = node
                        .entries
                        .iter()
                        .filter_map(|(v, p)| {
                            if *v != 0 && *v <= durable {
                                removed += 1;
                                None
                            } else {
                                Some((*v, Arc::clone(p)))
                            }
                        })
                        .collect();
                    if !entries.is_empty() {
                        let batch_min = entries
                            .iter()
                            .map(|(v, _)| *v)
                            .filter(|v| *v > 0)
                            .min()
                            .unwrap_or(0);
                        rebuilt = Some(make_batch(entries, batch_min, rebuilt));
                    }
                }
                let mut head = shared.head.lock().expect("MemSink frames mutex poisoned");
                let unchanged = match (&*head, &snapshot) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                };
                if unchanged {
                    let old = std::mem::replace(&mut *head, rebuilt);
                    shared.min.store(
                        head.as_ref().map_or(0, |n| n.min_positive),
                        Ordering::Release,
                    );
                    drop(head);
                    drop(old);
                    return Ok(removed);
                }
                drop(head);
                drop(rebuilt);
            }
        })
        .await
        .map_err(|e| DbError::Internal(format!("MemSink truncate join: {e}")))?
    }

    #[cfg(test)]
    pub(super) fn pause_replay(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        pause(&self.replay_pause)
    }

    #[cfg(test)]
    pub(super) fn pause_truncate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        pause(&self.truncate_pause)
    }

    #[cfg(test)]
    pub(super) fn with_lock(&self, f: impl FnOnce()) {
        let _guard = self.head.lock().expect("MemSink frames mutex poisoned");
        f();
    }
}
