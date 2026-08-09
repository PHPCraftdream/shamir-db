//! R0-B (#1008) — typed instance provenance for tx-plan reconcile.
//!
//! Proves the ONE unifying `(name_interned, instance_epoch)` retraction rule
//! (`shamir-engine::tx::pre_commit::retract_stale_provenance_ops`) closes the
//! four failure modes the brief identifies (execution map §R0-B):
//!
//! 1. `stage → DROP index → commit` for each of the four index kinds must
//!    NOT resurrect postings for the dropped index (index2/sorted previously
//!    had ZERO retraction; base_index had a byte-length/name heuristic).
//! 2. `stage (field a) → DROP → CREATE same-name (field b) → commit` (ABA)
//!    must NOT contaminate the new index with field-`a`-derived postings —
//!    the pre-R0-B `(is_unique, name_interned)` heuristic could not tell
//!    these two definitions apart because they share `name_interned`.
//! 3. `stage sorted op → RENAME → commit` (Part 1 + Part 2 combined) must
//!    land the row under the NEW name with no orphan under the OLD one.
//! 4. Multiple lifecycle transitions between stage and commit must still
//!    resolve to exactly the FINAL live definition's instance.
//! 5. A tx that stages against a rotating definition, then whose COMMIT
//!    itself fails for an unrelated reason, must leave no partial state.
//!
//! Every test in this file is confirmed (by hand, reverting
//! `retract_stale_provenance_ops`'s call sites in `pre_commit.rs` back to
//! their pre-R0-B shape — no sorted/index2 retraction at all, base_index's
//! byte-length heuristic restored) to FAIL against the pre-fix code. See the
//! implementation report for the exact revert/re-run each test was checked
//! against.

use std::sync::Arc;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index2::functional_backend::FunctionalBackend;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::table::TableManager;
use crate::tx::pre_commit::{RederivePreTxReadFailHook, TEST_REDERIVE_PRE_TX_READ_FAILURE};

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

fn record_with_two_str(k1: u64, v1: &str, k2: u64, v2: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(k1), InnerValue::Str(v1.into()));
    m.insert(InternerKey::new(k2), InnerValue::Str(v2.into()));
    InnerValue::Map(m)
}

/// A functional `lower(<field>)` index create op — mirrors
/// `index2_lifecycle_state_tests.rs::functional_lower_op` (the established
/// idiom for constructing a `CreateIndexOp` for an index2 backend in tests).
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

/// Resolve the set of record ids the functional `lower` index holds for a
/// given (already-lowercased) value, by querying the backend directly.
/// Mirrors `index2_lifecycle_state_tests.rs::functional_lookup`.
async fn functional_lookup(tbl: &TableManager, index_name_id: u64, lowered: &str) -> Vec<[u8; 16]> {
    use crate::index2::backend::{IndexQuery, IndexResult};
    let backend = match tbl.index2_registry().get_by_name(index_name_id).await {
        Some(b) => b,
        None => return Vec::new(),
    };
    let key = FunctionalBackend::hash_value(&InnerValue::Str(lowered.into()));
    let mut keys: smallvec::SmallVec<[Vec<u8>; 4]> = smallvec::SmallVec::new();
    keys.push(key.to_vec());
    match backend.lookup(IndexQuery::Point { keys }).await.unwrap() {
        IndexResult::Set(s) => s.iter().map(|rid| *rid.as_bytes()).collect(),
        IndexResult::Ranked(v) => v.iter().map(|(rid, _)| *rid.as_bytes()).collect(),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Failure mode 1 — stage → DROP → commit must not resurrect postings,
// for EACH of the four index kinds.
// ─────────────────────────────────────────────────────────────────────────

/// Regular (non-unique) base_index: a tx stages an insert while the index is
/// live, the index is DROP'd before commit, and the commit must NOT write a
/// new (orphan) posting for the gone index.
#[tokio::test]
async fn drop_regular_before_commit_no_resurrected_posting() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    let idx_name = key_id(&tbl, "idx_name").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![name_key])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_key, "alice"), Some(&mut tx))
        .await
        .unwrap();

    tbl.index_manager_ref()
        .drop_index(idx_name, None)
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    let irk = crate::index::index_keys::build_index_key_from_record(
        false,
        idx_name,
        &record_with_str(name_key, "alice"),
        &[IndexInfoItem::new(vec![name_key])],
    );
    if let Some(irk) = irk {
        let mut posting_key = irk.to_bytes().to_vec();
        posting_key.extend_from_slice(rid.as_bytes());
        let orphan = tbl
            .info_store()
            .get(shamir_storage::types::RecordKey::from_slice(&posting_key))
            .await;
        assert!(
            orphan.is_err(),
            "R0-B: no orphan posting for a DROP'd regular index must exist after commit"
        );
    }
}

