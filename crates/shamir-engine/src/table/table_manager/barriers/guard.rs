use crate::repo::RepoWritePermit;

/// F-70 (#897, P0) — RAII guard returned by
/// [`TableManager::begin_write_barrier`](crate::table::TableManager::begin_write_barrier): raises one bit of the shared
/// [`WriteBarrierFlags`](crate::index::write_barrier_flags::WriteBarrierFlags)
/// word on construction, clears it on drop (including every `?` early-return
/// path in the caller). Owns a CLONE of the `Arc<AtomicU8>`-backed flags
/// word (cheap — one refcount bump) rather than borrowing it, so it can be
/// returned by value from `begin_write_barrier` with no lifetime tied to
/// `&TableManager` — the shape a cross-crate `pub` acquisition helper needs.
///
/// F-95 (#957, P0): this guard ALSO embeds the per-table DDL admission
/// mutex guard (`ddl_admission`), acquired BEFORE the bit is raised inside
/// [`begin_write_barrier`](crate::table::TableManager::begin_write_barrier). Embedding it
/// here — rather than returning it as a third tuple element — means every
/// existing call site's destructuring (`let (_barrier, _lock) = …`) is
/// unchanged, and the admission guard's lifetime is automatically tied to
/// this guard's. Drop order is correct: the manual `Drop::drop` clears the
/// bit first, then Rust drops the struct's fields in declaration order
/// (`flags` → `bit` → `admission`), so the admission mutex is released
/// ONLY after the bit has been cleared — the next DDL waiting on admission
/// wakes to a clean state (bit 0) and does its own raise→drain→lock from
/// scratch.
///
/// Supersedes the old per-module `IndexCreateBarrierGuard`
/// (`table_manager_index_mgmt.rs`) and `SchemaActivationBarrierGuard`
/// (`shamir-db::execute::admin_schema`), both of which independently raised
/// a bit under an ALREADY-HELD `unique_write_lock` (lock-then-drain — the
/// F-70 inversion). This guard is constructed BEFORE the lock is taken (see
/// [`begin_write_barrier`](crate::table::TableManager::begin_write_barrier)); the bit is
/// still `SeqCst`-ordered (F-56, extended by F-69 to the whole word), which
/// is what the `writer_drain_barrier` proof requires regardless of when
/// the lock is acquired relative to the drain.
#[must_use]
pub struct WriteBarrierGuard {
    flags: crate::index::write_barrier_flags::WriteBarrierFlags,
    bit: u8,
    /// F-95 (#957): the DDL admission mutex guard, held for the caller's
    /// entire critical section. Declared LAST so it drops AFTER the
    /// manual `Drop::drop` has already cleared `bit`.
    // The field is never *read* — it is held purely for its `Drop`
    // side-effect (releasing `ddl_admission` after the bit is cleared).
    // Without it the admission mutex would be released at the end of
    // `begin_write_barrier` rather than the end of the caller's critical
    // section, reopening the exact multi-owner race this guard closes.
    #[allow(dead_code)]
    admission: tokio::sync::OwnedMutexGuard<()>,
    /// S5c-X — the per-repo admission permit, retained so the caller's
    /// ENTIRE critical section holds repository admission. `Some` when the
    /// permit was minted by [`TableManager::begin_write_barrier`](crate::table::TableManager::begin_write_barrier) itself;
    /// `None` on the `_admitted` path, where the caller keeps ownership of
    /// its own permit (dropping it at the end of the enclosing `_admitted`
    /// method — after this guard drops — releases admission).
    #[allow(dead_code)]
    permit: Option<RepoWritePermit>,
}

impl WriteBarrierGuard {
    pub(in crate::table::table_manager) fn set(
        flags: crate::index::write_barrier_flags::WriteBarrierFlags,
        bit: u8,
        admission: tokio::sync::OwnedMutexGuard<()>,
        permit: Option<RepoWritePermit>,
    ) -> Self {
        flags.set(bit);
        Self {
            flags,
            bit,
            admission,
            permit,
        }
    }

    /// Attach the caller-minted admission permit to a guard produced by the
    /// `_admitted` path, so the returned compound guard releases repo
    /// admission only when it drops.
    pub(in crate::table::table_manager) fn retaining_permit(
        mut self,
        permit: RepoWritePermit,
    ) -> Self {
        self.permit = Some(permit);
        self
    }
}

impl Drop for WriteBarrierGuard {
    fn drop(&mut self) {
        self.flags.clear(self.bit);
    }
}
