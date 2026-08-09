//!
//! #1048: E2E tests for hash DROP (regular + unique) crash-recovery op_id status writes.
//!
//! Tests that hash DROP's crash recovery writes SucceededViaCrashRecovery
//! under the SAME deterministic op_id the client received at dispatch time.
//!
//! Mirrors the structure of p997_hash_rename_durability_tests.rs' RENAME test.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::table::TableManager;
use shamir_types::types::record_id::RecordId;

// ============================================================================
// Helpers (from p997_hash_rename_durability_tests.rs)
// ============================================================================

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

async fn key_id(mgr: &TableManager, name: &str) -> u64 {
    let interner = mgr.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

// ============================================================================
// Tests
// ============================================================================

/// #1048: E2E test that a regular hash DROP's crash recovery writes
/// SucceededViaCrashRecovery under the SAME deterministic op_id the client
/// received at dispatch time.
///
/// Test structure mirrors the RENAME test:
/// 1. Create a table with a regular index + data
/// 2. Mint a real op_id using the same deterministic formula as handle_drop_index
/// 3. Start a DROP that will pause mid-flight (using the existing pause hook)
/// 4. Wait for the pause hook to fire (DROP is mid-flight)
/// 5. Simulate a crash by dropping the manager
/// 6. Reopen (simulate server restart) — recovery completes the DROP
/// 7. Poll for the op_id and verify SucceededViaCrashRecovery
#[tokio::test]
async fn p1048_e2e_hash_drop_op_id_recovery() {
    let (data_store, info_store) = make_stores();

    // Step 1: create a table with a regular index and data
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        let email_field = key_id(&mgr, "email").await;
        mgr.insert(&record_with_str(email_field, "a@b.com"))
            .await
            .unwrap();
        mgr.interner.persist().await.unwrap();
        mgr.create_index("by_email", &["email"]).await.unwrap();
        drop(mgr);
    }

    // Step 2: mint a real op_id (simulating dispatch) using the SAME formula as handle_drop_index
    let index_name = "by_email";
    let op_id = RecordId::system(&format!("ddl_drop_index_{}", index_name));

    // Step 3: start a DROP that will pause mid-flight
    let pause_hook =
        Arc::new(shamir_index::base_index::backfill_pause_hook::BackfillPauseHook::new());

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        // Install the drop pause hook BEFORE starting the DROP
        mgr.index_manager_ref()
            .set_drop_index_pause_hook(Some(Arc::clone(&pause_hook)));

        // This will write the tombstone, then pause mid-drop (after tombstone-write,
        // before the sweep — the exact crash window from #959)
        let mgr_c = mgr.clone();
        let drop_task = tokio::spawn(async move {
            mgr_c.drop_index(index_name, Some(op_id)).await.unwrap();
        });

        // Wait for the pause hook to fire (DROP is mid-flight)
        pause_hook.wait_until_parked().await;

        // Drop the drop_task and let mgr be dropped naturally to simulate a crash
        drop(drop_task);
    }

    // Step 4: reopen (simulate server restart) — recovery will complete the DROP
    let mgr: TableManager =
        TableManager::create("people".into(), data_store, Arc::clone(&info_store))
            .await
            .unwrap();

    // Verify the DROP completed: index is gone
    assert!(
        !mgr.index_exists(index_name).await,
        "index must be gone after recovery"
    );

    // Step 5: poll for the op_id and verify we got SucceededViaCrashRecovery
    use crate::table::ddl_op_log;
    use shamir_query_types::read::{DdlOpState, DdlOpStatus};

    let status: Option<DdlOpStatus> = ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .expect("read_op_status should not error");
    assert!(
        status.is_some(),
        "op_id must be present in the op-status log after recovery"
    );
    let status = status.unwrap();
    assert_eq!(
        status.op_id, op_id,
        "returned status must have the same op_id"
    );

    match status.state {
        DdlOpState::SucceededViaCrashRecovery {
            completed_at_restart,
        } => {
            // Success! The recovery wrote the correct status.
            assert!(
                completed_at_restart > 0,
                "completed_at_restart must be a valid timestamp"
            );
        }
        other => panic!(
            "Expected SucceededViaCrashRecovery after crash recovery, got {:?}",
            other
        ),
    }
}

/// #1048: E2E test that a unique hash DROP's crash recovery writes
/// SucceededViaCrashRecovery under the SAME deterministic op_id the client
/// received at dispatch time.
///
/// Mirrors the regular hash DROP test above, but for the unique family.
#[tokio::test]
async fn p1048_e2e_unique_hash_drop_op_id_recovery() {
    let (data_store, info_store) = make_stores();

    // Step 1: create a table with a unique index and data
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        let email_field = key_id(&mgr, "email").await;
        mgr.insert(&record_with_str(email_field, "a@b.com"))
            .await
            .unwrap();
        mgr.interner.persist().await.unwrap();
        mgr.create_unique_index("uniq_email", &["email"])
            .await
            .unwrap();
        drop(mgr);
    }

    // Step 2: mint a real op_id (simulating dispatch)
    let index_name = "uniq_email";
    let op_id = RecordId::system(&format!("ddl_drop_index_{}", index_name));

    // Step 3: start a DROP that will pause mid-flight
    let pause_hook =
        Arc::new(shamir_index::base_index::backfill_pause_hook::BackfillPauseHook::new());

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        // Install the drop pause hook BEFORE starting the DROP
        mgr.index_manager_ref()
            .set_drop_index_pause_hook(Some(Arc::clone(&pause_hook)));

        let mgr_c = mgr.clone();
        let drop_task = tokio::spawn(async move {
            mgr_c
                .drop_unique_index(index_name, Some(op_id))
                .await
                .unwrap();
        });

        // Wait for the pause hook to fire (DROP is mid-flight)
        pause_hook.wait_until_parked().await;

        // Simulate crash
        drop(drop_task);
    }

    // Step 4: reopen — recovery completes the DROP
    let mgr: TableManager =
        TableManager::create("people".into(), data_store, Arc::clone(&info_store))
            .await
            .unwrap();

    // Verify the DROP completed: unique index is gone
    assert!(
        !mgr.unique_index_exists(index_name).await,
        "unique index must be gone after recovery"
    );

    // Verify uniqueness is still enforced (we can insert the duplicate)
    let email_field = key_id(&mgr, "email").await;
    let record = record_with_str(email_field, "a@b.com");
    mgr.insert(&record).await.unwrap();

    // Step 5: poll for the op_id and verify SucceededViaCrashRecovery
    use crate::table::ddl_op_log;
    use shamir_query_types::read::{DdlOpState, DdlOpStatus};

    let status: Option<DdlOpStatus> = ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .expect("read_op_status should not error");
    assert!(
        status.is_some(),
        "op_id must be present in the op-status log after recovery"
    );
    let status = status.unwrap();
    assert_eq!(
        status.op_id, op_id,
        "returned status must have the same op_id"
    );

    match status.state {
        DdlOpState::SucceededViaCrashRecovery {
            completed_at_restart,
        } => {
            assert!(
                completed_at_restart > 0,
                "completed_at_restart must be a valid timestamp"
            );
        }
        other => panic!(
            "Expected SucceededViaCrashRecovery after crash recovery, got {:?}",
            other
        ),
    }
}