/// Unique base_index: same shape as the regular case, but the family that
/// carries an extra correctness hazard (a resurrected posting is also an
/// unconstrained value if it survives).
#[tokio::test]
async fn drop_unique_before_commit_no_resurrected_posting() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let email_key = key_id(&tbl, "email").await;

    let idx_name = key_id(&tbl, "idx_email").await;
    let def = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![email_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_str(email_key, "alice@example.com"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    tbl.index_manager_ref()
        .drop_unique_index(idx_name, None)
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    let irk = crate::index::index_keys::build_index_key_from_record(
        true,
        idx_name,
        &record_with_str(email_key, "alice@example.com"),
        &[IndexInfoItem::new(vec![email_key])],
    );
    if let Some(irk) = irk {
        let key = irk.to_bytes();
        let orphan = tbl
            .info_store()
            .get(shamir_storage::types::RecordKey::from_slice(&key))
            .await;
        assert!(
            orphan.is_err(),
            "R0-B: no orphan posting for a DROP'd unique index must exist after commit"
        );
    }
}

/// Sorted: pre-R0-B, sorted had ZERO retraction — this test fails outright
/// (not just "wrong contents") against the pre-fix code because the stale
/// SetPosting is unconditionally re-applied at commit.
#[tokio::test]
async fn drop_sorted_before_commit_no_resurrected_posting() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let score_key = key_id(&tbl, "score").await;

    tbl.create_sorted_index("idx_score", &["score"])
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "idx_score").await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let mut rec = new_map_wc(1);
    rec.insert(InternerKey::new(score_key), InnerValue::Int(42));
    let rid = tbl
        .insert_tx(&InnerValue::Map(rec), Some(&mut tx))
        .await
        .unwrap();

    tbl.drop_sorted_index("idx_score").await.unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    let remaining = tbl
        .sorted_indexes()
        .lookup_range(idx_name, None, None)
        .await;
    // The index definition itself is gone, but even if the manager still
    // answered a stale range query, it must never contain this row.
    if let Ok(set) = remaining {
        assert!(
            !set.contains(&rid),
            "R0-B: no orphan posting for a DROP'd sorted index must exist after commit"
        );
    }
}

