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
//! swap the live registry artifact for a native [`ShamirFunction`]
//! (`CasValidator`) through the additive
//! `ValidatorRegistry::replace_artifact`. Bindings created by `bind_validator`
//! are preserved across the swap. The native validator receives `record` /
//! `old_record` as `QueryValue` (string-keyed) in its `Params` — exactly the
//! shape the engine's `run_validators` builds — computes
//! `canonical_hash(old_record)` and compares it to `record["_prev_hash"]`.
//!
//! The validator and the test compute the expected hash through the **same**
//! `shamir_funclib::canonical::canonical_hash` over a string-keyed
//! `QueryValue`, so the two sides agree bit-for-bit. Records use string fields
//! only, so the JSON read-back round-trips to the identical `QueryValue` the
//! validator hashes.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::engine::validator::WriteOp;
use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_engine::function::{FnBatch, FnCtx, FunctionError, Params, ShamirFunction};
use shamir_funclib::canonical::{canonical_hash, PREV_HASH_FIELD};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{insert, update};
use shamir_query_builder::Query;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

fn to_req(b: &Batch) -> BatchRequest {
    let bytes = b.to_msgpack().expect("msgpack encode");
    rmp_serde::from_slice(&bytes).expect("msgpack decode")
}

// ═══════════════════════════════════════════════════════════════════════
// The native CAS validator
// ═══════════════════════════════════════════════════════════════════════

/// Optimistic-concurrency validator: a write is accepted only when the
/// `_prev_hash` it carries matches the canonical hash of the current
/// (`old_record`) version.
///
/// ABI: returns msgpack-able `QueryValue`:
/// - `Null` → valid.
/// - `{"errors":[{"field":["_prev_hash"],"code":"stale"}]}` → rejected.
struct CasValidator;

impl CasValidator {
    /// Build the `{"errors":[{"field":["_prev_hash"],"code":"stale"}]}` reject.
    fn stale() -> QueryValue {
        let mut err = new_map();
        err.insert(
            "field".to_owned(),
            QueryValue::List(vec![QueryValue::Str(PREV_HASH_FIELD.to_owned())]),
        );
        err.insert("code".to_owned(), QueryValue::Str("stale".to_owned()));
        let mut root = new_map();
        root.insert(
            "errors".to_owned(),
            QueryValue::List(vec![QueryValue::Map(err)]),
        );
        QueryValue::Map(root)
    }
}

