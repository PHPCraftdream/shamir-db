mod support;

pub(in crate::table) use support::guards::migration_freeze_guard;
pub use support::guards::{in_flight_create_guard, writer_drain_barrier};
pub use support::model::{buffer_config, table, table_config};
pub use support::storage::{interner_manager, persistable, record_counter, record_cow};
#[cfg(test)]
pub use support::test_seams::index2_backfill_hook;
pub(in crate::table) use support::test_seams::tx_scan_overlay;

// Re-export ddl_op_log from shamir-index for inline terminal status writes
pub use shamir_index::base_index::ddl_op_log;
pub mod degraded_index_count;
pub mod doctor;
mod read;
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

#[cfg(test)]
pub(crate) use table_manager_tx_ops::refresh_released_unique_cache;
mod table_manager_validators;
mod write;
#[cfg(test)]
pub(crate) use write::write_helpers;
// Engine sweep T1 chunk W3: `write_exec.rs` was split into `write/{insert,
// update/update, delete/delete, set/set}.rs` and `write_helpers.rs` moved to
// `write/write_helpers.rs`; the old `crate::table::write_helpers` path is
// kept via this module re-export.
#[path = "table_manager/admission/mod.rs"]
pub mod write_receipt;
pub use write_receipt::WriteOutcome;

#[cfg(test)]
pub(in crate::table) use read::vector::filtered_vector;

#[cfg(test)]
pub mod tests;

pub use interner_manager::InternerManager;
pub use migration_freeze_guard::MigrationFreezeGuard;
pub use persistable::{PersistRegistry, Persistable};
pub use record_counter::RecordCounter;
pub use table_config::TableConfig;
pub use table_manager::{
    table_token_for, Index2ReadView, InternerReadView, TableDataStoreReadView,
    TableInfoStoreReadView, TableManager, ValidatorRegistryReadView, WriteBarrierGuard,
};

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
pub(in crate::table) use read::planning::read_asof_seek;
#[cfg(test)]
pub(crate) use read::planning::read_asof_seek::{
    SeekLoopPreIterHook, TEST_SEEK_LOOP_PRE_ITER_HOOK,
};
