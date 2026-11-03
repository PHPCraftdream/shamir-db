use crate::index::write_barrier_flags::MIGRATION_FREEZE;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::{TableConfig, TableManager};
use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

async fn fixture() -> (RepoInstance, TableManager) {
    let repo = RepoInstance::new(
        "rename_visibility_test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        vec![TableConfig::new("a")],
    );
    let source = repo.get_table("a").await.unwrap();
    (repo, source)
}

#[tokio::test]
async fn visibility_wait_times_out_before_persist_and_restores_source() {
    let (repo, source) = fixture().await;
    source.ensure_attached().unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let version = gate.assign_next_version_guarded();
    let persist_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&persist_called);
    let rename = repo.rename_table_stores_with("a", "b", move |_| async move {
        called.store(true, Ordering::Release);
        Ok(())
    });
    let error = rename.await.unwrap_err();
    assert!(
        matches!(&error, DbError::Conflict(message) if message.starts_with("rename visibility wait timed out"))
    );
    assert!(!persist_called.load(Ordering::Acquire));
    assert!(repo.has_table("a"));
    assert!(!repo.has_table("b"));
    assert!(!source
        .index_manager_ref()
        .write_barrier_flags()
        .is_set(MIGRATION_FREEZE));
    let receipt = source
        .insert_with_receipt(
            &InnerValue::Null,
            shamir_query_types::batch::VisibilityWait::Bounded { timeout_ms: 1 },
        )
        .await
        .unwrap();
    let id = receipt.value;
    assert!(matches!(
        receipt.visibility,
        shamir_query_types::batch::VisibilityState::Pending { .. }
    ));
    assert_eq!(source.get(id).await.unwrap(), InnerValue::Null);
    source.ensure_attached().unwrap();
    source
        .create_index("after_timeout", &["name"])
        .await
        .unwrap();
    drop(version);
}
