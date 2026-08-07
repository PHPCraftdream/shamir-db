//! `DROP INDEX` unified family resolution tests (#1025).
//!
//! Tests that DROP INDEX resolves the index family from the catalog
//! (regular hash, unique hash, sorted, or index2), not from the client's
//! `unique: bool` hint. Mirrors the pattern already established in
//! `TableManager::rename_index`.

use shamir_engine::index::sorted_index_manager::SortedIndexDefinition;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::read::{DdlOpKind, DdlOpState};
use shamir_types::core::interner::TouchInd;

use super::helpers::*;

// =====================================================================
// Scenario 1: Mismatched-flag drop succeeds correctly — unique index
// dropped with unique: false (wrong/default value).
// =====================================================================
#[tokio::test]
async fn drop_unique_index_with_mismatched_unique_false() {
    let db = setup_db().await;

    // 1. Create a UNIQUE index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("idx_name", "users")
                .field("name")
                .unique(),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create unique index");
    }

    // 2. DROP INDEX with unique: false (wrong/default value) — must succeed
    //    and drop the correct (unique) index.
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("idx_name", "users")); // unique defaults to false
        let req = b.to_request_via_msgpack();
        let resp = db
            .execute("testdb", &req)
            .await
            .expect("drop with mismatched flag");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "DROP with unique:false on a unique index must succeed and report existed:true"
    );

    // 3. Verify the index is actually gone (not a no-op).
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.unique_index_exists("idx_name").await,
        "unique index must be gone after drop"
    );
}

// =====================================================================
// Scenario 2: Mismatched-flag drop succeeds correctly — regular index
// dropped with unique: true (wrong value).
// =====================================================================
#[tokio::test]
async fn drop_regular_index_with_mismatched_unique_true() {
    let db = setup_db().await;

    // 1. Create a REGULAR (non-unique) index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("idx_age", "users").field("age"), // unique defaults to false
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create regular index");
    }

    // 2. DROP INDEX with unique: true (wrong value) — must still succeed
    //    and drop the correct (regular) index.
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("idx_age", "users").unique()); // wrong: unique=true
        let req = b.to_request_via_msgpack();
        let resp = db
            .execute("testdb", &req)
            .await
            .expect("drop with mismatched flag");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "DROP with unique:true on a regular index must succeed and report existed:true"
    );

    // 3. Verify the index is actually gone.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.index_exists("idx_age").await,
        "regular index must be gone after drop"
    );
}

// =====================================================================
// Scenario 3: Same mismatched-flag proof through the if_exists = true
// early-exit path specifically — unique index dropped with unique:false.
// =====================================================================
#[tokio::test]
async fn drop_unique_index_with_mismatched_flag_via_if_exists() {
    let db = setup_db().await;

    // 1. Create a UNIQUE index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("idx_email", "users")
                .field("email")
                .unique(),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create unique index");
    }

    // 2. DROP INDEX ... IF EXISTS with unique: false (wrong value) — must
    //    succeed and drop the correct (unique) index, NOT no-op as
    //    "existed: false".
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("idx_email", "users").if_exists());
        let req = b.to_request_via_msgpack();
        let resp = db
            .execute("testdb", &req)
            .await
            .expect("drop with mismatched flag via if_exists");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "IF EXISTS with unique:false on a unique index must report existed:true"
    );

    // 3. Verify the index is actually gone.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.unique_index_exists("idx_email").await,
        "unique index must be gone after drop"
    );
}

// =====================================================================
// Scenario 4: Same mismatched-flag proof through the if_exists = true
// early-exit path — regular index dropped with unique:true.
// =====================================================================
#[tokio::test]
async fn drop_regular_index_with_mismatched_flag_via_if_exists() {
    let db = setup_db().await;

    // 1. Create a REGULAR (non-unique) index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index("op", ddl::create_index("idx_score", "users").field("score"));
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create regular index");
    }

    // 2. DROP INDEX ... IF EXISTS with unique: true (wrong value) — must
    //    succeed and drop the correct (regular) index.
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index(
            "op",
            ddl::drop_index("idx_score", "users").unique().if_exists(),
        );
        let req = b.to_request_via_msgpack();
        let resp = db
            .execute("testdb", &req)
            .await
            .expect("drop with mismatched flag via if_exists");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "IF EXISTS with unique:true on a regular index must report existed:true"
    );

    // 3. Verify the index is actually gone.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.index_exists("idx_score").await,
        "regular index must be gone after drop"
    );
}

// =====================================================================
// Scenario 5: DdlOpKind in the op-status log reflects the true family,
// not the client's declared flag — unique index dropped with unique:false.
// =====================================================================
#[tokio::test]
async fn ddl_op_kind_reflects_true_family_unique_dropped_with_mismatched_flag() {
    let db = setup_db().await;

    // 1. Create a UNIQUE index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("idx_phone", "users")
                .field("phone")
                .unique(),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create unique index");
    }

    // 2. DROP INDEX with unique: false (wrong value) and capture op_id.
    let op_id = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("idx_phone", "users")); // unique defaults to false
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop");
        resp.results["op"].op_id.expect("op_id must be set")
    };

    // 3. Poll the op status via get_ddl_op_status and assert the logged
    //    kind is DropUniqueHashIndex (the TRUE family), not DropHashIndex
    //    (which would reflect the client's wrong unique:false hint).
    let status = db
        .get_ddl_op_status("testdb", "main", "users", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("status record should exist");

    assert_eq!(
        status.kind,
        DdlOpKind::DropUniqueHashIndex {
            index_name: "idx_phone".to_string()
        },
        "DdlOpKind must reflect the TRUE family (unique), not the client's wrong hint"
    );

    // 4. Verify state is Succeeded.
    match status.state {
        DdlOpState::Succeeded { .. } => {
            // Expected
        }
        other => panic!("status should be Succeeded, got {:?}", other),
    }
}

