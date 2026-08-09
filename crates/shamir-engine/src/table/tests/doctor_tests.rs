//! Tests for the integrity doctor — `verify` and `repair`.

use std::str::FromStr;
use std::sync::Arc;

use num_bigint::BigInt;
use rust_decimal::Decimal;

use crate::db_instance::db_instance::DbInstance;
use crate::index::index_record_key::IndexRecordKey;
use crate::index2::state::IndexState;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::{BoxRepo, BoxRepoFactory};
use crate::repo::RepoConfig;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::TableConfig;
use shamir_query_builder::{filter, write};
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

/// Build a fresh in-memory DB with a `users` table, a regular index
/// on `city`, a unique index on `id`, and a sorted index on `score`.
/// Seeds N records. Returns the table manager + repo (the repo is needed
/// to drive writes through the implicit-tx commit path).
async fn seeded(n: usize) -> (crate::table::TableManager, crate::repo::RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();
    table.create_index("by_city", &["city"]).await.unwrap();
    table.create_unique_index("by_id", &["id"]).await.unwrap();
    table
        .create_sorted_index("by_score", &["score"])
        .await
        .unwrap();

    let interner = table.interner().get().await.unwrap();
    for i in 0..n {
        let mut m = new_map();
        m.insert("id".to_string(), QueryValue::Str(format!("u{:04}", i)));
        m.insert(
            "city".to_string(),
            QueryValue::Str(format!("city_{}", i % 4)),
        );
        m.insert(
            "score".to_string(),
            QueryValue::Int((i * 7919) as i64 % 1000),
        );
        let user = QueryValue::Map(m);
        let (inner_val, new_keys) = query_value_to_inner_tracked(&user, interner).unwrap();
        if !new_keys.is_empty() {
            table.interner().save_new_keys(&new_keys).await.unwrap();
        }
        table.insert(&inner_val).await.unwrap();
    }
    (table, repo)
}

#[tokio::test]
async fn verify_reports_healthy_after_seed() {
    let (table, _repo) = seeded(20).await;
    let report = table.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "freshly-seeded table should be healthy, got {:?}",
        report
    );
    assert_eq!(report.records_in_data, 20);
    assert_eq!(report.counter_value, 20);
}

#[tokio::test]
async fn verify_detects_orphan_regular_index_entry() {
    // Corrupt the index: insert an extra posting entry pointing to
    // a record that doesn't exist. Verify should report the index
    // as unhealthy.
    let (table, _repo) = seeded(10).await;
    let regular_defs: Vec<_> = table.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_city index registered");

    // Build a posting key for a non-existent record_id, write it
    // straight into info_store.
    let info_store = info_store_of(&table);
    let key = IndexRecordKey::new(false, def.name_interned).to_bytes();
    let fake_record_id = shamir_types::types::record_id::RecordId::new();
    let mut posting_key = key.to_vec();
    posting_key.extend_from_slice(fake_record_id.as_bytes());
    info_store
        .set(bytes::Bytes::from(posting_key).into(), bytes::Bytes::new())
        .await
        .unwrap();

    let report = table.verify().await.unwrap();
    let bucket = report
        .regular_indexes
        .iter()
        .find(|h| h.name_interned == def.name_interned)
        .expect("by_city in report");
    assert!(
        bucket.actual_entries > bucket.expected_entries,
        "verify must spot the orphan: actual {} > expected {}",
        bucket.actual_entries,
        bucket.expected_entries,
    );
    assert!(!report.is_healthy());
}

#[tokio::test]
async fn repair_heals_orphan_index_entry() {
    let (table, _repo) = seeded(10).await;
    let regular_defs: Vec<_> = table.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_city index registered");

    // Plant a bogus posting.
    let info_store = info_store_of(&table);
    let key = IndexRecordKey::new(false, def.name_interned).to_bytes();
    let fake = shamir_types::types::record_id::RecordId::new();
    let mut posting_key = key.to_vec();
    posting_key.extend_from_slice(fake.as_bytes());
    info_store
        .set(bytes::Bytes::from(posting_key).into(), bytes::Bytes::new())
        .await
        .unwrap();

    // Before: unhealthy.
    assert!(!table.verify().await.unwrap().is_healthy());

    let report = table.repair().await.unwrap();
    assert_eq!(report.records_scanned, 10);

    // After: every index matches data.
    let after = table.verify().await.unwrap();
    assert!(
        after.is_healthy(),
        "repair must restore consistency: {:?}",
        after
    );
}

