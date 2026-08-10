pub mod buffer_config;
// Re-export ddl_op_log from shamir-index for inline terminal status writes
pub use shamir_index::base_index::ddl_op_log;
pub mod degraded_index_count;
pub mod doctor;
mod filtered_vector;
pub mod in_flight_create_guard;
#[cfg(test)]
pub mod index2_backfill_hook;
pub mod interner_manager;
pub mod persistable;
mod read_asof_seek;
mod read_exec;
mod read_index_scan;
mod read_planner;
mod read_temporal;
pub mod record_counter;
pub mod record_cow;
#[allow(clippy::module_inception)]
pub mod table;
pub mod table_config;
pub mod table_manager;
mod table_manager_buffer;
mod table_manager_changefeed;
mod table_manager_crud;
mod table_manager_index_mgmt;
mod table_manager_locks;
mod table_manager_replication;
mod table_manager_sorted_index;
mod table_manager_streaming;
mod table_manager_tx_ops;
mod table_manager_validators;
mod tx_scan_overlay;
mod write_exec;
pub(crate) mod write_helpers;
pub mod writer_drain_barrier;

#[cfg(test)]
pub mod tests;

pub use interner_manager::InternerManager;
pub use persistable::{PersistRegistry, Persistable};
pub use record_counter::RecordCounter;
pub use table_config::TableConfig;
pub use table_manager::{table_token_for, TableManager, WriteBarrierGuard};

#[cfg(test)]
pub use table::Table;

// F-65 (#891): re-export the `read_one_tx_bytes` failure-injection seam so the
// FK indexed-action fast-path tests under `crate::query::batch::tests` can arm a
// one-shot injected read error. Test-only.
#[cfg(test)]
pub(crate) use table_manager_streaming::{ReadOneTxBytesFailHook, TEST_READ_ONE_TX_BYTES_FAILURE};

// F-74 (#901): re-export the F-58 seek-loop pause seam so the tx-commit
// bump/apply-ordering tests under `crate::tx::tests` can install it and
// rendezvous a concurrent AsOf read against a racing commit's Phase 5c
// seam. Test-only.
#[cfg(test)]
pub(crate) use read_asof_seek::{SeekLoopPreIterHook, TEST_SEEK_LOOP_PRE_ITER_HOOK};
