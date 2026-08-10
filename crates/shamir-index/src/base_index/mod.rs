// NOT `#[cfg(test)]`-gated: `shamir-engine`'s cross-crate test
// (`f72_planner_invisibility_tests.rs`) installs this hook on `IndexManager`
// from a DIFFERENT crate's test binary, where `shamir-index`'s OWN
// `cfg(test)` is not active (cross-crate `cfg(test)` does not propagate — a
// dependency is always compiled in non-test mode from the dependent's
// perspective). The hook is a zero-cost `None` on every real path (see
// `IndexManager::create_index_backfill_hook`'s field doc).
pub mod backfill_pause_hook;
pub mod ddl_op_log;
pub mod index_definition;
pub mod index_info;
pub mod index_info_item;
pub mod index_keys;
pub mod index_manager;
pub mod index_manager_unique;
pub mod index_record_key;
pub mod index_status;
pub mod sorted_index_definition;
pub mod sorted_index_manager;
pub mod write_barrier_flags;

#[cfg(test)]
pub mod tests;