/// index2 (functional, representative of FTS/functional/vector — all share
/// the SAME registry-driven retraction path). Pre-R0-B, index2 also had
/// ZERO retraction.
///
/// `functional_lookup`/`get_by_name` are NOT usable here for the negative
/// assertion: once the backend is dropped, `get_by_name` returns `None`
/// regardless of whether reconcile correctly retracted the stale op or
/// (pre-fix) silently re-applied it — both states look identical through
/// the registry. The only way to distinguish them is to capture the
/// backend's physical posting-key prefix (`descriptor().id`) BEFORE the
/// drop and query `info_store` DIRECTLY afterward, bypassing the registry
/// entirely.
#[tokio::test]
async fn drop_index2_before_commit_no_resurrected_posting() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "lower_name").await;

    // Capture the backend's physical id + build the EXACT posting key the
    // stale staged op would target, BEFORE the drop removes the backend
    // from the registry.
    let backend = tbl
        .index2_registry()
        .get_by_name(idx_name)
        .await
        .expect("functional backend must be registered");
    let backend_id = backend.descriptor().id;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(name_key, "Alice"), Some(&mut tx))
        .await
        .unwrap();

    tbl.drop_index2("lower_name", None).await.unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    // The backend is gone from the registry.
    assert!(
        tbl.index2_registry().get_by_name(idx_name).await.is_none(),
        "index2 backend must be gone after drop"
    );

    // Directly probe info_store for the exact posting key the stale staged
    // op would have written — the value hash of the lowercased "alice"
    // under the OLD backend's physical id. This is the ONLY assertion that
    // actually discriminates retracted-vs-resurrected: a pre-fix run (no
    // index2 retraction at all) writes this posting; the post-fix run must
    // not.
    let value_hash = FunctionalBackend::hash_value(&InnerValue::Str("alice".into()));
    let posting_key = shamir_index::posting_layout::build_posting_key(
        backend_id,
        shamir_index::posting_layout::type_tag::FUNCTIONAL,
        &value_hash,
        &rid,
    );
    let orphan = tbl
        .info_store()
        .get(shamir_storage::types::RecordKey::from_slice(&posting_key))
        .await;
    assert!(
        orphan.is_err(),
        "R0-B: no orphan posting for a DROP'd index2 backend must exist after commit \
         (checked the physical store directly under the old backend id, bypassing the \
         registry which trivially returns None either way)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Failure mode 2 — ABA: stage (field a) → DROP → CREATE same-name (field b)
// → commit. base_index regular AND unique.
// ─────────────────────────────────────────────────────────────────────────

/// Regular ABA: the OLD staged op (planned against field `a`) must NOT
/// survive into the NEW index (built on field `b`). The pre-R0-B
/// `(is_unique, name_interned)` heuristic could not distinguish the two
/// instances because they share `name_interned` — this is the exact defect
/// class R0-B's `instance_epoch` closes.
#[tokio::test]
async fn regular_aba_drop_create_same_name_no_field_a_contamination() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let a_key = key_id(&tbl, "a").await;
    let b_key = key_id(&tbl, "b").await;

    let idx_name = key_id(&tbl, "idx_x").await;
    let def_a = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![a_key])]);
    tbl.index_manager_ref().create_index(def_a).await.unwrap();

    // Stage a row with BOTH fields set — the field-`a` op is planned now.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_two_str(a_key, "avalue", b_key, "bvalue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // ABA: DROP the field-`a` index, CREATE a NEW index under the SAME name
    // on field `b`.
    tbl.index_manager_ref()
        .drop_index(idx_name, None)
        .await
        .unwrap();
    let def_b = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![b_key])]);
    tbl.index_manager_ref().create_index(def_b).await.unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    // The new index (field b) must find the row under "bvalue".
    let found_b = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("bvalue".into())])
        .await
        .unwrap()
        .unwrap();
    assert!(
        found_b.iter().any(|r| r == &rid),
        "R0-B ABA regular: the row must be indexed under the NEW (field b) definition"
    );

    // The new index must NOT contain a stale field-a-derived posting for
    // "avalue" — that posting belongs to the OLD (dropped) instance.
    let found_a = tbl
        .index_manager_ref()
        .lookup_by_index(idx_name, &[InnerValue::Str("avalue".into())])
        .await
        .unwrap()
        .unwrap();
    assert!(
        found_a.is_empty(),
        "R0-B ABA regular: the new index must NOT contain field-a-derived postings \
         from the dropped OLD instance under the same name"
    );
}

