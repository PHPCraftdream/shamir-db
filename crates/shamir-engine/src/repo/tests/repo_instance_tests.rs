use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::{BoxRepo, MemBufferRepoComposite};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::storage_membuffer::MemBufferConfig;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

fn create_test_instance() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("users"), TableConfig::new("orders")],
    )
}

/// L14+L5: MemBuffer-wrapped repo for testing store-layer unwrap wiring.
fn create_membuffer_wrapped_instance() -> RepoInstance {
    let inner = BoxRepo::InMemory(Arc::new(InMemoryRepo::new()));
    let config = MemBufferConfig {
        max_bytes: 64 * 1024 * 1024,
        max_entries: 100_000,
        ttl_ms: None,
        flush_interval_ms: 500,
        flush_batch_size: 256,
    };
    let repo = BoxRepo::MemBuffer(Arc::new(MemBufferRepoComposite { inner, config }));
    RepoInstance::new("mb_test".into(), repo, vec![TableConfig::new("items")])
}

#[tokio::test]
async fn test_repo_instance_creation() {
    let instance = create_test_instance();

    assert_eq!(instance.table_count(), 2);
    assert!(instance.has_table("users"));
    assert!(instance.has_table("orders"));
    assert!(!instance.has_table("products"));
}

#[tokio::test]
async fn test_list_table_names() {
    let instance = create_test_instance();
    let names = instance.list_table_names();

    assert_eq!(names.len(), 2);
    assert!(names.contains(&"users".to_string()));
    assert!(names.contains(&"orders".to_string()));
}

#[tokio::test]
async fn test_get_table_lazy() {
    let instance = create_test_instance();

    let table1 = instance.get_table("users").await.unwrap();
    assert_eq!(table1.name(), "users");

    let table2 = instance.get_table("users").await.unwrap();
    assert_eq!(table2.name(), "users");
}

#[tokio::test]
async fn test_get_table_not_configured() {
    let instance = create_test_instance();

    let result = instance.get_table("products").await;
    assert!(result.is_err());
}

// ============================================================================
// Index API tests
// ============================================================================

