//! F-50 (#869, spike) — index2 ops-plan re-derivation: red→green proof.
//!
//! Closes #538 Part B's "guaranteed miss" for ONE concrete case (single-row
//! INSERT, functional `lower(<field>)` index). The interleaving is
//! deterministic by construction — three sequential steps, no pause-seam:
//!   1. STAGE a tx's insert (its stage-time `all_backends()` snapshot is
//!      taken BEFORE the new functional backend is registered, so its
//!      `index_write_set` permanently has zero ops for it).
//!   2. COMPLETE a `create_index_v2` (backfill + register).
//!   3. COMMIT the tx.
//!
//! Pre-fix (current code): the tx commits a row that is physically present
//! but permanently absent from the new index — Phase 5c has no posting to
//! write for a backend that never made it into the stage-time plan.
//!
//! Post-fix (this spike): `pre_commit_prelock` Phase 2.7 re-derives index2
//! ops against the LIVE backend registry (gated by `IndexRegistry`'s
//! generation counter, captured at stage time) and appends them to
//! `tx.index_write_set` BEFORE the WAL entry is built — so the row lands in
//! the new index.

use std::sync::Arc;

use shamir_query_builder::write;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::functional_backend::FunctionalBackend;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// A functional `lower(<field>)` index create op (mirrors
/// `index2_create_barrier_tests`'s helper).
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

/// Resolve the record ids the functional `lower` index holds for a given
/// (already-lowercased) value, by querying the backend directly.
async fn functional_lookup(tbl: &TableManager, index_name_id: u64, lowered: &str) -> Vec<[u8; 16]> {
    let backend = tbl
        .index2_registry()
        .get_by_name(index_name_id)
        .await
        .expect("functional backend must be registered");
    let key = FunctionalBackend::hash_value(&InnerValue::Str(lowered.into()));
    let mut keys: smallvec::SmallVec<[Vec<u8>; 4]> = smallvec::SmallVec::new();
    keys.push(key.to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(s) => s.iter().map(|rid| *rid.as_bytes()).collect(),
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
    }
}

/// THE F-50 proof. A tx that STAGES its insert BEFORE the functional index is
/// registered, then COMMITS after `create_index_v2` has fully registered it,
/// MUST have its row indexed by the new backend.
///
/// Determinism: no pause hook is needed — the three steps (stage, create,
/// commit) are strictly sequential. The stage-time `all_backends()` snapshot
/// trivially cannot include a backend registered only at step 2.
///
/// Pre-fix: this test FAILS — the row is physically present (Phase 5a is
/// unconditional) but absent from the new index (Phase 5c has no op for a
/// backend the stale stage-time plan never saw). That is #538 Part B's
/// "guaranteed miss (not a rare race)".
///
/// Post-fix: Phase 2.7 re-derives the functional backend's posting against
/// the live registry and appends it before WAL serialization, so Phase 5c
/// writes it → the row is indexed.
#[tokio::test]
async fn tx_staging_before_index_register_commits_with_posting_after_fix() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    let name_field = key_id(&tbl, "name").await;

    // One pre-existing row so the create's backfill has something to stream
    // (and so the new index is non-trivially populated at register time).
    let _pre = tbl
        .insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();

    // === Step 1: STAGE the tx's insert BEFORE the functional index exists. ===
    // At this instant there is no functional backend in ANY form, so the
    // stage-time `all_backends()` snapshot is trivially complete (empty) —
    // the tx's `index_write_set` permanently carries zero ops for the
    // not-yet-created index. The generation is captured here.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("people")
        .rows([mpack!({ "name": "Bob" })])
        .build();
    let result = tbl
        .execute_insert_tx(
            &op,
            &mut tx,
            true,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    let bob = result.records[0].id.expect("insert must assign an id");

    // === Step 2: COMPLETE create_index_v2 (backfill + register). ===
    // Bob's row is still only STAGED (the tx has not committed), so the
    // backfill stream cannot see it — only Alice gets backfilled. The
    // register bumps the index2 generation past the value captured at step 1.
    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .expect("create_index_v2 must succeed");

    // === Step 3: COMMIT the tx. ===
    // Pre-fix: Phase 5c has no functional posting to write → guaranteed miss.
    // Post-fix: Phase 2.7 sees the generation advanced, re-derives Bob's
    // functional posting against the now-live backend, appends it before WAL
    // serialization → Phase 5c writes it.
    repo.commit_tx(tx)
        .await
        .expect("tx commit must succeed (data write is never at risk)");

    // The row IS physically present (Phase 5a is unconditional — never at risk).
    let _ = tbl.get(bob).await.expect("row must be physically present");

    // THE assertion: Bob MUST be queryable via the new functional index.
    let owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "bob").await;
    assert!(
        owners.contains(bob.as_bytes()),
        "F-50: the row staged before the index was registered MUST be indexed \
         after commit — a guaranteed miss here means #538 Part B is still open"
    );

    // And the pre-existing Alice (backfilled at step 2) is still indexed.
    let alice_owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "alice").await;
    assert_eq!(
        alice_owners.len(),
        1,
        "pre-existing row must remain backfilled into the new index"
    );
}

/// Quiescent control: with NO concurrent index creation, the generation gate
/// fires zero re-derivation work — a tx that stages and commits with an
/// unchanged index2 generation must behave exactly as before (its row is
/// indexed via the stage-time plan, no re-derivation). Guards against the
/// re-derivation path double-applying or regressing the common case.
#[tokio::test]
async fn quiescent_tx_unchanged_generation_indexes_via_stage_plan() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    // Create the functional index FIRST — it is live before any tx stages,
    // so the stage-time snapshot includes it and the generation never changes.
    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("people")
        .rows([mpack!({ "name": "Carol" })])
        .build();
    let result = tbl
        .execute_insert_tx(
            &op,
            &mut tx,
            true,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    let carol = result.records[0].id.expect("insert must assign an id");

    repo.commit_tx(tx).await.expect("commit must succeed");

    let owners = functional_lookup(&tbl, key_id(&tbl, "lower_name").await, "carol").await;
    assert!(
        owners.contains(carol.as_bytes()),
        "quiescent case: the row must be indexed via the normal stage-time plan"
    );
    assert_eq!(
        owners.len(),
        1,
        "no double-application: exactly one posting for the staged row"
    );
}
