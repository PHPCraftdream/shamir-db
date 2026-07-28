//! `DROP INDEX` support for index2 (fts / functional / vector) and sorted
//! indexes — scenario tests for the name-resolution + posting-cleanup +
//! persist-round-trip work (#872).
//!
//! These go through the FULL DDL path (`ShamirDb::execute` →
//! `handle_drop_index`), not a direct `TableManager` call, because the
//! resolution-order wiring and the `if_exists` early-exit live in the
//! handler and are the actual subject of the change.

use shamir_db::engine::index2::legacy::sorted_index_manager::SortedIndexManager;
use shamir_db::engine::index2::persistence::load_index2_metadata;
use shamir_db::engine::index2::posting_layout::type_tag;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_storage::types::Store;

use super::helpers::*;

/// Count every key under `prefix` in `store` (drains the whole stream).
async fn count_prefix(store: &std::sync::Arc<dyn Store>, prefix: bytes::Bytes) -> usize {
    use futures::StreamExt;
    let mut stream = store.scan_prefix_stream(prefix, 1024);
    let mut count = 0;
    while let Some(batch) = stream.next().await {
        count += batch.expect("prefix scan batch").len();
    }
    count
}

// =====================================================================
// Scenario 1: drop an index2 backend by name via the DDL path.
//   (a) gone from the live index2_registry,
//   (b) a persist round-trip (reload of `__meta__/indexes`, exactly what
//       `TableManager::create` reads on reopen) does NOT resurrect it,
//   (c) its postings are actually cleaned up by `drop_all()`.
// =====================================================================
#[tokio::test]
async fn drop_index2_backend_by_name_via_ddl() {
    let db = setup_db().await;

    // 1. Create a functional index2 backend on "users.name".
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

    // 2. Insert records so the backend has postings to clean up.
    {
        let mut b = Batch::new();
        b.id("ins");
        b.insert(
            "op",
            shamir_query_builder::write::insert("users")
                .row(shamir_query_builder::doc! { "name" => "Alice" })
                .row(shamir_query_builder::doc! { "name" => "Bob" })
                .row(shamir_query_builder::doc! { "name" => "Charlie" }),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.expect("insert rows");
    }

    // 3. Capture the backend's compact id + the info_store handle, and prove
    //    postings exist BEFORE the drop (otherwise the cleanup assertion
    //    below would be vacuous). Functional posting key layout is
    //    [index_id LE u32][type_tag u8 = FUNCTIONAL].
    let (info_store, posting_prefix) = {
        let table = db.get_table("testdb", "main", "users").await.unwrap();
        let interner = table.interner().get().await.unwrap();
        let name_key = interner
            .get_ind("func_idx")
            .expect("func_idx should be interned");
        let backend = table
            .index2_registry()
            .get_by_name(name_key.id())
            .await
            .expect("functional backend should be registered");
        let backend_id = backend.descriptor().id;

        let mut prefix = Vec::with_capacity(5);
        prefix.extend_from_slice(&backend_id.to_le_bytes());
        prefix.push(type_tag::FUNCTIONAL);
        let prefix = bytes::Bytes::from(prefix);

        let info_store = std::sync::Arc::clone(table.info_store());
        let before = count_prefix(&info_store, prefix.clone()).await;
        assert!(
            before > 0,
            "expected functional postings before drop, got {before}"
        );
        (info_store, prefix)
    };

    // 4. DROP INDEX via the DDL path.
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("func_idx", "users"));
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop index");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "DROP INDEX on an index2 backend must report existed:true"
    );

    // (a) gone from the live index2_registry.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    let backends = table.index2_registry().all_backends().await;
    assert!(
        backends.iter().all(|b| b.descriptor().name != "func_idx"),
        "index2 backend must be gone from the registry after drop"
    );

    // (c) postings actually cleaned up by drop_all() — not just
    //     registry-removed. Same prefix that was non-empty before is now
    //     empty.
    let after = count_prefix(&info_store, posting_prefix).await;
    assert_eq!(
        after, 0,
        "functional postings must be cleaned by drop_all() after DROP INDEX"
    );

    // (b) persist round-trip: reload `__meta__/indexes` (exactly what
    //     `TableManager::create` reads on reopen) and confirm the descriptor
    //     does NOT resurrect.
    let persisted = load_index2_metadata(&info_store).await.unwrap();
    let descriptors = persisted.map(|p| p.descriptors).unwrap_or_default();
    assert!(
        descriptors.iter().all(|d| d.name != "func_idx"),
        "dropped index2 backend must not be in persisted metadata (would resurrect on reopen)"
    );
}