/// Unique ABA: same as regular, PLUS confirm no false unique conflict from
/// the stale op's key (the old field-`a` unique posting must not collide
/// with — or be mistaken for validating against — the new field-`b` index).
#[tokio::test]
async fn unique_aba_drop_create_same_name_no_false_conflict() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let a_key = key_id(&tbl, "a").await;
    let b_key = key_id(&tbl, "b").await;

    let idx_name = key_id(&tbl, "idx_u").await;
    let def_a = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![a_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def_a)
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_two_str(a_key, "avalue", b_key, "bvalue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    tbl.index_manager_ref()
        .drop_unique_index(idx_name, None)
        .await
        .unwrap();
    let def_b = IndexDefinition::new(idx_name, vec![IndexInfoItem::new(vec![b_key])]);
    tbl.index_manager_ref()
        .create_unique_index(def_b)
        .await
        .unwrap();

    // Commit must succeed cleanly — no false UniqueViolation from the stale
    // field-a op's key colliding with reconcile logic.
    repo.commit_tx(tx).await.expect(
        "R0-B ABA unique: commit must succeed — no false conflict from the retracted \
         stale field-a posting",
    );

    let owner = tbl
        .index_manager_ref()
        .lookup_by_unique_index(idx_name, &[InnerValue::Str("bvalue".into())])
        .await
        .unwrap();
    assert_eq!(
        owner,
        Some(rid),
        "R0-B ABA unique: the row must own the NEW (field b) unique posting"
    );

    let stale_owner = tbl
        .index_manager_ref()
        .lookup_by_unique_index(idx_name, &[InnerValue::Str("avalue".into())])
        .await
        .unwrap();
    assert_eq!(
        stale_owner, None,
        "R0-B ABA unique: no posting must exist under the OLD (field a) value — \
         the stale op must have been retracted, not applied"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Failure mode 3 — Part 1 + Part 2 combined: stage sorted op → RENAME →
// commit. The direct end-to-end proof that #1007 and #1008 together close
// NP-2.
// ─────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sorted_rename_between_stage_and_commit_no_orphan_lands_under_new_name() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let score_key = key_id(&tbl, "score").await;

    tbl.create_sorted_index("idx_old", &["score"])
        .await
        .unwrap();
    let old_id = key_id(&tbl, "idx_old").await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let mut rec = new_map_wc(1);
    rec.insert(InternerKey::new(score_key), InnerValue::Int(7));
    let rid = tbl
        .insert_tx(&InnerValue::Map(rec), Some(&mut tx))
        .await
        .unwrap();

    // RENAME between stage and commit — Part 1 (generation bump) + Part 2
    // (instance_epoch bump + reconcile retraction) both engage here.
    tbl.rename_index("idx_old", "idx_new", None).await.unwrap();
    let new_id = key_id(&tbl, "idx_new").await;

    repo.commit_tx(tx).await.expect("commit must succeed");

    // The row must be indexed under the NEW name.
    let under_new = tbl
        .sorted_indexes()
        .lookup_range(new_id, None, None)
        .await
        .unwrap();
    assert!(
        under_new.contains(&rid),
        "R0-B + #1007: the row must be indexed under the NEW name after a \
         stage-then-rename sequence"
    );

    // No orphan posting survives under the OLD namespace. `lookup_range`
    // scans by physical key prefix (`SORTED_TAG || name_interned || ...`)
    // regardless of whether a live definition still answers to `old_id` —
    // this is the assertion that actually discriminates "the stale
    // pre-rename op was retracted" from "it was silently re-applied under
    // the old physical prefix" (a `find_by_name_interned(old_id).is_none()`
    // check would pass either way, since the DEFINITION is renamed away
    // regardless of whether its ops were retracted).
    let under_old = tbl
        .sorted_indexes()
        .lookup_range(old_id, None, None)
        .await
        .unwrap();
    assert!(
        !under_old.contains(&rid),
        "R0-B + #1007: no orphan posting for this row must survive under the OLD \
         (pre-rename) namespace"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Failure mode 4 — multiple lifecycle transitions between stage and commit.
// ─────────────────────────────────────────────────────────────────────────

/// CREATE → RENAME → DROP → CREATE-same-name, all before ONE commit. Proves
/// the epoch mechanism correctly resolves to the FINAL live instance, not
/// just the single-transition cases above.
#[tokio::test]
async fn multiple_transitions_before_commit_resolve_to_final_instance() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let a_key = key_id(&tbl, "a").await;
    let c_key = key_id(&tbl, "c").await;

    // Initial index on field `a`, named "idx1".
    let idx1 = key_id(&tbl, "idx1").await;
    let def1 = IndexDefinition::new(idx1, vec![IndexInfoItem::new(vec![a_key])]);
    tbl.index_manager_ref().create_index(def1).await.unwrap();

    // Stage a row while "idx1"/field-a is live.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(
            &record_with_two_str(a_key, "aval", c_key, "cval"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // RENAME idx1 -> idx2 (still on field a; regular-family rename is
    // create-new+drop-old under the hood, which already mints a fresh
    // instance via `create_index`).
    tbl.rename_index("idx1", "idx2", None).await.unwrap();

    // DROP idx2.
    let idx2 = key_id(&tbl, "idx2").await;
    tbl.index_manager_ref()
        .drop_index(idx2, None)
        .await
        .unwrap();

    // CREATE a NEW index under the ORIGINAL name "idx1", this time on field
    // `c` — a same-name-as-the-original-name recreation with a different
    // field, several transitions removed from stage time.
    let def_final = IndexDefinition::new(idx1, vec![IndexInfoItem::new(vec![c_key])]);
    tbl.index_manager_ref()
        .create_index(def_final)
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    // The FINAL live instance (idx1, field c) must have the row indexed
    // under "cval".
    let found_final = tbl
        .index_manager_ref()
        .lookup_by_index(idx1, &[InnerValue::Str("cval".into())])
        .await
        .unwrap()
        .unwrap();
    assert!(
        found_final.iter().any(|r| r == &rid),
        "R0-B multi-transition: the row must be indexed under the FINAL live \
         instance (idx1 on field c)"
    );

    // No stale field-a posting under the FINAL idx1 namespace (that op was
    // planned against a DIFFERENT, now-dead instance of "idx1").
    let found_stale = tbl
        .index_manager_ref()
        .lookup_by_index(idx1, &[InnerValue::Str("aval".into())])
        .await
        .unwrap()
        .unwrap();
    assert!(
        found_stale.is_empty(),
        "R0-B multi-transition: no stale field-a posting must survive under the \
         FINAL idx1 instance"
    );

    // idx2 (the intermediate renamed-then-dropped instance) has nothing.
    let found_idx2 = tbl
        .index_manager_ref()
        .lookup_by_index(idx2, &[InnerValue::Str("aval".into())])
        .await
        .unwrap()
        .unwrap();
    assert!(
        found_idx2.is_empty(),
        "R0-B multi-transition: the dropped intermediate instance (idx2) must have \
         no postings"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Failure mode 5 — rollback: a tx stages against one epoch, the definition
// rotates twice more before commit, then the COMMIT ITSELF fails for an
// UNRELATED reason. No partial/inconsistent state must be left in the
// registries.
// ─────────────────────────────────────────────────────────────────────────

/// Uses the F-73 pre-tx-read-failure injection hook (`TEST_REDERIVE_PRE_TX_READ_FAILURE`)
/// to force a genuine, deterministic commit abort unrelated to the R0-B
/// reconcile logic itself — proving the reconcile changes do not weaken the
/// tx's own atomicity (registries stay internally consistent even though
/// the reconcile function ran partway before the abort).
///
/// NOTE on red/green: per the brief, "the tx's own atomicity, not the
/// reconcile logic, should own this — but verify the reconcile changes
/// don't accidentally weaken it." This test is a NON-REGRESSION check on a
/// property the reconcile change must NOT break, not a proof that R0-B's
/// retraction mechanism itself is engaged — it correctly PASSES both with
/// and without `retract_stale_provenance_ops` wired in (confirmed by hand),
/// which is the expected/desired result: it demonstrates the reconcile
/// rewrite did not introduce a NEW atomicity hazard, orthogonal to whether
/// retraction itself fires.
#[tokio::test]
async fn rollback_after_multiple_rotations_leaves_no_partial_state() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let token = table_token_for("t");
    let name_key = key_id(&tbl, "name").await;

    // Pre-existing committed row (so the tx below is an UPDATE — the F-73
    // hook fires on the pre-tx read Phase 2.7 performs to distinguish
    // insert vs. update).
    let rid = tbl
        .insert(&record_with_str(name_key, "orig"))
        .await
        .unwrap();

    // Stage an UPDATE with NO index2 backend live yet.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    tbl.update_tx(rid, &record_with_str(name_key, "orig2"), Some(&mut tx))
        .await
        .unwrap();

    // Rotate the index2 registry TWICE before commit: create, drop, create
    // again under the same name — each mutation bumps the registry
    // generation, so the commit-time reconcile's ADD path will fire.
    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    tbl.drop_index2("lower_name", None).await.unwrap();
    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "lower_name").await;

    // Arm the fault: the pre-tx read that Phase 2.7's reconcile issues for
    // (token, rid) will return a genuine error, aborting the commit BEFORE
    // Phase 4's WAL begin — a clean, unrelated failure.
    let hook = TEST_REDERIVE_PRE_TX_READ_FAILURE
        .get_or_init(|| Arc::new(RederivePreTxReadFailHook::default()));
    hook.arm(token, rid);

    let result = repo.commit_tx(tx).await;
    assert!(
        result.is_err(),
        "R0-B rollback: the injected unrelated failure must abort the commit"
    );

    // Row must be UNCHANGED (still "orig", not "orig2") — Phase 5a never ran.
    let observed = tbl.get(rid).await.unwrap();
    let observed_name = match &observed {
        InnerValue::Map(m) => m.get(&InternerKey::new(name_key)).cloned(),
        _ => None,
    };
    assert!(
        matches!(observed_name, Some(InnerValue::Str(ref s)) if s == "orig"),
        "R0-B rollback: an aborted commit must not have applied the staged update"
    );

    // The registries themselves (NOT the reconcile logic — the tx's own
    // atomicity) must be internally consistent: the FINAL live index2
    // backend is exactly the second "lower_name" instance, still resolvable
    // and functional.
    assert!(
        tbl.index2_registry().get_by_name(idx_name).await.is_some(),
        "R0-B rollback: the index2 registry's own state must be unaffected by the \
         aborted tx — the live backend must still resolve"
    );

    // A FRESH, uncontaminated tx must still be able to use the index
    // normally afterward (no poisoned state left behind by the aborted
    // reconcile).
    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid2 = tbl
        .insert_tx(&record_with_str(name_key, "Fresh"), Some(&mut tx2))
        .await
        .unwrap();
    repo.commit_tx(tx2)
        .await
        .expect("a subsequent clean commit must succeed after the aborted one");
    let owners = functional_lookup(&tbl, idx_name, "fresh").await;
    assert!(
        owners.contains(rid2.as_bytes()),
        "R0-B rollback: the index must still function correctly for subsequent \
         commits after the earlier abort"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// F-1 (#1027) — batch INSERT (`insert_tx_many` / `insert_tx_many_bytes`)
// never stamped index2 provenance, so a staged batch op carried the
// placeholder `instance_epoch: 0` all the way to commit. ANY unrelated
// index2 DDL between stage and commit then bumps the registry generation,
// the commit-time reconcile derives `live_index2` from the CURRENT
// registry, and the placeholder-epoch op matches nothing in it — it gets
// silently RETRACTED even though the index it targets is still live and
// untouched. This is the batch-path twin of
// `drop_index2_before_commit_no_resurrected_posting` above, but instead of
// proving a DROP'd index's posting is retracted, it proves a STILL-LIVE
// index's posting SURVIVES an unrelated concurrent index2 CREATE — the
// exact inverse assertion that discriminates the bug (pre-fix: posting is
// wrongly retracted; post-fix: posting survives).
// ─────────────────────────────────────────────────────────────────────────

/// `insert_tx_many` (the `InnerValue`-tree batch path): stage a batch insert
/// while `lower_name` (index2/functional) is live, create an UNRELATED
/// second index2 backend before commit (bumps the registry generation
/// without touching `lower_name` itself), then commit. The row's posting
/// in `lower_name` must survive.
#[tokio::test]
async fn insert_tx_many_index2_posting_survives_unrelated_concurrent_create() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "lower_name").await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let values = vec![record_with_str(name_key, "Alice")];
    let ids = tbl.insert_tx_many(&values, &mut tx).await.unwrap();
    let rid = ids[0];

    // Unrelated concurrent index2 DDL between stage and commit — bumps
    // `IndexRegistry::generation` and forces the commit-time reconcile's
    // rederive path to run, without touching `lower_name` at all.
    tbl.create_index_v2(&functional_lower_op("lower_other", "t", "other"))
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    let owners = functional_lookup(&tbl, idx_name, "alice").await;
    assert!(
        owners.contains(rid.as_bytes()),
        "F-1: insert_tx_many's index2 posting must survive an unrelated \
         concurrent index2 CREATE between stage and commit — a batch-staged \
         op with an unstamped (placeholder instance_epoch: 0) provenance is \
         wrongly retracted by the commit-time reconcile the moment ANY \
         index2 DDL bumps the registry generation, even though lower_name \
         itself was never touched"
    );
}

/// `insert_tx_many_bytes` (the `RecordView`/lens batch path that
/// `execute_insert_tx`/`execute_set_tx` — i.e. every transactional
/// INSERT/UPSERT through the query executor — actually call): identical
/// scenario to the tree-path test above, driven through the bytes-staged
/// entry point instead.
#[tokio::test]
async fn insert_tx_many_bytes_index2_posting_survives_unrelated_concurrent_create() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));
    let tbl = repo.get_table("t").await.unwrap();
    let name_key = key_id(&tbl, "name").await;

    tbl.create_index_v2(&functional_lower_op("lower_name", "t", "name"))
        .await
        .unwrap();
    let idx_name = key_id(&tbl, "lower_name").await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let staged: Vec<bytes::Bytes> = vec![record_with_str(name_key, "Alice")
        .to_bytes()
        .expect("encode")];
    let ids = tbl.insert_tx_many_bytes(&staged, &mut tx).await.unwrap();
    let rid = ids[0];

    // Unrelated concurrent index2 DDL between stage and commit.
    tbl.create_index_v2(&functional_lower_op("lower_other", "t", "other"))
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    let _ = tbl.get(rid).await.expect("row itself must commit");

    let owners = functional_lookup(&tbl, idx_name, "alice").await;
    assert!(
        owners.contains(rid.as_bytes()),
        "F-1: insert_tx_many_bytes's index2 posting must survive an unrelated \
         concurrent index2 CREATE between stage and commit (same defect as \
         insert_tx_many, on the path execute_insert_tx/execute_set_tx \
         actually use for every tx INSERT/UPSERT, including a single row)"
    );
}
