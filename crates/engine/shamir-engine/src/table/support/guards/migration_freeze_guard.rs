use crate::index::write_barrier_flags::{WriteBarrierFlags, MIGRATION_FREEZE};

/// Owns a source table's migration-freeze bit, not its DDL admission or lock.
/// Dropping this guard synchronously allows writes again, including when
/// preparation is cancelled before the writer drain or lock fence completes.
#[derive(Debug)]
#[must_use = "keep the guard alive for the entire source migration"]
pub struct MigrationFreezeGuard {
    flags: WriteBarrierFlags,
    armed: bool,
}

impl MigrationFreezeGuard {
    /// Atomically claim the bit and immediately establish its RAII owner.
    /// A rejected claim does not own or clear the existing freeze.
    pub(in crate::table) fn try_set(flags: WriteBarrierFlags) -> Option<Self> {
        if flags.try_set(MIGRATION_FREEZE) {
            Some(Self { flags, armed: true })
        } else {
            None
        }
    }
    /// Retain the migration-freeze bit permanently after a successful rename.
    /// The consuming guard is disarmed, so its destructor leaves the bit set.
    pub(crate) fn retire(mut self) {
        self.armed = false;
    }
}

impl Drop for MigrationFreezeGuard {
    fn drop(&mut self) {
        if self.armed {
            self.flags.clear(MIGRATION_FREEZE);
        }
    }
}
