// NOT `#[cfg(test)]`-gated: `shamir-engine`'s cross-crate test
// (`f72_planner_invisibility_tests.rs`) installs this hook on `IndexManager`
// from a DIFFERENT crate's test binary, where `shamir-index`'s OWN
// `cfg(test)` is not active (cross-crate `cfg(test)` does not propagate — a
// dependency is always compiled in non-test mode from the dependent's
// perspective). The hook is a zero-cost `None` on every real path (see
// `IndexManager::create_index_backfill_hook`'s field doc).
pub mod index_manager;
pub mod index_manager_unique;
pub mod infra;
pub mod meta;
pub mod sorted_index_manager;
#[cfg(test)]
pub mod tests;

pub use infra::backfill_pause_hook;
pub use infra::ddl_op_log;
pub use infra::index_keys;
pub use infra::index_record_key;
pub use infra::write_barrier_flags;
pub use meta::index_definition;
pub use meta::index_info;
pub use meta::index_info_item;
pub use meta::index_status;
pub use meta::sorted_index_definition;
