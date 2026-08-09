//!
//! #1048: E2E tests for index2 DROP crash-recovery op_id status writes.
//!
//! Tests that index2 DROP's crash recovery writes SucceededViaCrashRecovery
//! under the SAME op_id the client received at dispatch time (#1051: minted
//! via `RecordId::new()`, carried through the tombstone — no longer
//! regenerated deterministically from the index name).
//!
//! Mirrors the structure of p997_hash_rename_durability_tests.rs' RENAME test.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_types::types::record_id::RecordId;

// ============================================================================
// Helpers
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

/// A functional `lower(<field>)` index create op (mirrors other test helpers)
fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
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

/// #1048: E2E test that an index2 DROP's crash recovery writes
/// SucceededViaCrashRecovery under the SAME op_id the client received at
/// dispatch time.
///
/// Test structure mirrors the RENAME test:
/// 1. Create a table with an index2 (functional) index + data
/// 2. Mint a real op_id via `RecordId::new()` (#1051: matches how
///    `handle_drop_index` mints it at dispatch time)
/// 3. Start a DROP that will pause mid-flight (using the existing pause hook)
/// 4. Wait for the pause hook to fire (DROP is mid-flight)
/// 5. Simulate a crash by dropping the manager
/// 6. Reopen (simulate server restart) — recovery completes the DROP
/// 7. Poll for the op_id and verify SucceededViaCrashRecovery
#[tokio::test]
async fn p1048_e2e_index2_drop_op_id_recovery() {
    let (data_store, info_store) = make_stores();

    // Step 1: create a table with an index2 (functional) index and data
    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();
        let name_field = key_id(&mgr, "name").await;
        mgr.insert(&record_with_str(name_field, "Alice"))
            .await
            .unwrap();
        mgr.insert(&record_with_str(name_field, "Bob"))
            .await
            .unwrap();
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();
        drop(mgr);
    }

    // Step 2: mint a real op_id (simulating dispatch), matching how
    // handle_drop_index mints it via RecordId::new() (#1051).
    let index_name = "lower_name";
    let op_id = RecordId::new();

    // Step 3: start a DROP that will pause mid-flight
    let pause_hook = Arc::new(BackfillPauseHook::new());

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        // Verify postings are present before DROP
        let name_interned = key_id(&mgr, "lower_name").await;
        let backend = mgr
            .index2_registry()
            .get_by_name(name_interned)
            .await
            .expect("index must be registered");
        let id = backend.descriptor().id;
        let prefix = Bytes::copy_from_slice(&id.to_le_bytes());
        let stream = info_store.scan_prefix_stream(prefix, 1000);
        futures::pin_mut!(stream);
        let mut posting_count = 0;
        while let Some(batch) = stream.next().await {
            for (_, _) in batch.unwrap() {
                posting_count += 1;
            }
        }
        assert!(
            posting_count > 0,
            "precondition: postings must be present before DROP, got {posting_count}"
        );

        // Install the post-sweep hook BEFORE starting the DROP
        mgr.set_drop_index2_post_sweep_hook(Some(Arc::clone(&pause_hook)));

        // This will write the tombstone, retire the backend, sweep postings,
        // then pause mid-drop (after sweep, before metadata persist — the exact
        // crash window from #988).
        // Use tokio::select! to cancel the drop_index2 future when the hook fires,
        // simulating a real crash (the whole process dies, taking all stacks with it).
        let mgr_c = mgr.clone();
        tokio::select! {
            _ = mgr_c.drop_index2(index_name, Some(op_id)) => {
                panic!("drop_index2 completed before post-sweep hook fired");
            }
            _ = pause_hook.wait_until_parked() => {
                // Parked: sweep done, metadata not yet persisted.
            }
        }

        // Drop both manager instances to simulate crash
        drop(mgr_c);
        drop(mgr);
    }

    // Step 4: reopen (simulate server restart) — recovery will complete the DROP
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Verify the DROP completed: index2 is gone
    let name_interned = key_id(&mgr, "lower_name").await;
    assert!(
        mgr.index2_registry()
            .get_by_name(name_interned)
            .await
            .is_none(),
        "index2 must be gone after recovery"
    );

    // Step 5: poll for the op_id and verify we got SucceededViaCrashRecovery
    use crate::table::ddl_op_log;
    use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};

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
    // #1051: discriminate on the record's identity, not just op_id presence
    // — proves the tombstone-carried index name (replacing the old
    // descriptor-lookup mechanism) actually resolves to the right name.
    match &status.kind {
        DdlOpKind::DropIndex2 { index_name: got } => {
            assert_eq!(
                got, index_name,
                "recovered status must name the dropped index"
            )
        }
        other => panic!("expected DropIndex2, got {:?}", other),
    }

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
