use std::sync::Arc;
use crate::table::TableManager;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_types::types::record_id::RecordId;


use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::fixtures::p997_hash_rename_durability_tests::*;

// ============================================================================
// #1048 (P1-2 sub-slice B): e2e crash-recovery test proving RFC §2.3's
// worked example — op_id carried through rename → crash → recovery →
// SucceededViaCrashRecovery status.
//
// This test reuses the existing `maybe_pause_rename_mid()` test seam to
// simulate a crash mid-rename, then verifies that polling for the SAME op_id
// returns `SucceededViaCrashRecovery` after recovery completes.
// ============================================================================

#[tokio::test]
async fn p1048_e2e_unique_rename_severe_case_op_id_threading() {
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

    // Step 2: mint a real op_id (simulating dispatch) and start a rename
    // that will pause mid-flight
    let op_id = RecordId::system("ddl_rename_index_uniq_email_to_uniq_email_new_test");
    let pause_hook = Arc::new(BackfillPauseHook::new());

    {
        let mgr = TableManager::create(
            "people".into(),
            Arc::clone(&data_store),
            Arc::clone(&info_store),
        )
        .await
        .unwrap();

        // Install the pause hook BEFORE starting the rename
        mgr.index_manager_ref()
            .set_rename_mid_pause_hook(Some(pause_hook.clone()));

        // This will write the tombstone with op_id, then pause mid-rename
        // (after drop_unique_index, before create_unique_index_body — the
        // EXACT SEVERE crash window from RFC §2.3)
        let rename_task = tokio::spawn(async move {
            mgr.rename_index("uniq_email", "uniq_email_new", Some(op_id))
                .await
        });

        // Wait for the pause hook to fire (rename is mid-flight)
        pause_hook.wait_until_parked().await;

        // Drop the rename_task and let mgr be dropped naturally to simulate a crash
        drop(rename_task);
    }

    // Step 3: verify tombstone was written with op_id
    let tombstone_list = load_unique_rename_tombstone(&info_store).await;
    assert_eq!(tombstone_list.len(), 1, "must have one stranded tombstone");
    let tombstone = &tombstone_list[0];
    assert_eq!(tombstone.old_name, "uniq_email");
    assert_eq!(tombstone.new_name, "uniq_email_new");
    assert_eq!(
        tombstone.op_id,
        Some(op_id.to_string()),
        "tombstone must carry the op_id minted at dispatch"
    );

    // Step 4: reopen (simulate server restart) — recovery will complete the rename
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    // Verify the rename completed: old is gone, new exists
    assert!(
        !mgr.unique_index_exists("uniq_email").await,
        "old unique index must be gone after recovery"
    );
    assert!(
        mgr.unique_index_exists("uniq_email_new").await,
        "new unique index must exist after recovery"
    );

    // Verify uniqueness is still enforced
    let email_field = key_id(&mgr, "email").await;
    let dup = mgr.insert(&record_with_str(email_field, "a@b.com")).await;
    assert!(
        dup.is_err(),
        "uniqueness must still be enforced after recovery"
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

    // Tombstone must be cleared
    assert!(
        load_unique_rename_tombstone(&info_store).await.is_empty(),
        "tombstone must be cleared after recovery"
    );
}