// =====================================================================
// Scenario 2: drop a sorted index by name via the DDL path, with a
// persist-round-trip check.
// =====================================================================
#[tokio::test]
async fn drop_sorted_index_by_name_via_ddl() {
    let db = setup_db().await;

    // 1. Create a sorted index on "users.score".
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "op",
            ddl::create_index("score_sorted", "users")
                .field("score")
                .sorted(),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req)
            .await
            .expect("create sorted index");
    }

    // 2. Insert a couple records so the index has entries.
    {
        let mut b = Batch::new();
        b.id("ins");
        b.insert(
            "op",
            shamir_query_builder::write::insert("users")
                .row(shamir_query_builder::doc! { "score" => 10 })
                .row(shamir_query_builder::doc! { "score" => 20 }),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.expect("insert rows");
    }

    // 3. Verify the sorted index is live before the drop (resolution + existence).
    {
        let table = db.get_table("testdb", "main", "users").await.unwrap();
        assert!(
            table.sorted_index_exists("score_sorted").await,
            "sorted index should exist before drop"
        );
    }

    // 4. DROP INDEX via the DDL path.
    let removed = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("score_sorted", "users"));
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop sorted index");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed,
        Some(true),
        "DROP INDEX on a sorted index must report existed:true"
    );

    // (a) gone from the live sorted-index set.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.sorted_index_exists("score_sorted").await,
        "sorted index must be gone from the live set after drop"
    );

    // (b) persist round-trip: reload the persisted sorted defs (what reopen
    //     reads) from the info_store via a fresh `SortedIndexManager` and
    //     confirm the dropped name does NOT resurrect.
    let info_store = std::sync::Arc::clone(table.info_store());
    let reloaded = SortedIndexManager::new(info_store).await.unwrap();
    let defs = reloaded.iter_indexes();
    assert!(
        defs.is_empty(),
        "dropped sorted index must not be in persisted defs (would resurrect on reopen): {defs:?}"
    );
}

// =====================================================================
// Scenario 3: `if_exists: true` correctly reports `existed: true` for an
// index2 / sorted index (the early-exit must NOT no-op them as "absent").
// Before the fix the early-exit only checked the two legacy mechanisms, so
// an existing index2/sorted index was wrongly reported as `existed: false`.
// =====================================================================
#[tokio::test]
async fn drop_index_if_exists_reports_existed_true_for_index2_and_sorted() {
    let db = setup_db().await;

    // Create one index2 (functional) and one sorted index.
    {
        let mut b = Batch::new();
        b.id("ci");
        b.create_index(
            "f",
            ddl::create_index("func_idx", "users")
                .field("name")
                .index_type("functional")
                .functional_op("lower"),
        );
        b.create_index(
            "s",
            ddl::create_index("score_sorted", "users")
                .field("score")
                .sorted(),
        );
        let req = b.to_request_via_msgpack();
        db.execute("testdb", &req).await.expect("create indexes");
    }

    // DROP INDEX ... IF EXISTS on the index2 backend → existed:true (recognized).
    let removed_index2 = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("func_idx", "users").if_exists());
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop index2");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed_index2,
        Some(true),
        "if_exists early-exit must recognize an existing index2 backend"
    );

    // DROP INDEX ... IF EXISTS on the sorted index → existed:true (recognized).
    let removed_sorted = {
        let mut b = Batch::new();
        b.id("di");
        b.drop_index("op", ddl::drop_index("score_sorted", "users").if_exists());
        let req = b.to_request_via_msgpack();
        let resp = db.execute("testdb", &req).await.expect("drop sorted");
        resp.results["op"].records[0].get_value_bool("existed")
    };
    assert_eq!(
        removed_sorted,
        Some(true),
        "if_exists early-exit must recognize an existing sorted index"
    );

    // Both must actually be gone.
    let table = db.get_table("testdb", "main", "users").await.unwrap();
    assert!(
        !table.index2_exists("func_idx").await,
        "index2 backend must be gone after drop"
    );
    assert!(
        !table.sorted_index_exists("score_sorted").await,
        "sorted index must be gone after drop"
    );
}

// =====================================================================
// Scenario 4 (regression guard): dropping a non-existent name with
// `if_exists: false` preserves the pre-existing behavior exactly — the
// common btree case is unchanged by the new sorted/index2 fallbacks.
// =====================================================================
#[tokio::test]
async fn drop_index_nonexistent_without_if_exists_is_unchanged() {
    let db = setup_db().await;

    let mut b = Batch::new();
    b.id("di");
    // No if_exists → must NOT error (current behavior returns existed:false),
    // and none of the four mechanisms should spuriously match a phantom name.
    b.drop_index("op", ddl::drop_index("no_such_idx", "users"));
    let req = b.to_request_via_msgpack();
    let resp = db
        .execute("testdb", &req)
        .await
        .expect("drop non-existent index");

    // Pre-existing behavior: a non-existent index reports existed:false (a
    // silent no-op result, NOT an error). The new sorted/index2 fallbacks
    // must not change this — they all return Ok(false) for a name no
    // mechanism has.
    assert_eq!(
        resp.results["op"].records[0].get_value_bool("existed"),
        Some(false),
        "dropping a non-existent index with if_exists:false must be unchanged (existed:false)"
    );
}
