use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use scc::hash_map::Entry;
use scc::HashMap;
use shamir_collections::THasher;
use tokio::sync::{Mutex, OwnedMutexGuard};

#[cfg(test)]
type FunctionDdlWaitHook = Arc<dyn Fn(String) + Send + Sync>;
#[cfg(test)]
type FunctionDdlAcquiredHook = Arc<dyn Fn(String, tokio::task::Id) + Send + Sync>;

#[derive(Clone)]
pub(in crate::shamir_db::shamir_db) struct FunctionDdlLocks {
    entries: Arc<HashMap<String, Arc<Mutex<()>>, THasher>>,
    #[cfg(test)]
    wait_hook: Arc<std::sync::OnceLock<FunctionDdlWaitHook>>,
    #[cfg(test)]
    acquired_hook: Arc<std::sync::OnceLock<FunctionDdlAcquiredHook>>,
}

impl FunctionDdlLocks {
    pub(in crate::shamir_db::shamir_db) fn new() -> Self {
        Self {
            entries: Arc::new(HashMap::with_hasher(THasher::default())),
            #[cfg(test)]
            wait_hook: Arc::new(std::sync::OnceLock::new()),
            #[cfg(test)]
            acquired_hook: Arc::new(std::sync::OnceLock::new()),
        }
    }

    #[cfg(test)]
    pub(in crate::shamir_db::shamir_db) fn set_wait_hook(&self, hook: FunctionDdlWaitHook) {
        let _ = self.wait_hook.set(hook);
    }

    #[cfg(test)]
    pub(in crate::shamir_db::shamir_db) fn set_acquired_hook(&self, hook: FunctionDdlAcquiredHook) {
        let _ = self.acquired_hook.set(hook);
    }

    #[cfg(test)]
    pub(in crate::shamir_db::shamir_db) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(in crate::shamir_db::shamir_db) async fn lock(&self, name: &str) -> FunctionDdlGuard<'_> {
        self.acquire(name).await
    }

    pub(in crate::shamir_db::shamir_db) async fn lock_pair(
        &self,
        from: &str,
        to: &str,
    ) -> (FunctionDdlGuard<'_>, Option<FunctionDdlGuard<'_>>) {
        // Pair acquisition always takes names in lexical order; identical names acquire once.
        if from == to {
            return (self.lock(from).await, None);
        }
        if from < to {
            (self.lock(from).await, Some(self.lock(to).await))
        } else {
            (self.lock(to).await, Some(self.lock(from).await))
        }
    }

    fn acquire<'a>(&'a self, name: &str) -> FunctionDdlAcquire<'a> {
        let (wait_lock, lease_lock) = match self.entries.entry_sync(name.to_owned()) {
            Entry::Occupied(entry) => (Arc::clone(entry.get()), Arc::clone(entry.get())),
            Entry::Vacant(entry) => {
                let lock = Arc::new(Mutex::new(()));
                entry.insert_entry(Arc::clone(&lock));
                (Arc::clone(&lock), lock)
            }
        };
        let wait = wait_lock.lock_owned();
        FunctionDdlAcquire {
            wait: Some(Box::pin(wait)),
            lease: Some(FunctionDdlLease {
                entries: Arc::clone(&self.entries),
                name: name.to_owned(),
                lock: Some(lease_lock),
            }),
            #[cfg(test)]
            wait_hook: self.wait_hook.get().cloned(),
            #[cfg(test)]
            acquired_hook: self.acquired_hook.get().cloned(),
            #[cfg(test)]
            wait_notified: false,
            _borrow: PhantomData,
        }
    }
}

pub(in crate::shamir_db::shamir_db) struct FunctionDdlGuard<'a> {
    guard: Option<OwnedMutexGuard<()>>,
    lease: Option<FunctionDdlLease>,
    _borrow: PhantomData<&'a FunctionDdlLocks>,
}

impl Drop for FunctionDdlGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        drop(self.lease.take());
    }
}

struct FunctionDdlAcquire<'a> {
    wait: Option<Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send + 'static>>>,
    lease: Option<FunctionDdlLease>,
    #[cfg(test)]
    wait_hook: Option<FunctionDdlWaitHook>,
    #[cfg(test)]
    acquired_hook: Option<FunctionDdlAcquiredHook>,
    #[cfg(test)]
    wait_notified: bool,
    _borrow: PhantomData<&'a FunctionDdlLocks>,
}

impl<'a> Future for FunctionDdlAcquire<'a> {
    type Output = FunctionDdlGuard<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let wait = this
            .wait
            .as_mut()
            .expect("function DDL lock acquisition polled after completion");
        match wait.as_mut().poll(cx) {
            Poll::Pending => {
                #[cfg(test)]
                if !this.wait_notified {
                    if let Some(hook) = &this.wait_hook {
                        if let Some(lease) = &this.lease {
                            hook(lease.name.clone());
                        }
                    }
                    this.wait_notified = true;
                }
                Poll::Pending
            }
            Poll::Ready(guard) => {
                this.wait.take();
                let lease = this
                    .lease
                    .take()
                    .expect("function DDL lock lease missing at acquisition");
                #[cfg(test)]
                if let Some(hook) = &this.acquired_hook {
                    if let Some(task_id) = tokio::task::try_id() {
                        hook(lease.name.clone(), task_id);
                    }
                }
                Poll::Ready(FunctionDdlGuard {
                    guard: Some(guard),
                    lease: Some(lease),
                    _borrow: PhantomData,
                })
            }
        }
    }
}

impl Drop for FunctionDdlAcquire<'_> {
    fn drop(&mut self) {
        drop(self.wait.take());
        drop(self.lease.take());
    }
}

struct FunctionDdlLease {
    entries: Arc<HashMap<String, Arc<Mutex<()>>, THasher>>,
    name: String,
    lock: Option<Arc<Mutex<()>>>,
}

impl Drop for FunctionDdlLease {
    fn drop(&mut self) {
        let Some(lock) = self.lock.take() else {
            return;
        };
        match self.entries.entry_sync(self.name.clone()) {
            Entry::Occupied(entry) if Arc::ptr_eq(entry.get(), &lock) => {
                // Drop the lease under the key guard; concurrent acquisition and cleanup then share one identity and count.
                drop(lock);
                if Arc::strong_count(entry.get()) == 1 {
                    drop(entry.remove_entry());
                }
            }
            Entry::Occupied(_) | Entry::Vacant(_) => drop(lock),
        }
    }
}
