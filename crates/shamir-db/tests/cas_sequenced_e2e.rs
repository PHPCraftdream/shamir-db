//! End-to-end proof of **sequenced writes / optimistic CAS by prev-hash**
//! (Phase 3a, #160) — the "BEFORE-half" of the record lifecycle.
//!
//! A write to a key is accepted only when the hash the writer presents
//! (`_prev_hash`) equals the canonical hash of the *current* stored version.
//! This is optimistic concurrency / a "blockchain ordering" of writes: a
//! stale writer who based its update on version v1 is rejected once the row
//! has already advanced to v2.
//!
//! # CAS validator mechanism
//!
//! No WASM toolchain is required. We first materialise a real validator
//! catalogue entry via `create_validator_from_wasm` (minimal `(module)`), then
//! swap the live registry artifact for a native [`RecordValidator`]
//! (`CasValidator`) through the additive
//! `ValidatorRegistry::replace_artifact`. Bindings created by `bind_validator`
//! are preserved across the swap. The native validator accesses `record` /
//! `old_record` via `&dyn RecordFields` by name — no Params or ShamirFunction
//! overhead.
//!
//! The validator and the test compute the expected hash through the **same**
//! `shamir_funclib::canonical::canonical_hash` over a string-keyed
//! `QueryValue`, so the two sides agree bit-for-bit. Records use string fields
//! only, so the msgpack read-back round-trips to the identical `QueryValue` the
//! validator hashes.

use std::sync::Arc;

use async_trait::async_trait;
use shamir_types::mpack;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_engine::validator::{RecordFields, RecordValidator, Validation, ValidatorCtx};
use shamir_funclib::canonical::{canonical_hash, PREV_HASH_FIELD};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{insert, update};
use shamir_query_builder::Query;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// The native CAS validator
// ═══════════════════════════════════════════════════════════════════════

/// Optimistic-concurrency validator: a write is accepted only when the
/// `_prev_hash` it carries matches the canonical hash of the current
/// (`old_record`) version.
///
/// Implements [`RecordValidator`] directly — no `ShamirFunction` overhead.
struct CasValidator;

impl CasValidator {
    /// Build a stale-rejection `Validation`.
    fn stale() -> Validation {
        let mut v = Validation::accept();
        v.field_error(vec![PREV_HASH_FIELD.to_owned()], "stale");
        v
    }
}