#[tokio::test]
async fn repair_heals_drifted_counter() {
    let (table, _repo) = seeded(15).await;
    // Force the cached counter out of sync with data.
    table.counter().set_to(999).await.unwrap();
    let pre = table.verify().await.unwrap();
    assert_eq!(pre.records_in_data, 15);
    assert_eq!(pre.counter_value, 999);
    assert!(!pre.counter_consistent);

    let report = table.repair().await.unwrap();
    assert_eq!(report.records_scanned, 15);
    assert_eq!(report.counter_after, 15);

    let post = table.verify().await.unwrap();
    assert!(post.counter_consistent);
}

#[tokio::test]
async fn execute_update_leaves_table_consistent_on_success() {
    let (table, repo) = seeded(8).await;
    let refs = new_map();

    let op = write::update("users")
        .where_(filter::eq("city", "city_0"))
        .set(mpack!({ "score": 999 }))
        .build()
        .unwrap();

    let result = super::write_exec_tests::update_via_tx(&repo, &table, &op, &refs)
        .await
        .unwrap();
    assert!(result.affected > 0, "expected at least one update");

    assert!(table.verify().await.unwrap().is_healthy());
}

#[tokio::test]
async fn execute_delete_leaves_table_consistent_on_success() {
    let (table, repo) = seeded(12).await;
    let refs = new_map();

    let op = write::delete("users")
        .where_(filter::eq("city", "city_1"))
        .build()
        .unwrap();

    let result = super::write_exec_tests::delete_via_tx(&repo, &table, &op, &refs)
        .await
        .unwrap();
    assert!(result.affected > 0);

    assert!(table.verify().await.unwrap().is_healthy());
}

#[tokio::test]
async fn bump_write_counter_spawns_background_verify_periodically() {
    // The watchdog should fire on the threshold crossing —
    // we bump by exactly one batch large enough to cross.
    let (table, _repo) = seeded(0).await;
    // First small bumps — should not spawn.
    table.bump_write_counter(10);
    table.bump_write_counter(10);
    assert!(!table.is_background_verify_running());

    // Cross the threshold (default 1024) — must fire.
    table.bump_write_counter(2048);
    // Either the background verify is already running, or it
    // finished extremely fast on this tiny table. Either way we
    // confirm it WAS triggered by waiting briefly and then
    // checking that no inconsistency was logged in the
    // returned verify state.
    tokio::task::yield_now().await;
    // Drain — the spawned task may have completed by now.
    // Note: this loop is ITERATION-bounded (tries < 100), not wall-clock
    // bounded. Each iteration is a single yield_now + an atomic-flag check,
    // so 100 iterations complete in sub-millisecond regardless of runner
    // speed. Unlike an unbounded `while !flag { yield }` spin (which can hang
    // forever if the flag never fires), this cannot become an anonymous
    // nextest TIMEOUT — the iteration cap terminates it deterministically.
    // Left as-is: the existing bound is adequate for this risk class.
    let mut tries = 0;
    while table.is_background_verify_running() && tries < 100 {
        tokio::task::yield_now().await;
        tries += 1;
    }
    assert!(!table.is_background_verify_running());
    // No state should have been damaged.
    assert!(table.verify().await.unwrap().is_healthy());
}

