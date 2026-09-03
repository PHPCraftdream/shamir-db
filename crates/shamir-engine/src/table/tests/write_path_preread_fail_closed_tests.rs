//! Task-group-2 (2026-08-14 shamir-engine review, P0 #2) — `delete` / `set` /
//! `update_tx` must fail closed on a genuine pre-read error.
//!
//! `delete_returning_version`, `set_returning_version`, and `update_tx` each
//! read the record's current value before mutating it, so index maintenance
//! and the counter can compute the right delta. Before this fix, all three
//! collapsed EVERY `get()`/`read_one_tx()` error (including
//! `DbError::Storage` from a transient I/O fault and `DbError::Codec` from a
//! corrupt stored record) to `None` via `.ok()`, indistinguishable from the
//! record genuinely not existing. That turned:
//! - `delete` into a silent no-op `(false, 0)` instead of an error, and
//! - `set` / `update_tx` into the WRONG plan (the insert branch instead of
//!   the update branch): the old unique-index posting for the row is never
//!   released (permanently blocking that value from reuse), and the row
//!   counter delta is `+1` on what is actually an existing row.
//!
//! This is the same defect class F-65 (#891) already fixed on the sibling
//! `delete_tx` path (`read_one_tx_bytes` propagates via `?`, not `.ok()`).
//! These tests reuse F-65's `TEST_READ_ONE_TX_BYTES_FAILURE` fault-injection
//! seam (`table_manager_streaming.rs`), extended by this fix to also gate
//! `TableManager::get` and `TableManager::read_one_tx` — the two pre-read
//! primitives behind the three sites here.

use std::sync::Arc;

use shamir_storage::error::DbError;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::types::value::InnerValue;

use crate::table::{ReadOneTxBytesFailHook, TableManager, TEST_READ_ONE_TX_BYTES_FAILURE};

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

fn make_tx(snapshot: u64) -> TxContext {
    TxContext::new(TxId::new(1), 0, snapshot, IsolationLevel::Snapshot)
}

/// `delete_returning_version` (`table_manager_crud.rs`): a non-`NotFound`
/// pre-read error must propagate, not collapse into "record absent".
#[tokio::test]
async fn delete_fails_closed_on_pre_read_error() {
    let tbl = make_table().await;
    let id = tbl.insert(&InnerValue::Str("v".into())).await.unwrap();

    let hook =
        TEST_READ_ONE_TX_BYTES_FAILURE.get_or_init(|| Arc::new(ReadOneTxBytesFailHook::default()));
    hook.arm(tbl.table_token(), id);

    let err = tbl
        .delete(id)
        .await
        .expect_err("a non-NotFound pre-read error must propagate from delete, not no-op");
    assert!(
        matches!(err, DbError::Storage(_)),
        "expected Storage error, got {err:?}"
    );

    // Fail-closed: the row must still be present — a silent no-op would ALSO
    // leave it present but return `Ok(false)` instead of `Err`, which is
    // exactly the bug this test pins.
    assert!(
        tbl.get(id).await.is_ok(),
        "row must remain present after the failed pre-read"
    );
}

/// `set_returning_version` (`table_manager_crud.rs`): a non-`NotFound`
/// pre-read error must propagate, not misroute an existing row into the
/// insert plan.
#[tokio::test]
async fn set_fails_closed_on_pre_read_error() {
    let tbl = make_table().await;
    let id = tbl.insert(&InnerValue::Str("orig".into())).await.unwrap();

    let hook =
        TEST_READ_ONE_TX_BYTES_FAILURE.get_or_init(|| Arc::new(ReadOneTxBytesFailHook::default()));
    hook.arm(tbl.table_token(), id);

    let err = tbl
        .set(id, &InnerValue::Str("new".into()))
        .await
        .expect_err(
            "a non-NotFound pre-read error must propagate from set, not misroute to insert",
        );
    assert!(
        matches!(err, DbError::Storage(_)),
        "expected Storage error, got {err:?}"
    );

    // Fail-closed: the original value must be untouched — the insert-plan
    // bug would overwrite it while treating the row as newly created.
    let value = tbl.get(id).await.unwrap();
    assert!(
        matches!(value, InnerValue::Str(ref s) if s == "orig"),
        "original value must be unchanged, got {value:?}"
    );
}

/// `update_tx` (`table_manager_tx_ops.rs`): a non-`NotFound` pre-read error
/// must propagate, not misroute an existing row into the insert plan or
/// silently stage a write.
#[tokio::test]
async fn update_tx_fails_closed_on_pre_read_error() {
    let tbl = make_table().await;
    let id = tbl.insert(&InnerValue::Str("orig".into())).await.unwrap();

    let hook =
        TEST_READ_ONE_TX_BYTES_FAILURE.get_or_init(|| Arc::new(ReadOneTxBytesFailHook::default()));
    hook.arm(tbl.table_token(), id);

    let mut tx = make_tx(u64::MAX);
    let err = tbl
        .update_tx(id, &InnerValue::Str("new".into()), Some(&mut tx))
        .await
        .expect_err(
            "a non-NotFound pre-read error must propagate from update_tx, not misroute to insert",
        );
    assert!(
        matches!(err, DbError::Storage(_)),
        "expected Storage error, got {err:?}"
    );

    // Fail-closed: the error must occur strictly before any staging, so no
    // write was recorded for this table under the wrong (insert) plan.
    assert!(
        !tx.write_set.contains_key(&tbl.table_token()),
        "update_tx must not stage a write when the pre-read fails"
    );
}
