//! #1067 — DDL op-log family classification + missing op_id for sorted/index2.
//!
//! Before this task:
//! - `handle_drop_index`'s sorted-family arm fell back to `DdlOpKind::DropHashIndex`
//!   (no dedicated variant existed) — a client polling a DROP-sorted-index status
//!   saw the wrong operation type.
//! - `handle_rename_index`'s classification checked ONLY `is_unique` — regular,
//!   sorted, AND index2 all collapsed into the same `RenameHashIndex` fallback.
//! - The sorted family never received an `op_id` at all (`drop_sorted_index`/
//!   `rename_index_sorted` had no `op_id` parameter).
//! - index2 RENAME had NO terminal `DdlOpStatus` write at all (#1066 built the
//!   durable tombstone/recovery mechanics but explicitly deferred the status
//!   integration to this task).
//!
//! Every test here asserts the SPECIFIC `DdlOpKind` variant and its field
//! values — not just "a status record exists" (the #1051/#1052 tautological
//! pattern this task's own brief calls out).
//!
//! Uses the real `Batch`/`ddl::{drop_index,rename_index}` builder API,
//! mirroring `p1065_ddl_status_contract_tests.rs`'s structure/API-usage style.

use shamir_engine::table::ddl_op_log;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::read::{DdlOpKind, DdlOpState};
use shamir_types::types::record_id::RecordId;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// Setup an in-memory ShamirDb with a `testdb/main/items` table carrying one
/// index of each of the four families: regular hash, unique hash, sorted,
/// and index2 (functional).
async fn setup_with_all_families() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    shamir.add_repo("testdb", repo_config).await.unwrap();

    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();
    table.create_index("idx_regular", &["city"]).await.unwrap();
    table
        .create_unique_index("idx_unique", &["email"])
        .await
        .unwrap();
    table
        .create_sorted_index("idx_sorted", &["age"])
        .await
        .unwrap();

    // index2 (functional) — via the wire DDL path, mirroring
    // `drop_index_index2_support.rs`'s own functional-index setup.
    let mut b = Batch::new();
    b.id("ci");
    b.create_index(
        "op",
        ddl::create_index("idx_index2", "items")
            .field("name")
            .index_type("functional")
            .functional_op("lower"),
    );
    let req = b.to_request_via_msgpack();
    shamir
        .execute("testdb", &req)
        .await
        .expect("create functional index2");

    shamir
}

// ============================================================================
// Test group 1 — DROP each of the four families: exact DdlOpKind + index_name.
// ============================================================================

#[tokio::test]
async fn p1067_drop_regular_classified_as_drop_hash_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_regular", "items")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::DropHashIndex { index_name } if index_name == "idx_regular"
        ),
        "expected DropHashIndex{{index_name: \"idx_regular\"}}, got {:?}",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

#[tokio::test]
async fn p1067_drop_unique_classified_as_drop_unique_hash_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_unique", "items")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::DropUniqueHashIndex { index_name } if index_name == "idx_unique"
        ),
        "expected DropUniqueHashIndex{{index_name: \"idx_unique\"}}, got {:?}",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

/// THE defect this task fixes for DROP: before #1067, this classified as
/// `DropHashIndex` (the fallback) instead of a dedicated sorted variant.
#[tokio::test]
async fn p1067_drop_sorted_classified_as_drop_sorted_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_sorted", "items")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::DropSortedIndex { index_name } if index_name == "idx_sorted"
        ),
        "expected DropSortedIndex{{index_name: \"idx_sorted\"}}, got {:?} \
         (pre-#1067 this would have been the wrong DropHashIndex fallback)",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

#[tokio::test]
async fn p1067_drop_index2_classified_as_drop_index2() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_index2", "items")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::DropIndex2 { index_name } if index_name == "idx_index2"
        ),
        "expected DropIndex2{{index_name: \"idx_index2\"}}, got {:?}",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

// ============================================================================
// Test group 2 — RENAME each of the four families: exact DdlOpKind +
// old_name/new_name.
// ============================================================================

#[tokio::test]
async fn p1067_rename_regular_classified_as_rename_hash_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_regular", "idx_regular_new")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::RenameHashIndex { old_name, new_name }
            if old_name == "idx_regular" && new_name == "idx_regular_new"
        ),
        "expected RenameHashIndex, got {:?}",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