#[async_trait]
impl ShamirFunction for CasValidator {
    async fn call(
        &self,
        _ctx: &FnCtx,
        _batch: &FnBatch,
        params: &Params,
    ) -> Result<QueryValue, FunctionError> {
        let record = params.get("record")?;
        let old_record = params.get("old_record")?;

        // The presented prev-hash (None / Null when absent).
        let presented = match record {
            QueryValue::Map(m) => match m.get(PREV_HASH_FIELD) {
                Some(QueryValue::Str(s)) => Some(s.clone()),
                Some(QueryValue::Null) | None => None,
                Some(_) => {
                    return Err(FunctionError::User(
                        "_prev_hash must be a string".to_owned(),
                    ))
                }
            },
            _ => None,
        };

        match (old_record, presented) {
            // No prior version and no prev-hash asserted → first write, accept.
            (QueryValue::Null, None) => Ok(QueryValue::Null),
            // Prior version exists: the presented hash must match its content.
            (old, Some(p)) => {
                // canonical_hash excludes the top-level _prev_hash, so it hashes
                // the *content* of the current version — exactly what the writer
                // must have based its update on.
                let current = canonical_hash(old);
                // Non-secret content hashes; a plain compare is fine (no
                // timing-attack surface — anyone holding the record can
                // recompute the hash).
                if current == p {
                    Ok(QueryValue::Null)
                } else {
                    Ok(Self::stale())
                }
            }
            // A prior version exists but no prev-hash was asserted → stale
            // (the writer is overwriting blind).
            (_old, None) => Ok(Self::stale()),
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
fn insert_request(id: &str, record: serde_json::Value) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.insert("ins", insert("docs").row(record));
    to_req(&b)
}

/// Update the single `docs` row matched by `key`, applying `set`.
fn update_request(id: &str, key: &str, set: serde_json::Value) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.update("upd", update("docs").where_(eq("key", key)).set(set));
    to_req(&b)
}

/// Read all `docs` rows.
fn read_all_request(id: &str) -> BatchRequest {
    let mut b = Batch::new();
    b.id(id);
    b.query("all", Query::from("docs"));
    to_req(&b)
}

/// Convert a JSON object record into a string-keyed `QueryValue::Map`.
///
/// Records in these tests are flat string→string maps, so this mirrors exactly
/// the `QueryValue` the validator hashes (the engine stores strings and reads
/// them back as JSON strings).
fn json_record_to_query_value(v: &serde_json::Value) -> QueryValue {
    match v {
        serde_json::Value::Null => QueryValue::Null,
        serde_json::Value::Bool(b) => QueryValue::Bool(*b),
        serde_json::Value::String(s) => QueryValue::Str(s.clone()),
        serde_json::Value::Number(n) => {
            // Tests use string fields only; keep ints exact if any appear.
            QueryValue::Int(n.as_i64().expect("test records use string/int only"))
        }
        serde_json::Value::Array(a) => {
            QueryValue::List(a.iter().map(json_record_to_query_value).collect())
        }
        serde_json::Value::Object(o) => {
            let mut m = new_map();
            for (k, val) in o {
                m.insert(k.clone(), json_record_to_query_value(val));
            }
            QueryValue::Map(m)
        }
    }
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
    canonical_hash(&json_record_to_query_value(&records[0]))
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
        &insert_request("ins_v1", json!({ "key": "doc1", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    let hash_v1 = current_hash(&db).await;

    // v2: update with _prev_hash = hash(v1) → ACCEPTED.
    let upd_v2 = update_request(
        "upd_v2",
        "doc1",
        json!({ "body": "v2", "_prev_hash": hash_v1 }),
    );
    db.execute("testdb", &upd_v2)
        .await
        .expect("v2 update with correct prev_hash must be accepted");

    // The row is now v2.
    let resp = db
        .execute("testdb", &read_all_request("after_v2"))
        .await
        .unwrap();
    assert_eq!(resp.results["all"].records[0]["body"], json!("v2"));

    // Stale replay: another writer still holds hash(v1) and tries to update
    // AFTER the row already advanced to v2 → REJECTED with `stale`.
    let stale = update_request(
        "upd_stale",
        "doc1",
        json!({ "body": "v2_conflict", "_prev_hash": hash_v1 }),
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
        resp.results["all"].records[0]["body"],
        json!("v2"),
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
        &insert_request("ins_v1", json!({ "key": "chain", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    let hash_v1 = current_hash(&db).await;
    db.execute(
        "testdb",
        &update_request(
            "upd_v2",
            "chain",
            json!({ "body": "v2", "_prev_hash": hash_v1 }),
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
            json!({ "body": "v3", "_prev_hash": hash_v2 }),
        ),
    )
    .await
    .expect("v3 with prev=hash(v2) must pass");

    let resp = db
        .execute("testdb", &read_all_request("after_v3"))
        .await
        .unwrap();
    assert_eq!(
        resp.results["all"].records[0]["body"],
        json!("v3"),
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
        &insert_request("ins_v1", json!({ "key": "blind", "body": "v1" })),
    )
    .await
    .expect("v1 insert must succeed");

    // Update with NO _prev_hash on an existing row → rejected.
    let err = db
        .execute(
            "testdb",
            &update_request("upd_blind", "blind", json!({ "body": "v2" })),
        )
        .await
        .expect_err("an update without _prev_hash on an existing row must be rejected");
    assert!(
        err.to_string().contains("stale"),
        "blind update must be rejected as stale, got: {err}"
    );
}
