use std::sync::Arc;

#[cfg(test)]
use shamir_collections::TFxMap;
use shamir_storage::error::DbResult;
use shamir_storage::types::{AtomicBatch, Repo as _, Store};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};
#[cfg(test)]
use tokio::sync::Notify;

use crate::repo::RepoInstance;

/// cfg(test) DDL-interlock handshake hook: `entered` fires (notify_waiters)
/// BEFORE the admission lock is awaited; `done` is set (Release) and
/// `exited` fires (notify_waiters) when the DDL guard drops.
#[cfg(test)]
pub(crate) struct DdlHook {
    pub entered: Notify,
    pub done: AtomicBool,
    pub exited: Notify,
}

#[cfg(test)]
fn ddl_hooks() -> &'static Mutex<TFxMap<String, Arc<DdlHook>>> {
    static HOOKS: OnceLock<Mutex<TFxMap<String, Arc<DdlHook>>>> = OnceLock::new();
    HOOKS.get_or_init(|| Mutex::new(TFxMap::default()))
}

/// Get-or-insert the DDL hook for `repo_name` (test-side accessor).
#[cfg(test)]
pub(crate) fn ddl_hook(repo_name: &str) -> Arc<DdlHook> {
    ddl_hooks()
        .lock()
        .unwrap()
        .entry(repo_name.to_owned())
        .or_insert_with(|| {
            Arc::new(DdlHook {
                entered: Notify::new(),
                done: AtomicBool::new(false),
                exited: Notify::new(),
            })
        })
        .clone()
}

fn fire_ddl_entered(repo_name: &str) {
    #[cfg(test)]
    ddl_hook(repo_name).entered.notify_waiters();
    #[cfg(not(test))]
    let _ = repo_name;
}

/// Guard type returned by [`RepoInstance::ddl_admission_lock`]: on drop it
/// records hook completion (cfg(test)) before releasing the admission mutex.
pub(crate) struct DdlGuard {
    inner: tokio::sync::OwnedMutexGuard<()>,
    repo_name: String,
}

impl Drop for DdlGuard {
    fn drop(&mut self) {
        #[cfg(test)]
        {
            let hooks = ddl_hooks().lock().unwrap();
            if let Some(hook) = hooks.get(&self.repo_name) {
                hook.done.store(true, Ordering::Release);
                hook.exited.notify_waiters();
            }
        }
    }
}

impl RepoInstance {
    pub(crate) async fn follower_journal_store(&self) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get("__changelog__").await
    }

    /// Backend capability for the S5c-X PR6b-2 receipt apply protocol.
    pub(crate) fn storage_supports_atomic_batch(&self) -> bool {
        self.repo.supports_atomic_batch()
    }

    /// DDL-only write-admission guard (PR6b-2 follow-up): the FULL coverage
    /// set holds this mutex for the whole of each operation — the
    /// repo-level index/rename wrappers (`create_index`,
    /// `create_unique_index`, `drop_index`, `drop_unique_index`,
    /// `rename_table_stores`), the follower receipt apply pipeline
    /// (`apply_batch_receipt`), the shamir-db facade table create/drop
    /// (`ShamirDb::add_table_as`/`drop_table`, via
    /// [`RepoInstance::ddl_admission_guard`]), and the wire executor's
    /// direct `TableManager` index DDL (CREATE/DROP/RENAME INDEX, also via
    /// `ddl_admission_guard`) — so follower applies can never interleave
    /// with local DDL.
    ///
    /// Contention model (sanctioned category 2): DDL ops and the receipt
    /// apply pipeline only; never on the read/write hot path; contention
    /// nil. Deadlock safety: no guarded entry point calls another guarded
    /// entry point (each takes the mutex EXACTLY ONCE per path and releases
    /// it before returning — e.g. `drop_table_cleaning_validators` delegates
    /// to the already-guarded `drop_table`, and `rename_table_as` routes
    /// into the already-guarded `rename_table_stores` without a second
    /// acquisition; the tokio mutex is NOT reentrant), so lock ordering is
    /// flat — no nested acquisition, hence no deadlock.
    /// Acquire the repository DDL admission guard. Test hooks fire `entered`
    /// BEFORE the lock is awaited (a waiter signals it is about to block) and
    /// `done`+`exited` when the guard drops — deterministic handshake for the
    /// DDL-interlock tests.
    pub(crate) async fn ddl_admission_lock(&self) -> DdlGuard {
        fire_ddl_entered(self.name());
        let inner = self.ddl_admission.clone().lock_owned().await;
        DdlGuard {
            inner,
            repo_name: self.name().to_owned(),
        }
    }

    /// Public DDL admission guard: production DDL entry points outside this
    /// crate (the shamir-db facade and the wire executor) must hold this for
    /// the whole operation so follower receipt applies cannot interleave.
    /// Fires the same cfg(test) `entered` notify as `ddl_admission_lock`.
    pub async fn ddl_admission_guard(&self) -> tokio::sync::OwnedMutexGuard<()> {
        fire_ddl_entered(self.name());
        self.ddl_admission.clone().lock_owned().await
    }

    /// Raw named-store access for tests and the receipt pipeline
    /// (`pub(super)` struct field, exposed deliberately).
    pub(crate) async fn raw_store(&self, name: &str) -> DbResult<Arc<dyn Store>> {
        self.repo.store_get(name).await
    }

    /// Begin a repository-wide atomic batch over named stores (PR6b-2).
    pub(crate) async fn begin_repo_atomic_batch(&self) -> DbResult<Box<dyn AtomicBatch>> {
        self.repo.begin_atomic_batch().await
    }
}
