//! P1-2 (#967): DDL structured error tests.
//!
//! When a multi-phase DDL operation fails, the operation status log should
//! contain a structured `DdlOpState::Failed { detail }` entry with the
//! enriched error message, not just a plain `DbError` return.
//!
//! These tests use a `FailableSetStore` to inject failures at specific
//! points and assert the op-status log contains the structured failure state.

use crate::table::TableManager;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::error::Error;
use std::io::{Error as IoError, ErrorKind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper that can be armed to deterministically fail exactly the
/// NEXT `set` call, then automatically disarms itself. A sticky (non-
/// self-disarming) fault would also fail the structured `Failed { detail }`
/// status write that the code under test performs immediately after the
/// injected failure (both go through the same `info_store`), so the fault
/// must be one-shot to isolate "make this ONE write fail" from "then let
/// the failure-handling code's own writes succeed".
struct FailableSetStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
    scan_armed: AtomicBool,
}

impl FailableSetStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
            scan_armed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl Store for FailableSetStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.armed.swap(false, Ordering::SeqCst) {
            return Err(DbError::Io(IoError::new(
                ErrorKind::PermissionDenied,
                "injected set failure",
            )));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        if self.scan_armed.swap(false, Ordering::SeqCst) {
            return Box::pin(futures::stream::once(async {
                Err(DbError::Io(IoError::new(
                    ErrorKind::PermissionDenied,
                    "injected scan failure",
                )))
            }));
        }
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(keys).await
    }
}

#[test]
fn rv_index_2ba3b945_contextual_io_round_trip() {
    let original = DbError::Io(IoError::new(ErrorKind::PermissionDenied, "original io"));
    let contextual = shamir_index::backend::contextualize_db_error(
        original,
        "CREATE INDEX failed: original io".to_string(),
    );
    let index_error = shamir_index::backend::IndexError::from(contextual);
    assert!(index_error.to_string().contains("CREATE INDEX failed"));
    let error = DbError::from(index_error);
    assert!(error.to_string().contains("CREATE INDEX failed"));
    assert!(matches!(&error, DbError::Io(io) if io.kind() == ErrorKind::PermissionDenied));
    let mut source = error.source();
    let mut found_original = false;
    while let Some(current) = source {
        if let Some(io) = current.downcast_ref::<IoError>() {
            if io.kind() == ErrorKind::PermissionDenied && io.to_string() == "original io" {
                found_original = true;
            }
        }
        source = current.source();
    }
    assert!(
        found_original,
        "original I/O error must remain in the source chain"
    );
}

#[test]
fn rv_index_2ba3b945_index_error_source_chain() {
    let index_error = shamir_index::backend::IndexError::from(DbError::Io(IoError::new(
        ErrorKind::PermissionDenied,
        "index source",
    )));
    let mut found = None;
    let mut current = index_error.source();
    while let Some(error) = current {
        if let Some(io) = error.downcast_ref::<IoError>() {
            found = Some(io);
            break;
        }
        current = error.source();
    }
    let io =
        found.expect("original I/O error must be reachable through the IndexError source chain");
    assert_eq!(io.kind(), ErrorKind::PermissionDenied);
    assert_eq!(io.to_string(), "index source");
    assert!(
        matches!(DbError::from(index_error), DbError::Io(io) if io.kind() == ErrorKind::PermissionDenied)
    );
}

#[tokio::test]
async fn rv_index_2ba3b945_metadata_save_preserves_io() {
    let faulty = Arc::new(FailableSetStore::new(Arc::new(InMemoryStore::new())));
    let store: Arc<dyn Store> = faulty.clone();
    faulty.armed.store(true, Ordering::SeqCst);
    let error = shamir_index::persistence::save_index2_metadata(
        &shamir_index::IndexRegistry::new(),
        &store,
    )
    .await
    .unwrap_err();
    assert!(matches!(&error, DbError::Io(io) if io.kind() == ErrorKind::PermissionDenied));
    assert_eq!(error.source().unwrap().to_string(), "injected set failure");
}

