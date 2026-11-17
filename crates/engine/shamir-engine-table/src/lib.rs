mod support;

pub(crate) use support::guards::migration_freeze_guard;
pub use support::guards::{in_flight_create_guard, writer_drain_barrier};
pub use support::model::{buffer_config, table, table_config};
pub use support::storage::fk_reverse_cache;
pub use support::storage::{interner_manager, persistable, record_counter, record_cow};
pub use support::table_resolver::TableResolver;
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use support::test_seams::index2_backfill_hook;
pub(crate) use support::test_seams::tx_scan_overlay;
pub use support::validator_bound;

// Re-export ddl_op_log from shamir-index for inline terminal status writes
pub use shamir_index::base_index::ddl_op_log;
mod read;
pub mod table_manager;
pub use table_manager::diagnostics::{degraded_index_count, doctor, index_metrics};
mod write;
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use support::test_seams::test_support;

#[doc(hidden)]
pub use table_manager::AUTO_VERIFY_EVERY_N_WRITES;
#[path = "table_manager/admission/mod.rs"]
pub mod write_receipt;
pub use write_receipt::WriteOutcome;

#[cfg(test)]
pub mod tests;

pub use interner_manager::InternerManager;
pub use migration_freeze_guard::MigrationFreezeGuard;

pub use persistable::{PersistRegistry, Persistable};
pub use record_counter::RecordCounter;
#[doc(hidden)]
pub use support::model::names::{new_table_store_id, table_store_names};
#[doc(hidden)]
pub use support::storage::catalog::TableCatalog;
pub use table_config::TableConfig;
#[doc(hidden)]
pub use table_manager::MigrationFreezeError;
pub use table_manager::{
    table_token_for, Index2ReadView, InternerReadView, TableDataStoreReadView,
    TableInfoStoreReadView, TableManager, ValidatorRegistryReadView, WriteBarrierGuard,
};

#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use table::Table;

// F-65 (#891): re-export the `read_one_tx_bytes` failure-injection seam so the
// FK indexed-action fast-path tests under `crate::query::batch::tests` can arm a
// one-shot injected read error. Test-only.
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use table_manager::accessors::table_manager_streaming::{
    ReadOneTxBytesFailHook, TEST_READ_ONE_TX_BYTES_FAILURE,
};

// F-74 (#901): re-export the F-58 seek-loop pause seam so the tx-commit
// bump/apply-ordering tests under `crate::tx::tests` can install it and
// rendezvous a concurrent AsOf read against a racing commit's Phase 5c
// seam. Test-only.
#[cfg(test)]
pub(crate) use read::planning::read_asof_seek;
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use read::planning::read_asof_seek::{SeekLoopPreIterHook, TEST_SEEK_LOOP_PRE_ITER_HOOK};
// P1096: cross-crate tx tests refresh the released-unique cache through the facade.
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use table_manager::transactions::tx_support::refresh_released_unique_cache;

#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use table_manager::operations::table_manager_crud::{
    PostBarrierPreWriteHook, TEST_POST_BARRIER_PRE_WRITE_HOOK,
};
#[cfg(any(test, feature = "test-util"))]
#[doc(hidden)]
pub use table_manager::transactions::tx_support::{
    PostGenCapturePreUniqueCheckHook, TEST_POST_GEN_CAPTURE_PRE_UNIQUE_CHECK_HOOK,
};
