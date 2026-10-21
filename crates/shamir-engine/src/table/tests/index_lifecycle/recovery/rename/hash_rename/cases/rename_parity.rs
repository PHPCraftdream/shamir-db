use crate::table::TableManager;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

use crate::table::tests::index_lifecycle::recovery::rename::hash_rename::fixtures::p997_hash_rename_durability_tests::*;

// ============================================================================
// #1000 (@oh post-#997-review fix): recovery interrupted between two
// stranded tombstone entries must not silently discard the not-yet-processed
// entry's tombstone.
//
// `recover_hash_renames` originally called `IndexManager::clear_from_renaming`
// per entry inside its reconciliation loop. That method derives the persisted
// snapshot from the IN-MEMORY `renaming_regular`/`renaming_unique` maps —
// which are ALWAYS EMPTY at open time (`IndexManager::new` starts fresh;
// `load_renaming_list` never rehydrates them, it only returns a local `Vec`).
// So the first per-entry clear during recovery persisted an empty list,
// silently discarding any sibling entry's tombstone if recovery was then
// interrupted before finishing it. A failed (not just crashed) rename
// legitimately leaves its tombstone behind without clearing it, so a
// multi-entry stranded list is a real, reachable state — not a contrived one.
// ============================================================================

#[tokio::test]
async fn p1000_regular_recovery_interrupted_between_entries_preserves_sibling_tombstone() {
    let (data_store, info_store) = make_stores();
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
    mgr.insert(&record_with_str(email_field, "c@d.com"))
        .await
        .unwrap();
    mgr.interner.persist().await.unwrap();

    // Two independent regular indexes, both left in the "crash before
    // create" matrix state (old present, new absent) and stranded
    // simultaneously in the SAME tombstone list.
    mgr.create_index("by_email", &["email"]).await.unwrap();
    mgr.create_index("by_email2", &["email"]).await.unwrap();

    let entry1 = rename_entry("by_email", "by_email_new");
    let entry2 = rename_entry("by_email2", "by_email2_new");
    let key = RecordId::system("idx_ren").to_bytes();
    let bytes = bincode::serialize(&vec![entry1.clone(), entry2.clone()]).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Install the between-entries pause hook, then run recovery directly
    // (bypassing `TableManager::create`, which would construct its own
    // fresh `IndexManager` a hook installed from outside couldn't reach)
    // racing it against the hook parking.
    let hook = Arc::new(BackfillPauseHook::new());
    mgr.index_manager_ref()
        .set_recover_renames_between_entries_hook(Some(Arc::clone(&hook)));
    let permit = mgr.write_domain.admit().await.unwrap();
    tokio::select! {
        r = mgr.recover_hash_renames_admitted(&permit) => {
            panic!("recovery completed before the between-entries hook fired: {r:?}");
        }
        _ = hook.wait_until_parked() => {
            // Parked: entry 1 (by_email -> by_email_new) is fully
            // reconciled; entry 2 must not have been touched yet.
        }
    }

    // The assertion this test exists for: BOTH tombstone entries must
    // still be on disk while recovery is parked between them. The old
    // buggy per-entry `clear_from_renaming` call would have already wiped
    // the WHOLE list to `[]` here, silently stranding entry 2.
    let remaining = load_regular_rename_tombstone(&info_store).await;
    assert_eq!(
        remaining.len(),
        2,
        "both tombstone entries must still be on disk while recovery is \
         parked between them -- entry 2 must not be silently discarded by \
         entry 1's reconciliation"
    );

    // Simulate the interruption completing as a real crash: drop the
    // manager (the parked `recover_hash_renames` call above was already
    // cancelled by `tokio::select!`).
    drop(mgr);

    // Reopen -- a fresh, uninterrupted recovery pass must finish BOTH
    // entries (entry 1's steps are idempotent no-ops the second time).
    let mgr2 = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    assert!(
        !mgr2.index_exists("by_email").await,
        "entry 1's old index must be gone"
    );
    assert!(
        mgr2.index_exists("by_email_new").await,
        "entry 1's new index must exist"
    );
    assert!(
        !mgr2.index_exists("by_email2").await,
        "entry 2's old index must be gone -- must NOT have been silently \
         stranded by entry 1's earlier reconciliation"
    );
    assert!(
        mgr2.index_exists("by_email2_new").await,
        "entry 2's new index must exist after the second recovery pass"
    );

    let r1 = mgr2
        .lookup_by_index("by_email_new", &[InnerValue::Str("a@b.com".into())])
        .await
        .unwrap();
    assert!(!r1.is_empty(), "entry 1's postings must be resolvable");
    let r2 = mgr2
        .lookup_by_index("by_email2_new", &[InnerValue::Str("c@d.com".into())])
        .await
        .unwrap();
    assert!(!r2.is_empty(), "entry 2's postings must be resolvable");

    assert!(
        load_regular_rename_tombstone(&info_store).await.is_empty(),
        "tombstone list must be fully cleared once both entries are done"
    );
}