#[tokio::test]
async fn p1067_rename_unique_classified_as_rename_unique_hash_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_unique", "idx_unique_new")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::RenameUniqueHashIndex { old_name, new_name }
            if old_name == "idx_unique" && new_name == "idx_unique_new"
        ),
        "expected RenameUniqueHashIndex, got {:?}",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

/// THE defect this task fixes for RENAME: before #1067, `handle_rename_index`
/// checked ONLY `is_unique` — a sorted rename fell through to the
/// `RenameHashIndex` `else` branch.
#[tokio::test]
async fn p1067_rename_sorted_classified_as_rename_sorted_index() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_sorted", "idx_sorted_new")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect("status should exist");
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::RenameSortedIndex { old_name, new_name }
            if old_name == "idx_sorted" && new_name == "idx_sorted_new"
        ),
        "expected RenameSortedIndex, got {:?} (pre-#1067 this would have been \
         the wrong RenameHashIndex fallback)",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

/// THE defect this task fixes for RENAME (index2 half): before #1067, an
/// index2 rename ALSO fell through to `RenameHashIndex` — AND #1066 never
/// wrote a terminal status for index2 RENAME at all (deferred to this task).
#[tokio::test]
async fn p1067_rename_index2_classified_as_rename_index2() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let op_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_index2", "idx_index2_new")
            .repo("main")
            .request_id(op_id),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let status = ddl_op_log::read_op_status(table.info_store(), &op_id)
        .await
        .unwrap()
        .expect(
            "status should exist — before #1067 index2 RENAME wrote NO \
             terminal status at all (#1066 deferred this)",
        );
    assert!(
        matches!(
            &status.kind,
            DdlOpKind::RenameIndex2 { old_name, new_name }
            if old_name == "idx_index2" && new_name == "idx_index2_new"
        ),
        "expected RenameIndex2, got {:?} (pre-#1067 this would have been the \
         wrong RenameHashIndex fallback, if it existed at all)",
        status.kind
    );
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

// ============================================================================
// Test group 3 — sorted DROP and RENAME carry a real op_id end-to-end.
// ============================================================================

/// Before #1067, `drop_sorted_index` had no `op_id` parameter at all — the
/// sorted family never got a status record written, so this poll would find
/// nothing. Proves op_id round-trips: the wire response's op_id matches what
/// `ddl_op_log::read_op_status` finds under that SAME id.
#[tokio::test]
async fn p1067_sorted_drop_op_id_round_trips() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let client_supplied_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("idx_sorted", "items")
            .repo("main")
            .request_id(client_supplied_id),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let result = &resp.results["d"];
    let returned_op_id = result.op_id.expect("op_id must be set on the response");
    assert_eq!(
        returned_op_id, client_supplied_id,
        "returned op_id must equal the client-supplied request_id"
    );

    let status = ddl_op_log::read_op_status(table.info_store(), &client_supplied_id)
        .await
        .unwrap()
        .expect(
            "status must be found by the client-supplied id — before #1067 the \
             sorted family never threaded op_id through at all",
        );
    assert_eq!(status.op_id, client_supplied_id);
    assert!(matches!(
        &status.kind,
        DdlOpKind::DropSortedIndex { index_name } if index_name == "idx_sorted"
    ));
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}

/// Same proof for sorted RENAME.
#[tokio::test]
async fn p1067_sorted_rename_op_id_round_trips() {
    let shamir = setup_with_all_families().await;
    let db = shamir.get_db("testdb").unwrap();
    let table = db.get_table("main", "items").await.unwrap();

    let client_supplied_id = RecordId::new();
    let mut b = Batch::new();
    b.id(1);
    b.rename_index(
        "r",
        ddl::rename_index("items", "idx_sorted", "idx_sorted_new")
            .repo("main")
            .request_id(client_supplied_id),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let result = &resp.results["r"];
    let returned_op_id = result.op_id.expect("op_id must be set on the response");
    assert_eq!(
        returned_op_id, client_supplied_id,
        "returned op_id must equal the client-supplied request_id"
    );

    let status = ddl_op_log::read_op_status(table.info_store(), &client_supplied_id)
        .await
        .unwrap()
        .expect(
            "status must be found by the client-supplied id — before #1067 \
             rename_index_sorted had no op_id parameter at all",
        );
    assert_eq!(status.op_id, client_supplied_id);
    assert!(matches!(
        &status.kind,
        DdlOpKind::RenameSortedIndex { old_name, new_name }
        if old_name == "idx_sorted" && new_name == "idx_sorted_new"
    ));
    assert!(matches!(status.state, DdlOpState::Succeeded { .. }));
}
