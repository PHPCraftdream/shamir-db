//! Guard manifest: RAII guards and barriers around table-manager state.

pub mod in_flight_create_guard;
pub(crate) mod migration_freeze_guard;
pub mod writer_drain_barrier;

#[cfg(all(loom, test))]
mod tests;
