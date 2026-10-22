//! #1067 — crash-recovery proof that index2 RENAME's `DdlOpStatus` write
//! (added by this task; #1066 built the tombstone/recovery mechanics but
//! explicitly deferred the status integration) survives a crash and is
//! written as `SucceededViaCrashRecovery` by `recover_index2_renames`.
//!
//! Crash injection uses `tokio::select!` racing the real `rename_index` call
//! against the deterministic `rename_index2_mid_pause_hook` — the SAME
//! pause point `p1066_index2_rename_durability_tests.rs`'s Test 3 uses
//! (fires after `rename_entry` mutates the live registry, before
//! `save_index2_metadata` persists it). NEVER `tokio::spawn` +
//! `drop(JoinHandle)`, which does not cancel the spawned task and would
//! hang the test for the full 180s nextest timeout (the exact trap #1048
//! hit before) — see `p1060_online_index_crash_recovery_tests.rs` /
//! `p1066_index2_rename_durability_tests.rs` for the proven shape this
//! file copies.
//!
//! The sorted family is NOT covered by an equivalent crash-recovery test
//! here: `SortedIndexManager`'s DROP/RENAME tombstones (`Vec<u64>` /
//! `Vec<(u64, u64)>`) do not carry an `op_id` at all — only the terminal
//! status write on the INLINE (no-crash) path was in this task's scope
//! (see the brief's "Explicitly out of scope" section: the tombstone
//! FORMAT itself, which would be needed for a sorted crash-recovery status
//! test, is a separate, deeper change). index2 RENAME's tombstone already
//! carries `op_id` end-to-end (#1051/#1066), so it is the one family where
//! a real crash-recovery `SucceededViaCrashRecovery` proof is achievable
//! within #1067's scope.

use std::sync::Arc;

use shamir_query_types::read::{DdlOpKind, DdlOpState};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::ddl_op_log;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_query_types::admin::types::CreateIndexOp;

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

/// A functional `lower(<field>)` index create op (mirrors p1048's/p1066's
/// own helper).
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

#[tokio::test]
async fn p1067_index2_rename_crash_recovery_writes_succeeded_via_crash_recovery() {
    let (data_store, info_store) = make_stores();

    let op_id = RecordId::new();

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
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();

        // Install the MID pause hook: fires after rename_entry mutates the
        // registry, before save_index2_metadata persists it — the SAME
        // window p1066's Test 3 exercises, but this time with a real op_id
        // so we can prove the #1067 status write survives the crash too.
        let hook = Arc::new(BackfillPauseHook::new());
        mgr.set_rename_index2_mid_pause_hook(Some(Arc::clone(&hook)));

        let mgr_c = mgr.clone();
        tokio::select! {
            _ = mgr_c.rename_index("lower_name", "lower_name_new", Some(op_id)) => {
                panic!("rename_index completed before the mid pause hook fired");
            }
            _ = hook.wait_until_parked() => {
                // Parked: registry shows NEW name, disk still shows OLD name,
                // tombstone carries op_id, NO terminal status written yet
                // (the #1067 status write sits AFTER save_index2_metadata,
                // which hasn't run yet at this pause point).
            }
        }

        // Sanity: no terminal status durable yet at this pause point — the
        // #1067 write happens strictly after save_index2_metadata, which
        // this pause point sits strictly before.
        let pre_crash_status = ddl_op_log::read_op_status(&info_store, &op_id)
            .await
            .unwrap();
        assert!(
            pre_crash_status.is_none()
                || matches!(pre_crash_status.unwrap().state, DdlOpState::InProgress),
            "no terminal status should be durable while parked before \
             save_index2_metadata has run"
        );

        // Simulate a crash: drop both handles, cancelling the parked future
        // (the `tokio::select!` above already dropped the `rename_index`
        // future when the pause-hook branch won the race).
        drop(mgr_c);
        drop(mgr);
    }

    // Reopen (simulate restart) — recovery must complete the rename AND
    // (the #1067 fix) write SucceededViaCrashRecovery for the tombstoned
    // op_id BEFORE clearing the tombstone.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(new_id).await.is_some(),
        "recovery must complete the rename to the new name"
    );

    let status = ddl_op_log::read_op_status(&info_store, &op_id)
        .await
        .unwrap()
        .expect(
            "SucceededViaCrashRecovery status must be durable after recovery — \
             before #1067, recover_index2_renames loaded op_id from the \
             tombstone but left it explicitly unused, so no status was ever \
             written here",
        );
    assert_eq!(status.op_id, op_id);
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::RenameIndex2 { old_name, new_name }
            if old_name == "lower_name" && new_name == "lower_name_new"
        ),
        "expected RenameIndex2{{old_name: \"lower_name\", new_name: \"lower_name_new\"}}, \
         got {:?}",
        status.kind
    );
    assert!(
        matches!(status.state, DdlOpState::SucceededViaCrashRecovery { .. }),
        "expected SucceededViaCrashRecovery (proving recovery, not the inline \
         path, wrote this status), got {:?}",
        status.state
    );
}

/// Regression guard: an index2 rename tombstone with NO op_id (pre-#1067
/// tombstone shape, or a non-DDL caller that passed `None`) must NOT cause
/// recovery to error or panic — the status write is skipped silently, same
/// convention `recover_index2_drops` already uses for a `None` op_id.
#[tokio::test]
async fn p1067_index2_rename_recovery_skips_status_write_when_op_id_absent() {
    let (data_store, info_store) = make_stores();

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
        mgr.interner().persist().await.unwrap();
        mgr.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
            .await
            .unwrap();

        let hook = Arc::new(BackfillPauseHook::new());
        mgr.set_rename_index2_mid_pause_hook(Some(Arc::clone(&hook)));

        let mgr_c = mgr.clone();
        tokio::select! {
            // No op_id (`None`) — mirrors a non-DDL / pre-#1067 caller.
            _ = mgr_c.rename_index("lower_name", "lower_name_new", None) => {
                panic!("rename_index completed before the mid pause hook fired");
            }
            _ = hook.wait_until_parked() => {}
        }

        drop(mgr_c);
        drop(mgr);
    }

    // Reopen — recovery must complete the rename without error even though
    // the tombstone carries no op_id.
    let mgr = TableManager::create("people".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    let new_id = key_id(&mgr, "lower_name_new").await;
    assert!(
        mgr.index2_registry().get_by_name(new_id).await.is_some(),
        "recovery must still complete the rename when op_id is absent"
    );
}