// =====================================================================
// Scenario 6: DdlOpKind in the op-status log reflects the true family —
// regular index dropped with unique:true.
// =====================================================================
#[tokio::test]
async fn ddl_op_kind_reflects_true_family_regular_dropped_with_mismatched_flag() {
    let db = setup_db().await;

    // 1. Create a REGULAR (non-unique) index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("idx_address", "users").field("address"),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create regular index");
    }

    // 2. DROP INDEX with unique: true (wrong value) and capture op_id.
    let op_id = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("idx_address", "users").unique()); // wrong: unique=true
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop");
        resp.results["op"].op_id.expect("op_id must be set")
    };

    // 3. Poll the op status and assert the logged kind is DropHashIndex
    //    (the TRUE family), not DropUniqueHashIndex (which would reflect
    //    the client's wrong unique:true hint).
    let status = db
        .get_ddl_op_status("testdb", "main", "users", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("status record should exist");

    assert_eq!(
        status.kind,
        DdlOpKind::DropHashIndex {
            index_name: "idx_address".to_string()
        },
        "DdlOpKind must reflect the TRUE family (regular), not the client's wrong hint"
    );

    // 4. Verify state is Succeeded.
    match status.state {
        DdlOpState::Succeeded { .. } => {
            // Expected
        }
        other => panic!("status should be Succeeded, got {:?}", other),
    }
}

// =====================================================================
// Scenario 7: DdlOpKind in the op-status log reflects index2 family
// when dropping an index2 backend (confirms the orchestrator's fix
// that added is_index2 → DropIndex2 handling).
// =====================================================================
#[tokio::test]
async fn ddl_op_kind_reflects_index2_family() {
    let db = setup_db().await;

    // 1. Create an index2 (functional) backend.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("func_idx", "users")
                .field("name")
                .index_type("functional")
                .functional_op("lower"),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create functional index");
    }

    // 2. DROP INDEX and capture op_id.
    let op_id = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("func_idx", "users"));
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop");
        resp.results["op"].op_id.expect("op_id must be set")
    };

    // 3. Poll the op status and assert the logged kind is DropIndex2.
    let status = db
        .get_ddl_op_status("testdb", "main", "users", &op_id.to_string())
        .await
        .expect("get_ddl_op_status should succeed")
        .expect("status record should exist");

    assert_eq!(
        status.kind,
        DdlOpKind::DropIndex2 {
            index_name: "func_idx".to_string()
        },
        "DdlOpKind must be DropIndex2 for index2 backends"
    );

    // 4. Verify state is Succeeded.
    match status.state {
        DdlOpState::Succeeded { .. } => {
            // Expected
        }
        other => panic!("status should be Succeeded, got {:?}", other),
    }
}

// =====================================================================
// Scenario 8: cross-family collision guard correctly refuses a genuine
// pre-existing collision (a name present in >1 family). #1010's CREATE-time
// preflight prevents NEW collisions from forming, but the guard must still
// catch data that predates it. Reproduced here the same way
// `verify_detects_pre_existing_cross_family_collision`
// (crates/shamir-engine/src/table/tests/
// r0c_registry_atomicity_and_namespace_tests.rs) constructs one: register a
// sorted-index definition directly against `SortedIndexManager`, bypassing
// the wire-level CREATE preflight entirely, under a name a regular index
// already owns.
// =====================================================================
#[tokio::test]
async fn drop_index_refuses_genuine_cross_family_collision() {
    let db = setup_db().await;

    // 1. Create a REGULAR index named "shared_name" through the normal wire
    //    path (this alone is legitimate — #1010's preflight has nothing to
    //    object to yet).
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("shared_name", "users").field("name"),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create regular index");
    }

    // 2. Bypass the wire CREATE preflight and directly register a SORTED
    //    index definition under the SAME interned name — the only way to
    //    reproduce pre-#1010 collision data, since every wire entry point
    //    now refuses this.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    let name_interned = {
        let interner = table.interner().get().await.unwrap();
        match interner.touch_ind("shared_name").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    };
    let field_key = {
        let interner = table.interner().get().await.unwrap();
        match interner.touch_ind("name").unwrap() {
            TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
        }
    };
    let sorted_def = SortedIndexDefinition::new(name_interned, vec![field_key]);
    table.sorted_indexes().register(sorted_def).await.unwrap();

    // 3. DROP INDEX "shared_name" over the wire must refuse with
    //    cross_family_collision — not silently drop only the regular index
    //    and leave the illegally-registered sorted definition behind.
    let mut b = Batch::new();
    b.id("di");
    b.drop_index("op", ddl::drop_index("shared_name", "users"));
    let req = b.to_request_via_msgpack();
    let err = db.execute("testdb", &req).await.unwrap_err();
    assert_eq!(
        err.code(),
        Some("cross_family_collision"),
        "expected code 'cross_family_collision', got: {:?} ({})",
        err.code(),
        err
    );
}
