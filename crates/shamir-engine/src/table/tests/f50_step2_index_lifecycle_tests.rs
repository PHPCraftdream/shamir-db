//! F-50 Step 2 (#870) — index2 ops-plan re-derivation: extensions.
//!
//! Mirrors `f50_index_lifecycle_spike_tests.rs`'s red→green proof pattern for
//! the three Step 2 extensions:
//!   - **Part B**: vector backend `staged_vectors` re-derivation.
//!   - **Part C**: FTS `BumpFtsStats` double-count guard (regression test).
//!   - **Part D**: sorted-index residual (register-before-backfill closed by
//!     the generation-gate re-derivation).
//!
//! The interleaving is deterministic by construction — three sequential steps,
//! no pause-seam: stage, complete the DDL (register), commit.

use std::sync::Arc;

use shamir_query_builder::write;
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::mpack;
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::backend::{IndexQuery, IndexResult};
use crate::index2::vector::SearchOpts;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;

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

// ─────────────────────────────────────────────────────────────────────────────
// Part B — vector backend staged_vectors re-derivation
// ─────────────────────────────────────────────────────────────────────────────

fn vector_index_op(dim: u32) -> CreateIndexOp {
    CreateIndexOp {
        create_index: "vec_idx".into(),
        table: "vecs".into(),
        fields: vec![vec!["embedding".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(dim),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

/// Build a `{embedding: [f32...]}` record (via InnerValue::List of F64 — the
/// production vector record shape).
fn vec_record(emb_key: u64, vec: &[f32]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(emb_key),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    InnerValue::Map(m)
}

/// Part B proof: a tx that STAGES its insert BEFORE a vector index is
/// registered, then COMMITS after `create_index_v2` has fully registered it,
/// MUST have its row's embedding present in the HNSW graph (queryable via the
/// backend's `lookup(IndexQuery::Vector{..})`).
///
/// Determinism: three sequential steps, no pause-seam (mirrors
/// `tx_staging_before_index_register_commits_with_posting_after_fix` for the
/// functional backend).
///
/// Pre-fix (Step 2 vector branch absent): `VectorBackend::plan_insert_tx` is a
/// no-op for a tx, so the stage-time plan contributed nothing; Phase 5c has no
/// posting AND — crucially — Phase 5d has no `staged_vector` to promote
/// because the stage-time `stage_vectors` loop saw no vector backend. The
/// row's embedding is permanently absent from the HNSW graph.
///
/// Post-fix (Step 2 Part B): Phase 2.7 sees the generation advanced, calls the
/// new vector backend's `staged_vector(rid, new_rec)` (re-deriving the
/// embedding extraction) and stages it via `tx.stage_vector`. Phase 5d then
/// promotes it into the live graph → the row's embedding is queryable.
#[tokio::test]
async fn vector_staging_before_index_register_present_after_fix() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();

    let emb_key = key_id(&tbl, "embedding").await;

    // One pre-existing row so the create's backfill has something to stream.
    let _pre = tbl
        .insert(&vec_record(emb_key, &[1.0, 0.0, 0.0]))
        .await
        .unwrap();

    // === Step 1: STAGE the tx's insert BEFORE the vector index exists. ===
    // At this instant there is no vector backend in ANY form, so the stage-time
    // `all_backends()` snapshot is trivially complete (empty). The tx's
    // `staged_vectors` permanently carries nothing for the not-yet-created
    // vector backend. The generation is captured here.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("vecs")
        .rows([mpack!({ "embedding": [0.5, 0.5, 0.0] })])
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
    let staged_rid = result.records[0].id.expect("insert must assign an id");

    // === Step 2: COMPLETE create_index_v2 (backfill + register). ===
    // The staged row is still only STAGED, so the backfill stream cannot see
    // it — only the pre-existing row gets backfilled. The register bumps the
    // index2 generation past the value captured at step 1.
    tbl.create_index_v2(&vector_index_op(3))
        .await
        .expect("create_index_v2 must succeed");

    // === Step 3: COMMIT the tx. ===
    // Pre-fix: Phase 5d has no staged vector to promote → guaranteed miss.
    // Post-fix: Phase 2.7 sees the generation advanced, re-derives the staged
    // vector for the new backend via `staged_vector(rid, new_rec)` → Phase 5d
    // promotes it into the live graph.
    repo.commit_tx(tx).await.expect("tx commit must succeed");

    // The row IS physically present.
    let _ = tbl
        .get(staged_rid)
        .await
        .expect("row must be physically present");

    // THE assertion: the staged row's embedding MUST be queryable via the new
    // vector backend (present in the HNSW graph post-commit).
    let backend = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "vec_idx").await)
        .await
        .expect("vector backend must be registered");

    // Search for vectors near [0.5, 0.5, 0.0] — the staged row should be a
    // top-1 hit (cosine similarity = 1.0 to itself).
    let result = backend
        .lookup(IndexQuery::Vector {
            vec: vec![0.5, 0.5, 0.0],
            k: 10,
            opts: SearchOpts::default(),
        })
        .await
        .expect("vector lookup must succeed");

    let rids: Vec<[u8; 16]> = match result {
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
        _ => panic!("expected Ranked result from vector lookup"),
    };
    assert!(
        rids.iter().any(|r| r == staged_rid.as_bytes()),
        "F-50 Step 2 Part B: the row staged before the vector index was \
         registered MUST have its embedding in the HNSW graph after commit — \
         a guaranteed miss here means the staged_vectors re-derivation is \
         missing or broken. Got rids: {:?}, expected to contain {:?}",
        rids,
        staged_rid
    );

    // And the pre-existing row (backfilled at step 2) is also present.
    assert!(
        rids.len() >= 2,
        "pre-existing row must also be backfilled into the vector index; \
         got {} results, expected ≥ 2",
        rids.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Part C — FTS BumpFtsStats double-count guard (regression test)
// ─────────────────────────────────────────────────────────────────────────────

fn fts_index_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

/// Part C: a tx that plans an FTS insert at STAGE time against an FTS backend
/// that is ALREADY registered, then COMMITS, must produce EXACTLY ONE
/// `BumpFtsStats { sign: +1 }` for that backend — not two.
///
/// This is the quiescent case for FTS (analogous to the spike's
/// `quiescent_tx_unchanged_generation_indexes_via_stage_plan` for the
/// functional backend): the generation never changes, so Phase 2.7's
/// `backends_newer_than(stage_gen)` returns an empty diff (the FTS backend's
/// insertion-gen ≤ stage_gen), and no re-derivation fires.
///
/// The mechanism should already be correct (the generation filter's
/// `insertion_gen > stage_gen` naturally excludes already-planned backends);
/// this test is a regression guard against a future change that breaks the
/// filter and double-applies `BumpFtsStats` (which would corrupt
/// `doc_count`).
#[tokio::test]
async fn fts_quiescent_tx_exactly_one_bumpftsstats_no_double_count() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();

    let title_key = key_id(&tbl, "title").await;

    // Create the FTS index FIRST — it is live before any tx stages, so the
    // stage-time snapshot includes it and the generation never changes.
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();

    // Snapshot the backend's doc_count BEFORE the tx (backfill found nothing —
    // the table is empty — so this is 0).
    let backend = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("fts backend must be registered");
    // Downcast to FtsRankedBackend to read `stats.doc_count`.
    let fts = backend
        .as_any()
        .downcast_ref::<crate::index2::fts_ranked_backend::FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let doc_count_pre = fts.doc_count();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("docs")
        .rows([mpack!({ "title": "hello world foo bar" })])
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

    repo.commit_tx(tx).await.expect("commit must succeed");

    // THE assertion: exactly ONE BumpFtsStats was applied — doc_count advanced
    // by exactly 1, not 2. A double-count (two BumpFtsStats) would leave
    // doc_count = 2 for a single inserted doc.
    let doc_count_post = fts.doc_count();
    assert_eq!(
        doc_count_post,
        doc_count_pre + 1,
        "F-50 Step 2 Part C: exactly ONE BumpFtsStats must fire for a tx that \
         planned the FTS insert at stage time (quiescent case — generation \
         unchanged, so Phase 2.7's backends_newer_than diff is empty and no \
         re-derivation fires). A doc_count of {} (expected {}) means the \
         generation filter is double-counting.",
        doc_count_post,
        doc_count_pre + 1
    );

    // Sanity: the row's title IS indexed (the quiescent control also confirms
    // the stage-time plan wrote its postings).
    let _ = record_with_str(title_key, "hello world foo bar");
    let tokens = backend.tokenize_query("hello");
    let result = backend
        .lookup(IndexQuery::Fts {
            tokens,
            mode: crate::index2::backend::FtsMode::OrAny,
        })
        .await
        .expect("fts lookup must succeed");
    let n_results = match result {
        IndexResult::Set(s) => s.len(),
        IndexResult::Ranked(v) => v.len(),
    };
    assert!(
        n_results > 0,
        "the inserted doc's 'hello' token must be indexed (quiescent stage plan)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Part D — sorted-index residual (generation-gate re-derivation)
// ─────────────────────────────────────────────────────────────────────────────

/// Part D proof: a tx that STAGES its insert BEFORE a sorted index is
/// registered (via `create_sorted_index`), then COMMITS after, MUST have its
/// row indexed by the new sorted index.
///
/// Determinism: three sequential steps, no pause-seam.
///
/// Pre-fix (no generation counter on `SortedIndexManager` + no commit-time
/// sorted re-derivation): the tx's `index_write_set` permanently carries zero
/// sorted ops for the new index (the stage-time `plan_legacy_insert_ops`'s
/// `sorted_indexes.plan_record_created` ran against an empty def snapshot).
/// Phase 5c has no sorted posting to write → the row is permanently absent
/// from the new sorted index — the SAME guaranteed-miss class as index2
/// Part B.
///
/// Post-fix (Step 2 Part D): `SortedIndexManager::generation` is captured at
/// stage (`note_sorted_stage_gen`) alongside the index2 generation. At commit
/// Phase 2.7, the sorted generation-gate fires: if advanced, the commit
/// pipeline re-derives sorted posting ops via `plan_record_created` /
/// `plan_record_updated` / `plan_record_deleted` against ALL current defs
/// (idempotent) and appends them to `tx.index_write_set` before WAL
/// serialization → Phase 5c writes them.
#[tokio::test]
async fn sorted_staging_before_register_present_after_fix() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("nums"));
    let tbl = repo.get_table("nums").await.unwrap();

    let val_key = key_id(&tbl, "val").await;

    // One pre-existing row so the create_sorted_index's backfill has something
    // to stream.
    let _pre = tbl.insert(&record_with_str(val_key, "10")).await.unwrap();

    // === Step 1: STAGE the tx's insert BEFORE the sorted index exists. ===
    // At this instant there are no sorted defs, so the stage-time
    // `plan_legacy_insert_ops`'s `sorted_indexes.plan_record_created` returns
    // zero ops. The sorted generation is captured here.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("nums")
        .rows([mpack!({ "val": "42" })])
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
    let staged_rid = result.records[0].id.expect("insert must assign an id");

    // === Step 2: COMPLETE create_sorted_index (register + backfill). ===
    // The staged row is still only STAGED, so the backfill stream cannot see
    // it — only the pre-existing row gets backfilled. The register bumps the
    // sorted generation past the value captured at step 1.
    tbl.create_sorted_index("val_sorted", &["val"])
        .await
        .expect("create_sorted_index must succeed");

    // === Step 3: COMMIT the tx. ===
    // Pre-fix: Phase 5c has no sorted posting to write → guaranteed miss.
    // Post-fix: Phase 2.7's sorted gate sees the generation advanced,
    // re-derives the sorted posting for the new def via `plan_record_created`
    // → Phase 5c writes it.
    repo.commit_tx(tx).await.expect("tx commit must succeed");

    // The row IS physically present.
    let _ = tbl
        .get(staged_rid)
        .await
        .expect("row must be physically present");

    // THE assertion: the staged row MUST be queryable via the new sorted index.
    // A sorted index stores `(encoded_value, record_id)` entries; a range scan
    // over the full keyspace (Unbounded..Unbounded) returns every indexed rid.
    let name_interned = key_id(&tbl, "val_sorted").await;
    let defs = tbl.sorted_indexes().iter_indexes();
    let def = defs
        .iter()
        .find(|d| d.name_interned == name_interned)
        .expect("sorted def must be registered");

    // Build the entry prefix for this def and scan it directly from the
    // info_store (the sorted index's physical home).
    let prefix = tbl.info_store().scan_prefix_stream(
        bytes::Bytes::from(sorted_entry_prefix(def.name_interned)),
        1000,
    );
    futures::pin_mut!(prefix);
    let mut found_rids: Vec<[u8; 16]> = Vec::new();
    while let Some(batch) = futures::StreamExt::next(&mut prefix).await {
        for (key_bytes, _) in batch.unwrap() {
            // Sorted entry keys are `<name_interned_be><encoded_value><rid>`;
            // the trailing 16 bytes are the rid.
            if key_bytes.len() >= 16 {
                let mut rid = [0u8; 16];
                rid.copy_from_slice(&key_bytes[key_bytes.len() - 16..]);
                found_rids.push(rid);
            }
        }
    }

    assert!(
        found_rids.iter().any(|r| r == staged_rid.as_bytes()),
        "F-50 Step 2 Part D: the row staged before the sorted index was \
         registered MUST be indexed after commit — a guaranteed miss here \
         means the sorted-index re-derivation is missing or broken. \
         Found rids: {:?}, expected to contain {:?}",
        found_rids,
        staged_rid
    );
}

/// Build the entry-key prefix for a sorted index def (the high bytes every
/// entry under that def shares). Used by the Part D test to scan the def's
/// physical entries directly from the info_store.
///
/// Mirrors `SortedIndexManager::entry_prefix` (private): `SORTED_TAG (0x80) +
/// name_interned_be`. Kept inline here so the test does not depend on a
/// private accessor. `SORTED_TAG` is the `pub(crate) const 0x80` from
/// `sorted_index_definition.rs`.
fn sorted_entry_prefix(name_interned: u64) -> Vec<u8> {
    let mut buf = Vec::with_capacity(9);
    buf.push(0x80); // SORTED_TAG
    buf.extend_from_slice(&name_interned.to_be_bytes());
    buf
}