#[tokio::test]
async fn rv_index_2ba3b945_injected_io_survives_index_to_engine() {
    let faulty = Arc::new(FailableSetStore::new(Arc::new(InMemoryStore::new())));
    let info_store: Arc<dyn Store> = faulty.clone();
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("rows".into(), data_store, info_store)
        .await
        .unwrap();
    let interner = mgr.interner().get().await.unwrap();
    let key = interner.touch_ind("body").unwrap().into_key();
    mgr.interner().persist().await.unwrap();
    let op = shamir_query_builder::ddl::create_index("search", "rows")
        .field("body")
        .index_type("fts")
        .build();
    let shamir_query_types::batch::BatchOp::CreateIndex(op) = op else {
        panic!("expected create index");
    };
    mgr.create_index_v2(&op).await.unwrap();
    let mut row = shamir_types::types::common::new_map();
    row.insert(
        key,
        shamir_types::types::value::InnerValue::Str("hello".into()),
    );
    faulty.armed.store(true, Ordering::SeqCst);
    let error = mgr
        .index2_on_insert(
            &RecordId::system("row"),
            &shamir_types::types::value::InnerValue::Map(row),
        )
        .await
        .unwrap_err();
    let source = error.source().expect("Io source survives");
    let io = source
        .downcast_ref::<IoError>()
        .expect("source is io error");
    assert_eq!(io.kind(), ErrorKind::PermissionDenied);
    assert_eq!(io.to_string(), "injected set failure");
    assert!(matches!(error, DbError::Io(_)));
}

#[tokio::test]
async fn rv_index_2ba3b945_functional_planner_propagates_scan_io() {
    let faulty = Arc::new(FailableSetStore::new(Arc::new(InMemoryStore::new())));
    let info_store: Arc<dyn Store> = faulty.clone();
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("rows".into(), data_store, info_store)
        .await
        .unwrap();
    let interner = mgr.interner().get().await.unwrap();
    interner.touch_ind("email").unwrap();
    mgr.interner().persist().await.unwrap();
    let op = shamir_query_builder::ddl::create_index("email_lower", "rows")
        .field("email")
        .index_type("functional")
        .functional_op("lower")
        .build();
    let shamir_query_types::batch::BatchOp::CreateIndex(op) = op else {
        panic!("expected create index");
    };
    mgr.create_index_v2(&op).await.unwrap();
    let filter = shamir_query_builder::filter::computed("lower", "email", "eq", "alice");
    let interner = mgr.interner().get().await.unwrap();
    assert!(mgr
        .try_plan_index2(&filter, interner)
        .await
        .unwrap()
        .is_some());
    faulty.scan_armed.store(true, Ordering::SeqCst);
    let result = mgr.try_plan_index2(&filter, interner).await;
    assert!(matches!(
        result,
        Err(DbError::Io(ref io))
            if io.kind() == ErrorKind::PermissionDenied && io.to_string() == "injected scan failure"
    ));
    assert!(!faulty.scan_armed.load(Ordering::SeqCst));
}

// ============================================================================
// Tests
// ============================================================================