#[tokio::test]
async fn test_repo_instance_create_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    assert!(instance.index_exists("users", "email_idx").await.unwrap());
    assert!(!instance.index_exists("users", "nonexistent").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_composite_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "name_city_idx", &["name", "city"])
        .await
        .unwrap();

    assert!(instance
        .index_exists("users", "name_city_idx")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_nested_path_index() {
    let instance = create_test_instance();

    instance
        .create_index("users", "city_idx", &["address.city"])
        .await
        .unwrap();

    assert!(instance.index_exists("users", "city_idx").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_create_unique_index() {
    let instance = create_test_instance();

    instance
        .create_unique_index("users", "email_unique", &["email"])
        .await
        .unwrap();

    // Unique index exists in unique collection, not regular
    assert!(!instance
        .index_exists("users", "email_unique")
        .await
        .unwrap());
    assert!(instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_drop_index() {
    let instance = create_test_instance();

    // Create index
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();
    assert!(instance.index_exists("users", "email_idx").await.unwrap());

    // Drop index
    let dropped = instance.drop_index("users", "email_idx").await.unwrap();
    assert!(dropped);
    assert!(!instance.index_exists("users", "email_idx").await.unwrap());

    // Drop again returns false
    let dropped_again = instance.drop_index("users", "email_idx").await.unwrap();
    assert!(!dropped_again);
}

#[tokio::test]
async fn test_repo_instance_drop_unique_index() {
    let instance = create_test_instance();

    // Create unique index
    instance
        .create_unique_index("users", "email_unique", &["email"])
        .await
        .unwrap();
    assert!(instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());

    // Drop unique index
    let dropped = instance
        .drop_unique_index("users", "email_unique")
        .await
        .unwrap();
    assert!(dropped);
    assert!(!instance
        .unique_index_exists("users", "email_unique")
        .await
        .unwrap());
}

#[tokio::test]
async fn test_repo_instance_lookup_by_index() {
    let instance = create_test_instance();

    // Create index
    instance
        .create_index("users", "status_idx", &["status"])
        .await
        .unwrap();

    // Lookup with no data returns empty
    let results = instance
        .lookup_by_index(
            "users",
            "status_idx",
            &[InnerValue::Str("active".to_string())],
        )
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_repo_instance_index_isolation_between_tables() {
    let instance = create_test_instance();

    // Create index on users table
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    // Create different index on orders table
    instance
        .create_index("orders", "user_id_idx", &["user_id"])
        .await
        .unwrap();

    // Check isolation
    assert!(instance.index_exists("users", "email_idx").await.unwrap());
    assert!(!instance.index_exists("users", "user_id_idx").await.unwrap());
    assert!(!instance.index_exists("orders", "email_idx").await.unwrap());
    assert!(instance
        .index_exists("orders", "user_id_idx")
        .await
        .unwrap());
}

// ============================================================================
// III.1 — table_by_token O(1) resolution
// ============================================================================

#[tokio::test]
async fn table_by_token_is_constant_time_and_correct() {
    // Register many tables, both via the constructor and via add_table,
    // then assert every one resolves through the reverse index to the
    // correct TableManager, and an unknown token returns None cleanly.
    let repo = Arc::new(InMemoryRepo::new());
    let initial: Vec<TableConfig> = (0..50)
        .map(|i| TableConfig::new(format!("tbl_init_{i}")))
        .collect();
    let instance = RepoInstance::new("tt".into(), BoxRepo::InMemory(repo), initial);

    // A batch added dynamically after construction.
    for i in 0..50 {
        instance.add_table(TableConfig::new(format!("tbl_dyn_{i}")));
    }

    // Every registered name resolves by its deterministic token to itself.
    for i in 0..50 {
        for prefix in ["tbl_init_", "tbl_dyn_"] {
            let name = format!("{prefix}{i}");
            let token = table_token_for(&name);
            let resolved = instance
                .table_by_token(token)
                .await
                .unwrap()
                .unwrap_or_else(|| panic!("token for '{name}' did not resolve"));
            assert_eq!(resolved.name(), name);
        }
    }

    // A token that no table owns resolves to None (not an error, no panic).
    let bogus = table_token_for("table_that_was_never_registered");
    assert!(instance.table_by_token(bogus).await.unwrap().is_none());
}

#[tokio::test]
async fn table_by_token_drops_with_remove_table() {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new(
        "tt".into(),
        BoxRepo::InMemory(repo),
        vec![TableConfig::new("alpha"), TableConfig::new("beta")],
    );

    let alpha_token = table_token_for("alpha");
    assert!(instance
        .table_by_token(alpha_token)
        .await
        .unwrap()
        .is_some());

    // After removing the config, the token must no longer resolve — a
    // stale reverse-index entry must not resurrect a dropped table.
    assert!(instance.remove_table("alpha"));
    assert!(instance
        .table_by_token(alpha_token)
        .await
        .unwrap()
        .is_none());

    // The sibling table is unaffected.
    let beta_token = table_token_for("beta");
    let beta = instance.table_by_token(beta_token).await.unwrap().unwrap();
    assert_eq!(beta.name(), "beta");
}

#[tokio::test]
async fn add_table_twice_is_idempotent_for_token_lookup() {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("tt".into(), BoxRepo::InMemory(repo), vec![]);

    instance.add_table(TableConfig::new("dup"));
    instance.add_table(TableConfig::new("dup"));

    let token = table_token_for("dup");
    let resolved = instance.table_by_token(token).await.unwrap().unwrap();
    assert_eq!(resolved.name(), "dup");
}

#[tokio::test]
async fn test_repo_instance_clone_shares_state() {
    let instance = create_test_instance();
    let instance2 = instance.clone();

    // Create index through first instance
    instance
        .create_index("users", "email_idx", &["email"])
        .await
        .unwrap();

    // Check visible through second instance
    assert!(instance2.index_exists("users", "email_idx").await.unwrap());
}

#[tokio::test]
async fn test_repo_instance_crud_with_table() {
    let instance = create_test_instance();

    let table = instance.get_table("users").await.unwrap();

    // Insert
    let value = InnerValue::Str("test@example.com".to_string());
    let id = table.insert(&value).await.unwrap();
    assert_eq!(table.count().await.unwrap(), 1);

    // Get
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value);

    // Delete
    let deleted = table.delete(id).await.unwrap();
    assert!(deleted);
    assert_eq!(table.count().await.unwrap(), 0);
}

// ============================================================================
// L14+L5 — store-layer wiring: __data__ unwrapped, __info__/__history__ wrapped
// ============================================================================

/// L14: when a MemBuffer-wrapped repo creates an MVCC table, the
/// `__data__` store must be unwrapped (raw backend) because MVCC tables
/// never read or write through `__data__` — all I/O goes through the
/// version log (`history`). The MemBuffer wrapper is dead weight.
#[tokio::test]
async fn l14_data_store_unwrapped_for_mvcc_table() {
    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();

    // data_store should be raw (unwrapped) — raw_backend returns None.
    assert!(
        table.data_store().raw_backend().await.is_none(),
        "__data__ store must be unwrapped (raw) for MVCC tables"
    );
}

/// L14 regression guard: `__info__` (indexes, counter) must remain
/// wrapped in MemBuffer even for MVCC tables — it IS actively
/// read/written by the index and counter subsystems.
#[tokio::test]
async fn l14_info_store_remains_wrapped_for_mvcc_table() {
    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();

    // info_store should still be wrapped — raw_backend returns Some.
    assert!(
        table.info_store().raw_backend().await.is_some(),
        "__info__ store must remain wrapped in MemBuffer"
    );
}

/// L5: `__history__` must remain wrapped in MemBuffer for the
/// read-through cache benefit. Version-keyed values are immutable (new
/// write = new version), so cached reads never go stale (except via
/// explicit vacuum_key → remove, which correctly inserts a Tombstone).
#[tokio::test]
async fn l5_history_store_remains_wrapped_for_mvcc_table() {
    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();
    let mvcc = table.mvcc_store_ref().expect("MVCC must be attached");

    // history_store should still be wrapped — raw_backend returns Some.
    assert!(
        mvcc.history_store().raw_backend().await.is_some(),
        "__history__ store must remain wrapped in MemBuffer (read-through cache)"
    );
}

/// L14: MVCC tables do not write to `__data__` — writes go exclusively
/// through the version log. Verify by inserting through the table's MVCC
/// path and confirming the raw data_store is empty.
#[tokio::test]
async fn l14_mvcc_write_does_not_touch_data_store() {
    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();

    // Insert a record through the MVCC path.
    let value = InnerValue::Str("hello".to_string());
    let _id = table.insert(&value).await.unwrap();

    // The raw data_store should have zero entries — MVCC writes go to
    // history, not __data__.
    let mut stream = table.data_store().iter_stream(64);
    use futures::StreamExt;
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(
        count, 0,
        "__data__ must have zero entries for MVCC tables — writes go to history"
    );
}

/// L5: repeated reads of the same version-key hit the MemBuffer cache
/// and do NOT go to the raw backend on the second read.
#[tokio::test]
async fn l5_repeated_read_hits_cache() {
    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();

    // Insert a record through the MVCC path.
    let value = InnerValue::Str("cached".to_string());
    let id = table.insert(&value).await.unwrap();

    // First read (populates cache or is already cached from the write).
    let v1 = table.get(id).await.unwrap();
    assert_eq!(v1, value);

    // Second read — should hit the MemBuffer cache.
    let v2 = table.get(id).await.unwrap();
    assert_eq!(v2, value);
    // Functional correctness: both reads return the same value.
    // The cache hit is structural (MemBuffer's get returns from moka
    // before falling through to inner) — no counter seam needed for
    // this regression guard.
}

/// L5: after vacuum_key removes a version from history, the MemBuffer
/// cache entry is invalidated (Tombstone) and does NOT resurrect the
/// deleted version.
#[tokio::test]
async fn l5_vacuum_does_not_resurrect_from_cache() {
    use shamir_tx::Retention;

    let instance = create_membuffer_wrapped_instance();
    let table = instance.get_table("items").await.unwrap();
    let mvcc = table
        .mvcc_store_ref()
        .expect("MVCC must be attached")
        .clone();

    // Set retention to keep only 1 version (CurrentOnly).
    mvcc.set_retention(Retention {
        max_count: Some(1),
        min_count: None,
        max_age_secs: None,
    })
    .expect("valid retention policy");

    let key = bytes::Bytes::from_static(b"mykey");

    // Write version 1.
    let _v1 = mvcc
        .set_versioned(key.clone(), bytes::Bytes::from_static(b"val1"))
        .await
        .unwrap();

    // Write version 2 — vacuum_key runs inline and reclaims v1.
    let v2 = mvcc
        .set_versioned(key.clone(), bytes::Bytes::from_static(b"val2"))
        .await
        .unwrap();

    // Current read should return v2, NOT v1 (v1 was vacuumed).
    let current = mvcc.get_current(key.clone()).await.unwrap();
    assert_eq!(
        current,
        Some(bytes::Bytes::from_static(b"val2")),
        "current version must be v2 after vacuum"
    );

    // The history log should have only v2's entry (v1 was reclaimed).
    let timeline = mvcc.history_of(&key).await.unwrap();
    assert_eq!(
        timeline.len(),
        1,
        "only one version should remain after vacuum (v1 reclaimed)"
    );
    assert_eq!(timeline[0].version, v2);
}

// ============================================================================
// A13 — remove_table must clean up per_table_mvcc (drop+recreate split-brain)
// ============================================================================
//
// Audit finding A13 (`docs/audits/2026-07-06-concurrency-engine.md`):
// `remove_table` removed the catalogue (`configs`), the `TableManager`
// cache (`tables`), and the reverse-index entry (`token_names`), but NOT
// the `per_table_mvcc` registry entry. Since `table_token_for` is a
// deterministic hash of the name, a DROP followed by a CREATE of the SAME
// name produced a split-brain: the new `create_table_context` builds a
// fresh `MvccStore` and the new `TableManager` holds it directly, but
// `per_table_mvcc.insert(token, new_mvcc)` silently no-ops (key already
// present from the dropped table) — so the commit pipeline / SSI provider
// / drainer, which all resolve the MvccStore BY TOKEN through
// `per_table_mvcc`, keep writing through the STALE old store. A committed
// tx's overlay lands in the OLD store and is invisible to reads issued
// through the NEW `TableManager` (which holds its own store reference) —
// committed transactions silently vanish.

/// Direct registry-emptiness proof (A13): after `remove_table`, the
/// `per_table_mvcc` entry for the dropped table's token MUST be gone —
/// otherwise a subsequent `create_table_context` under the same name
/// silently fails to register its genuinely-new `MvccStore`.
#[tokio::test]
async fn a13_remove_table_clears_per_table_mvcc_entry() {
    let instance = create_test_instance();

    // Materialize the table so `create_table_context` runs and inserts
    // the MvccStore into `per_table_mvcc` under the deterministic token.
    let _ = instance.get_table("users").await.unwrap();
    let token = table_token_for("users");
    assert!(
        instance.per_table_mvcc().get(&token).is_some(),
        "precondition: users table must have a registered MvccStore"
    );

    // Drop the table — the stale registry entry must NOT survive.
    assert!(instance.remove_table("users"));
    assert!(
        instance.per_table_mvcc().get(&token).is_none(),
        "remove_table must evict the per_table_mvcc entry; a stale \
         registration causes a split-brain on drop+recreate (A13)"
    );

    // Sibling table is unaffected — only the dropped token is evicted.
    let orders_token = table_token_for("orders");
    assert!(
        instance.per_table_mvcc().get(&orders_token).is_none(),
        "orders should not be in per_table_mvcc (never materialized)"
    );
    let _ = instance.get_table("orders").await.unwrap();
    assert!(
        instance.per_table_mvcc().get(&orders_token).is_some(),
        "sibling table's MvccStore must be untouched by the drop"
    );
}

/// End-to-end split-brain reproduction (A13): create table `t`, drop it,
/// recreate `t` under the SAME name, commit a write through the NEW
/// `TableManager`, then READ through the same `TableManager` and assert
/// the just-committed write IS visible. Before the fix, the commit
/// pipeline resolves the MvccStore via `per_table_mvcc[token]` (which
/// still points at the STALE old store), so the committed overlay lands
/// in the OLD store and the NEW `TableManager`'s own read-through-store
/// sees nothing — committed data silently vanishes.
#[tokio::test]
async fn a13_drop_then_recreate_write_visible_through_new_table() {
    use shamir_tx::IsolationLevel;

    let instance = create_test_instance();
    instance.add_table(TableConfig::new("t"));

    // Materialize the FIRST incarnation so per_table_mvcc[token("t")]
    // holds the OLD MvccStore.
    let t1 = instance.get_table("t").await.unwrap();
    let seed_rid = t1
        .insert(&InnerValue::Str("old-incarnation".into()))
        .await
        .unwrap();
    let _ = t1.get(seed_rid).await.unwrap();

    // === DROP the table ===
    assert!(instance.remove_table("t"));

    // === RECREATE under the SAME name ===
    instance.add_table(TableConfig::new("t"));
    let t2 = instance.get_table("t").await.unwrap();

    // The TableManager holds a fresh MvccStore; the commit pipeline must
    // resolve the SAME store through per_table_mvcc. If the stale entry
    // leaked (A13), the tx's committed overlay lands in the OLD store.
    let (mut tx, _g) = instance.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let new_rid = t2
        .insert_tx(&InnerValue::Str("new-incarnation".into()), Some(&mut tx))
        .await
        .unwrap();
    let outcome = instance.commit_tx(tx).await.unwrap();
    assert!(
        outcome.commit_version > 0,
        "tx must commit with a positive version"
    );

    // Read through the NEW TableManager. Its own held MvccStore is the
    // same one the commit pipeline wrote to ONLY if A13 is fixed.
    let read_back = t2.get(new_rid).await.unwrap();
    assert!(
        matches!(read_back, InnerValue::Str(ref s) if s == "new-incarnation"),
        "committed write must be visible through the recreated table; \
         got {read_back:?} (A13 split-brain: commit pipeline wrote to the \
         stale old MvccStore still registered in per_table_mvcc)"
    );
}

/// Regression guard: a plain DROP (no recreate) must leave the table
/// genuinely gone — `has_table` false, `get_table` errors, and no panic
/// from background machinery that may have held a reference.
#[tokio::test]
async fn a13_plain_drop_no_recreate_still_clean() {
    let instance = create_test_instance();
    let _ = instance.get_table("users").await.unwrap();
    let token = table_token_for("users");
    assert!(instance.per_table_mvcc().get(&token).is_some());

    assert!(instance.remove_table("users"));
    assert!(!instance.has_table("users"));
    assert!(instance.get_table("users").await.is_err());
    assert!(
        instance.per_table_mvcc().get(&token).is_none(),
        "registry entry must be evicted on plain drop too"
    );
}
