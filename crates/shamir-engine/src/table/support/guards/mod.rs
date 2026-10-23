//! Guard manifest: RAII guards and barriers around table-manager state.

pub mod in_flight_create_guard;
pub(in crate::table) mod migration_freeze_guard;
pub mod writer_drain_barrier;