#[async_trait]
impl RecordValidator for CasValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        // The presented prev-hash from the new record (None = absent/null).
        let presented = new
            .and_then(|f| f.str(&[PREV_HASH_FIELD]))
            .map(str::to_owned);

        // The materialized old record as QueryValue for canonical_hash.
        let old_qv = old.map(|f| f.to_query_value());

        match (old_qv, presented) {
            // No prior version and no prev-hash asserted → first write, accept.
            (None, None) => Validation::accept(),
            // Prior version exists: the presented hash must match its content.
            (Some(old_val), Some(p)) => {
                // canonical_hash excludes the top-level _prev_hash, so it hashes
                // the *content* of the current version — exactly what the writer
                // must have based its update on.
                let current = canonical_hash(&old_val);
                // Non-secret content hashes; a plain compare is fine (no
                // timing-attack surface — anyone holding the record can
                // recompute the hash).
                if current == p {
                    Validation::accept()
                } else {
                    Self::stale()
                }
            }
            // A prior version exists but no prev-hash was asserted → stale.
            (Some(_), None) => Self::stale(),
            // No prior version but prev-hash was asserted → stale.
            (None, Some(_)) => Self::stale(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Setup helpers
// ═══════════════════════════════════════════════════════════════════════

/// In-memory ShamirDb with `testdb/main/docs`.
async fn setup_db() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("docs"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

/// Create the native CAS validator bound to `docs` on Update.
async fn bind_cas_validator(db: &ShamirDb) {
    let empty_wasm = wat::parse_str("(module)").unwrap();
    let id = db
        .create_validator_from_wasm("v_cas", &empty_wasm, false)
        .await
        .unwrap();

    // Swap the live artifact for the native CAS validator (bindings preserved).
    let swapped = db
        .validators()
        .replace_artifact(&id, Arc::new(CasValidator));
    assert!(swapped, "replace_artifact must find the freshly-created id");

    db.bind_validator(
        "testdb",
        "main",
        "docs",
        "v_cas",
        vec![WriteOp::Update],
        1500,
    )
    .await
    .unwrap();
}

/// Insert one record into `docs` (string fields only).
fn insert_request(id: &str, record: QueryValue) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.insert("ins", insert("docs").row(record));
    b.to_request_via_msgpack()
}

/// Update the single `docs` row matched by `key`, applying `set`.
fn update_request(id: &str, key: &str, set: QueryValue) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.update("upd", update("docs").where_(eq("key", key)).set(set));
    b.to_request_via_msgpack()
}

/// Read all `docs` rows.
fn read_all_request(id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.query("all", Query::from("docs"));
    b.to_request_via_msgpack()
}

/// Read the single stored `docs` row back and compute its canonical content
/// hash — exactly the value a writer must present as `_prev_hash` next.
async fn current_hash(db: &ShamirDb) -> String {
    let resp = db
        .execute("testdb", &read_all_request("read_hash"))
        .await
        .unwrap();
    let records = &resp.results["all"].records;
    assert_eq!(records.len(), 1, "expected exactly one stored row");
    canonical_hash(&records[0].as_value())
}

// ═══════════════════════════════════════════════════════════════════════
// Test 1: the headline CAS sequence — accept, then stale-reject the replay
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cas_accepts_fresh_then_rejects_stale_replay() {
    let db = setup_db().await;
    bind_cas_validator(&db).await;

    // v1: insert (no prior version, no _prev_hash). Insert is not bound, so
    // it is accepted unconditionally.
    db.execute(
        "testdb",
        &insert_request("ins_v1", mpack!({ "key": "doc1", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    let hash_v1 = current_hash(&db).await;

    // v2: update with _prev_hash = hash(v1) → ACCEPTED.
    let upd_v2 = update_request(
        "upd_v2",
        "doc1",
        mpack!({ "body": "v2", "_prev_hash": @(QueryValue::Str(hash_v1.clone())) }),
    );
    db.execute("testdb", &upd_v2)
        .await
        .expect("v2 update with correct prev_hash must be accepted");

    // The row is now v2.
    let resp = db
        .execute("testdb", &read_all_request("after_v2"))
        .await
        .unwrap();
    assert_eq!(
        resp.results["all"].records[0].get_value_str("body"),
        Some("v2")
    );

    // Stale replay: another writer still holds hash(v1) and tries to update
    // AFTER the row already advanced to v2 → REJECTED with `stale`.
    let stale = update_request(
        "upd_stale",
        "doc1",
        mpack!({ "body": "v2_conflict", "_prev_hash": @(QueryValue::Str(hash_v1.clone())) }),
    );
    let err = db
        .execute("testdb", &stale)
        .await
        .expect_err("stale replay must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("stale"),
        "rejection must carry the `stale` code, got: {msg}"
    );
    assert!(
        msg.contains("_prev_hash"),
        "rejection must reference the `_prev_hash` field, got: {msg}"
    );

    // The conflicting write must NOT have landed — body is still v2.
    let resp = db
        .execute("testdb", &read_all_request("after_stale"))
        .await
        .unwrap();
    assert_eq!(
        resp.results["all"].records[0].get_value_str("body"),
        Some("v2"),
        "the stale write must not have overwritten v2"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 2: the correct chain v1 → v2 → v3 with fresh prev_hash each step
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cas_correct_chain_passes_each_step() {
    let db = setup_db().await;
    bind_cas_validator(&db).await;

    db.execute(
        "testdb",
        &insert_request("ins_v1", mpack!({ "key": "chain", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    let hash_v1 = current_hash(&db).await;
    db.execute(
        "testdb",
        &update_request(
            "upd_v2",
            "chain",
            mpack!({ "body": "v2", "_prev_hash": @(QueryValue::Str(hash_v1.clone())) }),
        ),
    )
    .await
    .expect("v2 with prev=hash(v1) must pass");

    let hash_v2 = current_hash(&db).await;
    assert_ne!(hash_v1, hash_v2, "content changed, hash must advance");

    db.execute(
        "testdb",
        &update_request(
            "upd_v3",
            "chain",
            mpack!({ "body": "v3", "_prev_hash": @(QueryValue::Str(hash_v2.clone())) }),
        ),
    )
    .await
    .expect("v3 with prev=hash(v2) must pass");

    let resp = db
        .execute("testdb", &read_all_request("after_v3"))
        .await
        .unwrap();
    assert_eq!(
        resp.results["all"].records[0].get_value_str("body"),
        Some("v3"),
        "the chain v1→v2→v3 must have advanced the row to v3"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Test 3: a blind update (no _prev_hash) on an existing row is rejected
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cas_rejects_blind_update_without_prev_hash() {
    let db = setup_db().await;
    bind_cas_validator(&db).await;

    db.execute(
        "testdb",
        &insert_request("ins_v1", mpack!({ "key": "blind", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    // Update with NO _prev_hash on an existing row → rejected.
    let err = db
        .execute(
            "testdb",
            &update_request("upd_blind", "blind", mpack!({ "body": "v2" })),
        )
        .await
        .expect_err("an update without _prev_hash on an existing row must be rejected");
    assert!(
        err.to_string().contains("stale"),
        "blind update must be rejected as stale, got: {err}"
    );
}
