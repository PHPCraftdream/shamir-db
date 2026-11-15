//! Durability seam: the one place every `SyncAll` persist goes through.
use fjall::{Database, PersistMode};
use std::sync::Arc;

/// Test/verification seam: counts every `Database::persist(PersistMode::SyncAll)`
/// issued by this module (per-store `flush()`, `persist_once`, atomic-batch
/// commit). O(1) per call on a rare shutdown/commit path — not a hot path.
#[doc(hidden)]
pub static FJALL_PERSIST_SYNC_ALL_COUNT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Reset the persist counter (test setup).
#[doc(hidden)]
pub fn reset_persist_sync_all_count() {
    FJALL_PERSIST_SYNC_ALL_COUNT.store(0, std::sync::atomic::Ordering::Release);
}

/// The ONE place every `SyncAll` persist goes through, so tests can count
/// full-database fsyncs regardless of the caller.
///
/// Returns the RAW fjall error (not a mapped `DbError`): the atomic-batch
/// commit site wraps it in its own `Uncertain` detail string, while the
/// per-store `flush()` site maps to `"Fjall persist: {e}"` — a shared
/// `DbError` here would change one of the two caller's message text.
pub(super) fn persist_sync_all(db: &Arc<Database>) -> Result<(), fjall::Error> {
    FJALL_PERSIST_SYNC_ALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    db.persist(PersistMode::SyncAll)
}