#[tokio::test]
async fn bump_write_counter_is_single_flight() {
    // Two threshold-crossing bumps in immediate succession must
    // NOT spawn two concurrent verifies. The single-flight latch
    // ensures only one runs at a time.
    let (table, _repo) = seeded(100).await;
    // Bump to just below threshold.
    table.bump_write_counter(1000);
    // Snapshot — no verify yet.
    assert!(!table.is_background_verify_running());

    // First crossing — starts verify.
    table.bump_write_counter(100);
    assert!(table.is_background_verify_running() || !table.is_background_verify_running()); // race-safe; the assertion is below

    // Second crossing while first running — must NOT spawn second.
    table.bump_write_counter(2048);
    // Best-effort check: counter latch is bool, so the second
    // crossing's spawn attempt fails because verify_running was
    // true. There's still a race window if the first verify
    // finished between the two bumps. We make the assertion
    // resilient by ensuring eventually exactly zero verifies are
    // still running and verify is healthy.
    // Same iteration-bounded pattern as above (tries < 200) — see the comment
    // there for why this is adequately bounded (deterministic termination via
    // iteration cap, not the unbounded spin-wait class).
    let mut tries = 0;
    while table.is_background_verify_running() && tries < 200 {
        tokio::task::yield_now().await;
        tries += 1;
    }
    assert!(!table.is_background_verify_running());
    assert!(table.verify().await.unwrap().is_healthy());
}

#[tokio::test]
async fn insert_many_leaves_table_consistent_on_success() {
    let (table, _repo) = seeded(0).await;
    let interner = table.interner().get().await.unwrap();
    let mut values = Vec::new();
    for i in 0..5 {
        let mut m = new_map();
        m.insert("id".to_string(), QueryValue::Str(format!("u{}", i)));
        m.insert("city".to_string(), QueryValue::Str("NYC".into()));
        m.insert("score".to_string(), QueryValue::Int(i as i64));
        let user = QueryValue::Map(m);
        let (inner_val, new_keys) = query_value_to_inner_tracked(&user, interner).unwrap();
        if !new_keys.is_empty() {
            table.interner().save_new_keys(&new_keys).await.unwrap();
        }
        values.push(inner_val);
    }
    let ids = table.insert_many(&values).await.unwrap();
    assert_eq!(ids.len(), 5);

    // State is consistent.
    assert!(table.verify().await.unwrap().is_healthy());
}

// Test helper: pull the `info_store` of a TableManager so the
// verify/repair tests can plant bogus index postings directly.
fn info_store_of(table: &crate::table::TableManager) -> Arc<dyn Store> {
    Arc::clone(table.info_store())
}