/// #967: RENAME hash index failure writes structured `Failed { detail }`.
///
/// This test injects a failure during a hash index RENAME operation and
/// asserts that the op-status log contains a `DdlOpState::Failed { detail }`
/// entry with the enriched error message.
#[tokio::test]
async fn p967_rename_hash_index_failure_writes_structured_error() {
    let faulty = Arc::new(FailableSetStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let mgr = TableManager::create(
        "default".into(),
        data_store.clone(),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Create a regular hash index to rename.
    mgr.create_index("name_idx", &["name"]).await.unwrap();

    // Drop the manager to ensure persisted state.
    drop(mgr);

    // Reopen with a new manager instance.
    let mgr = TableManager::create("default".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Mint a real op_id.
    let op_id = RecordId::system("ddl_rename_index_name_idx");

    // Install the mid-rename pause hook (fires after the tombstone is
    // written and the new index is created, before the old index is
    // dropped) — this is the EXACT point `rename_index`'s own `#967`
    // structured-error write covers (see the `drop_index(old_id)` match
    // arm), so the injected failure must land there, not on the tombstone
    // write (which returns early via `?` before that code is ever reached).
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_rename_mid_pause_hook(Some(Arc::clone(&hook)));

    let mgr_c = mgr.clone();
    let rename_task = tokio::spawn(async move {
        mgr_c
            .rename_index("name_idx", "name_idx_renamed", Some(op_id))
            .await
    });

    // Wait for the rename to park (tombstone written, new index created,
    // old index NOT yet dropped).
    hook.wait_until_parked().await;

    // Arm the one-shot fault: the very next `set` call (inside
    // `drop_index(old_id)`) will fail.
    faulty.armed.store(true, Ordering::SeqCst);

    // Release the hook — `drop_index(old_id)` now runs and hits the fault.
    hook.release();

    let result = rename_task.await.unwrap();
    assert!(
        matches!(result, Err(DbError::Io(ref io)) if io.kind() == ErrorKind::PermissionDenied),
        "rename must preserve the injected Io error"
    );

    // Read the op-status log.
    let status: Option<DdlOpStatus> = crate::table::ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .expect("read_op_status should not error");

    // Assert the op_id is present in the log.
    assert!(
        status.is_some(),
        "op_id must be present in the op-status log after failure"
    );
    let status = status.unwrap();

    // Assert the structured shape matches `Failed { detail }`.
    match status.state {
        DdlOpState::Failed { detail } => {
            // Assert the detail contains the enriched error message.
            assert!(
                detail.contains("RENAME INDEX") && detail.contains("name_idx"),
                "detail must contain the enriched error message"
            );
            assert!(
                detail.contains("injected set failure"),
                "detail must contain the injected failure cause, got: {detail}"
            );
        }
        other => panic!("Expected DdlOpState::Failed {{ detail }}, got {:?}", other),
    }

    // Assert the op_kind is correct.
    assert!(
        matches!(status.kind, DdlOpKind::RenameHashIndex { .. }),
        "op_kind must be RenameHashIndex"
    );
}

/// #967: RENAME unique hash index failure writes structured `Failed { detail }`.
///
/// This test injects a failure during a unique hash index RENAME operation
/// and asserts that the op-status log contains a structured failure entry.
#[tokio::test]
async fn p967_rename_unique_hash_index_failure_writes_structured_error() {
    let faulty = Arc::new(FailableSetStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let mgr = TableManager::create(
        "default".into(),
        data_store.clone(),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();

    // Create a unique hash index to rename.
    mgr.create_unique_index("unique_name_idx", &["name"])
        .await
        .unwrap();

    // Drop the manager to ensure persisted state.
    drop(mgr);

    // Reopen with a new manager instance.
    let mgr = TableManager::create("default".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Mint a real op_id.
    let op_id = RecordId::system("ddl_rename_unique_index_unique_name_idx");

    // Install the mid-rename pause hook — see the regular-family test above
    // for why the fault must land here (inside `drop_index`/`drop_unique_index`
    // during the second mutating step), not on the tombstone write.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_rename_mid_pause_hook(Some(Arc::clone(&hook)));

    let mgr_c = mgr.clone();
    let rename_task = tokio::spawn(async move {
        mgr_c
            .rename_index("unique_name_idx", "unique_name_idx_renamed", Some(op_id))
            .await
    });

    hook.wait_until_parked().await;

    // Arm the one-shot fault: the very next `set` call will fail.
    faulty.armed.store(true, Ordering::SeqCst);

    hook.release();

    let result = rename_task.await.unwrap();
    assert!(
        matches!(result, Err(DbError::Io(ref io)) if io.kind() == ErrorKind::PermissionDenied),
        "rename must preserve the injected Io error"
    );

    // Read the op-status log.
    let status: Option<DdlOpStatus> = crate::table::ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .expect("read_op_status should not error");

    // Assert the structured failure shape.
    assert!(
        status.is_some(),
        "op_id must be present in the op-status log after failure"
    );
    let status = status.unwrap();

    match status.state {
        DdlOpState::Failed { detail } => {
            assert!(
                detail.contains("RENAME INDEX") && detail.contains("unique_name_idx"),
                "detail must contain the enriched error message"
            );
        }
        other => panic!("Expected DdlOpState::Failed {{ detail }}, got {:?}", other),
    }

    assert!(
        matches!(status.kind, DdlOpKind::RenameUniqueHashIndex { .. }),
        "op_kind must be RenameUniqueHashIndex"
    );
}