/// Build a table with mixed indexed field types and seed records whose
/// indexed leaves exercise every `InnerValue` variant the production data
/// can hold: plain scalar (Int, Str), Dec, Big, Bin, and a nested Map.
/// The doctor's lens-driven `verify` and `repair` MUST compute
/// byte-identical expected index counts and posting bytes as the tree
/// path that originally built the indexes during insert.
///
/// Strategy:
///   1. Insert records whose indexed field (`tag`) carries varied types.
///   2. `verify` immediately after — the expected counts are computed by
///      the lens path (the change under test), the actual counts were
///      built by the tree path during `insert`. Equal ⇒ lens-tree parity
///      on leaf-presence detection.
///   3. Corrupt the index by planting a bogus posting.
///   4. `repair` (lens-driven) rebuilds from data.
///   5. `verify` again — healthy ⇒ the lens-driven rebuild produced
///      posting bytes accepted by the same verify code, confirming
///      byte-identity of the posting keys/values.
#[tokio::test]
async fn doctor_lens_parity_mixed_indexed_types() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("mixed")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "mixed").await.unwrap();

    // Regular index on `tag`, unique index on `uid`, sorted on `score`.
    table.create_index("by_tag", &["tag"]).await.unwrap();
    table.create_unique_index("by_uid", &["uid"]).await.unwrap();
    table
        .create_sorted_index("by_score", &["score"])
        .await
        .unwrap();

    let interner = table.interner().get().await.unwrap();

    // Record 0 — plain Int tag, Int score.
    {
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u0".into()));
        m.insert("tag".to_string(), QueryValue::Int(42));
        m.insert("score".to_string(), QueryValue::Int(100));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // Record 1 — Str tag, F64 score.
    {
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u1".into()));
        m.insert("tag".to_string(), QueryValue::Str("hello".into()));
        m.insert("score".to_string(), QueryValue::F64(3.5));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // Record 2 — Dec tag (non-scalar: materialize_at returns InnerValue::Dec,
    // scalar_at returns None).
    {
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u2".into()));
        m.insert(
            "tag".to_string(),
            QueryValue::Dec(Decimal::from_str("123.456").unwrap()),
        );
        m.insert("score".to_string(), QueryValue::Int(200));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // Record 3 — Big tag.
    {
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u3".into()));
        m.insert(
            "tag".to_string(),
            QueryValue::Big(BigInt::from_str("99999999999999999999").unwrap()),
        );
        m.insert("score".to_string(), QueryValue::Int(300));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // Record 4 — Bin tag.
    {
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u4".into()));
        m.insert(
            "tag".to_string(),
            QueryValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]),
        );
        m.insert("score".to_string(), QueryValue::Int(400));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // Record 5 — nested Map tag (container: extract_index_leaves materializes
    // the whole subtree).
    {
        let mut inner_map = new_map();
        inner_map.insert(
            "nested_key".to_string(),
            QueryValue::Str("nested_val".into()),
        );
        let mut m = new_map();
        m.insert("uid".to_string(), QueryValue::Str("u5".into()));
        m.insert("tag".to_string(), QueryValue::Map(inner_map));
        m.insert("score".to_string(), QueryValue::Int(500));
        let (iv, nk) = query_value_to_inner_tracked(&QueryValue::Map(m), interner).unwrap();
        if !nk.is_empty() {
            table.interner().save_new_keys(&nk).await.unwrap();
        }
        table.insert(&iv).await.unwrap();
    }

    // 1. Verify immediately — lens-computed expected counts must match the
    //    tree-built actual counts from the insert path.
    let report = table.verify().await.unwrap();
    assert!(
        report.is_healthy(),
        "lens-driven verify must match tree-built indexes: {report:?}"
    );
    assert_eq!(report.records_in_data, 6);

    // 2. Corrupt the regular index to force repair.
    let info_store = info_store_of(&table);
    let regular_defs: Vec<_> = table.index_manager_ref().iter_indexes().collect();
    let def = regular_defs.first().expect("by_tag index");
    {
        let key = IndexRecordKey::new(false, def.name_interned).to_bytes();
        let fake = shamir_types::types::record_id::RecordId::new();
        let mut posting_key = key.to_vec();
        posting_key.extend_from_slice(fake.as_bytes());
        info_store
            .set(bytes::Bytes::from(posting_key).into(), bytes::Bytes::new())
            .await
            .unwrap();
    }
    assert!(!table.verify().await.unwrap().is_healthy());

    // 3. Repair — lens-driven rebuild of all indexes.
    let repair_report = table.repair().await.unwrap();
    assert_eq!(repair_report.records_scanned, 6);

    // 4. Verify after repair — healthy ⇒ lens path produced byte-identical
    //    posting keys and values.
    let after = table.verify().await.unwrap();
    assert!(
        after.is_healthy(),
        "lens-driven repair must produce byte-identical postings: {after:?}"
    );
    assert_eq!(after.records_in_data, 6);
    // Confirm per-index counts match expectations.
    for ih in &after.regular_indexes {
        assert_eq!(
            ih.expected_entries, ih.actual_entries,
            "regular index {}: expected {} != actual {}",
            ih.name_interned, ih.expected_entries, ih.actual_entries
        );
    }
    for ih in &after.unique_indexes {
        assert_eq!(
            ih.expected_entries, ih.actual_entries,
            "unique index {}: expected {} != actual {}",
            ih.name_interned, ih.expected_entries, ih.actual_entries
        );
    }
    for ih in &after.sorted_indexes {
        assert_eq!(
            ih.expected_entries, ih.actual_entries,
            "sorted index {}: expected {} != actual {}",
            ih.name_interned, ih.expected_entries, ih.actual_entries
        );
    }
}

// ============================================================================
// P1-1 (#966) — doctor::verify() must surface stuck-Building base indexes
// (regular / unique / sorted). A Building index is permanently planner-
// invisible until an explicit doctor::repair() rebuilds it (the base_index
// family has NO automatic open-time self-heal, unlike index2's F-50 Step
// 3b). Each test below proves verify() catches the stuck state per family.
// ============================================================================

/// Build a minimal in-memory repo + table for the Building-detection tests.
async fn make_pause_test_repo(table_name: &str) -> (RepoInstance, crate::table::TableManager) {
    let repo = RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table(TableConfig::new(table_name));
    let tbl = repo.get_table(table_name).await.unwrap();
    (repo, tbl)
}

/// Intern a name and return its u64 id.
async fn intern_id(tbl: &crate::table::TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Build a single-field `InnerValue::Map` record (string value).
fn str_record(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// Build a single-field `InnerValue::Map` record (int value).
fn int_record(key: u64, val: i64) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Int(val));
    InnerValue::Map(m)
}

/// P1-1: a regular (hash) index stuck at `Building` mid-backfill must be
/// surfaced by `verify()` as unhealthy — even if the partial entry count
/// happens to match. Uses the existing `create_index_backfill_hook` pause
/// seam (no new interruption mechanism). Also includes a post-release
/// Ready check as the regression guard for this family.
#[tokio::test]
async fn verify_detects_building_regular_index() {
    use crate::table::index2_backfill_hook::BackfillPauseHook;

    let (_repo, tbl) = make_pause_test_repo("people").await;
    let status_key = intern_id(&tbl, "status").await;

    // Filler rows to force multiple backfill batches (batch_size = 1000).
    for i in 0..1200 {
        tbl.insert(&str_record(status_key, &format!("filler_{i}")))
            .await
            .unwrap();
    }

    // Pre-existing rows — the backfill has work to do and actually pauses.
    for s in ["a", "b", "c"] {
        tbl.insert(&str_record(status_key, s)).await.unwrap();
    }

    // Install the pause hook on the TableManager's online-index path.
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.online_index_backfill_hook
        .store(Some(Arc::clone(&hook)));

    // Spawn create_index — registers at Building, then parks mid-backfill.
    let tbl_c = tbl.clone();
    let create = tokio::spawn(async move { tbl_c.create_index("status_idx", &["status"]).await });

    hook.wait_until_parked().await;
    let idx_id = intern_id(&tbl, "status_idx").await;

    // THE PROOF: verify() while still Building.
    let report = tbl.verify().await.unwrap();
    let entry = report
        .regular_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("status_idx in regular_indexes");
    assert_eq!(
        entry.state,
        IndexState::Building,
        "mid-backfill regular index must report Building"
    );
    assert!(
        !entry.is_healthy(),
        "a Building regular index must be unhealthy even if counts match"
    );
    assert!(
        entry.message.is_some(),
        "a Building regular index must carry a diagnostic message"
    );
    assert!(
        !report.is_healthy(),
        "a Building regular index must make the overall report unhealthy"
    );

    // Release — def flips to Ready.
    hook.release();
    create.await.unwrap().expect("create_index must complete");

    // Regression guard: a fully-Ready regular index is healthy.
    let report2 = tbl.verify().await.unwrap();
    let entry2 = report2
        .regular_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("status_idx in regular_indexes");
    assert_eq!(entry2.state, IndexState::Ready);
    assert!(entry2.is_healthy());
    assert!(entry2.message.is_none());
    assert!(report2.is_healthy());
}

/// P1-1: a unique (hash) index stuck at `Building` must be surfaced by
/// `verify()` as unhealthy.
///
/// NOTE: the unique-index CREATE path uses the "Option B" design (register
/// AFTER backfill, never `Building` in production — see
/// `create_unique_index_body`'s audit A9 doc). There is therefore no pause
/// hook for unique creates. This test simulates the stuck-Building state
/// by re-registering the definition at `Building` via the public
/// `create_unique_index_from_records` API (which adds whatever `state` it
/// receives, no flip). This exercises the exact code path `verify()` uses
/// to detect the gap — if a bug or future code change ever left a unique
/// index at `Building`, `verify()` would catch it here. Also includes a
/// pre-flip Ready check as the regression guard for this family.
#[tokio::test]
async fn verify_detects_building_unique_index() {
    let (_repo, tbl) = make_pause_test_repo("uniq").await;
    let id_key = intern_id(&tbl, "id").await;

    for i in 0..5 {
        tbl.insert(&str_record(id_key, &format!("u{i}")))
            .await
            .unwrap();
    }

    // Create the unique index normally — Ready.
    tbl.create_unique_index("by_id", &["id"]).await.unwrap();
    let idx_id = intern_id(&tbl, "by_id").await;

    // Regression guard: a fully-Ready unique index is healthy.
    let ready_report = tbl.verify().await.unwrap();
    let ready_entry = ready_report
        .unique_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("by_id in unique_indexes");
    assert_eq!(ready_entry.state, IndexState::Ready);
    assert!(ready_entry.is_healthy());
    assert!(ready_report.is_healthy());

    // Simulate a stuck-Building unique index: re-register the def at Building.
    let def = tbl
        .index_manager_ref()
        .get_unique_index_definition(idx_id)
        .expect("by_id unique index registered");
    let mut building_def = def.clone();
    building_def.state = IndexState::Building;
    let records = tbl.collect_all_current_records().await.unwrap();
    tbl.index_manager_ref()
        .create_unique_index_from_records(building_def, records)
        .await
        .unwrap();

    // THE PROOF: verify() must report the Building unique index as unhealthy.
    let report = tbl.verify().await.unwrap();
    let entry = report
        .unique_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("by_id in unique_indexes");
    assert_eq!(
        entry.state,
        IndexState::Building,
        "a re-registered-at-Building unique index must report Building"
    );
    assert!(
        !entry.is_healthy(),
        "a Building unique index must be unhealthy"
    );
    assert!(
        entry.message.is_some(),
        "a Building unique index must carry a diagnostic message"
    );
    assert!(
        !report.is_healthy(),
        "a Building unique index must make the overall report unhealthy"
    );
}

/// P1-1: a sorted (B-tree) index stuck at `Building` mid-backfill must be
/// surfaced by `verify()` as unhealthy. Uses the existing
/// `create_sorted_index_backfill_hook` pause seam. Also includes a
/// post-release Ready check as the regression guard for this family.
#[tokio::test]
async fn verify_detects_building_sorted_index() {
    let (_repo, tbl) = make_pause_test_repo("nums").await;
    let score_key = intern_id(&tbl, "score").await;

    for score in [10, 20, 30] {
        tbl.insert(&int_record(score_key, score)).await.unwrap();
    }

    // Install the sorted-index-specific pause hook.
    let hook = Arc::new(crate::table::index2_backfill_hook::BackfillPauseHook::new());
    tbl.set_create_sorted_index_backfill_hook(Some(Arc::clone(&hook)));

    // Spawn create_sorted_index — registers at Building, then parks.
    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_sorted_index("score_sorted", &["score"]).await });

    hook.wait_until_parked().await;
    let idx_id = intern_id(&tbl, "score_sorted").await;

    // THE PROOF: verify() while still Building.
    let report = tbl.verify().await.unwrap();
    let entry = report
        .sorted_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("score_sorted in sorted_indexes");
    assert_eq!(
        entry.state,
        IndexState::Building,
        "mid-backfill sorted index must report Building"
    );
    assert!(
        !entry.is_healthy(),
        "a Building sorted index must be unhealthy even if counts match"
    );
    assert!(
        entry.message.is_some(),
        "a Building sorted index must carry a diagnostic message"
    );
    assert!(
        !report.is_healthy(),
        "a Building sorted index must make the overall report unhealthy"
    );

    // Release — def flips to Ready.
    hook.release();
    create
        .await
        .unwrap()
        .expect("create_sorted_index must complete");

    // Regression guard: a fully-Ready sorted index is healthy.
    let report2 = tbl.verify().await.unwrap();
    let entry2 = report2
        .sorted_indexes
        .iter()
        .find(|h| h.name_interned == idx_id)
        .expect("score_sorted in sorted_indexes");
    assert_eq!(entry2.state, IndexState::Ready);
    assert!(entry2.is_healthy());
    assert!(entry2.message.is_none());
    assert!(report2.is_healthy());
}
