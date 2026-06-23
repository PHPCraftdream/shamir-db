//! S-write server capability tests — id-keyed msgpack insert path.
//!
//! Exercises the `records_idmsgpack` branch of `execute_insert_tx`:
//!
//! 1. **Byte-identity convergence**: id-keyed and name-keyed inserts of the
//!    same logical record produce byte-identical stored bytes, and the records
//!    de-intern to identical QueryValues.
//! 2. **Indexed insert via records_idmsgpack**: an indexed field inserted via
//!    the id-keyed path is indexed correctly (lookup finds the record).
//! 3. **Security — unresolved key rejected**: bytes with a key id outside the
//!    interner's range are rejected; no record is staged or written.
//! 4. **Validator on id-keyed**: a bound Insert validator runs on id-keyed
//!    records; a violating record is rejected by `run_validators_qv`.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use serde_bytes::ByteBuf;
use smallvec::smallvec;

use shamir_query_builder::write;
use shamir_query_types::write::InsertOp;
use shamir_query_types::TableRef;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::codecs::interned::{query_value_to_storage_bytes, record_view_to_query_value};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::RecordView;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::read::ReadQuery;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::record_cow::RecordCow;
use crate::table::table_manager::TableManager;
use crate::table::tests::write_exec_tests::{insert_via_tx, setup_empty_table};
use crate::table::TableConfig;
use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorBinding, ValidatorCtx, ValidatorRegistry,
    WriteOp,
};
use shamir_types::access::Actor;
use shamir_types::mpack;
use shamir_types::record_view::ScalarRef;

// ============================================================================
// Stub validators
// ============================================================================

/// A validator that rejects records whose "score" field is negative.
///
/// Reads `score` via `new.scalar(&["score"])` using the by-name interface.
struct RejectNegativeScore;

#[async_trait]
impl RecordValidator for RejectNegativeScore {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        let score = new.and_then(|f| f.scalar(&["score"])).and_then(|s| {
            if let ScalarRef::Int(i) = s {
                Some(i)
            } else {
                None
            }
        });
        if matches!(score, Some(n) if n < 0) {
            Validation::reject("score_negative")
        } else {
            Validation::accept()
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Create a bare `TableManager` backed by two in-memory stores.
async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

/// Create a `TableManager` with a `ValidatorRegistry` containing the given
/// bindings.
async fn make_table_with_validator(
    bindings: Vec<ValidatorBinding>,
    registry: Arc<ValidatorRegistry>,
) -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    crate::validator::persistence::save_validators_metadata(&bindings, &info)
        .await
        .unwrap();

    let mut tm = TableManager::create("t".into(), data, info).await.unwrap();
    tm.set_validator_registry(registry);
    tm
}

/// Build an `InsertOp` with only `records_idmsgpack` populated (no `values`).
fn idmsgpack_op(table: &str, bufs: Vec<ByteBuf>) -> InsertOp {
    InsertOp {
        insert_into: TableRef::new(table),
        values: Vec::new(),
        records_idmsgpack: bufs,
    }
}

/// Run `execute_insert_tx` directly without commit (staging only).
/// Used by tests that only need the `execute_insert_tx` return value
/// (security rejection, validator rejection) — no committed data is needed.
async fn stage_insert(
    tbl: &TableManager,
    op: &InsertOp,
) -> Result<crate::query::write::WriteResult, shamir_storage::error::DbError> {
    let mut tx = TxContext::new(TxId::new(99), 0, u64::MAX, IsolationLevel::Snapshot);
    tx.implicit = true;
    tbl.execute_insert_tx(op, &mut tx, true).await
}

/// Collect all (RecordId, raw_bytes) pairs from the committed table store.
async fn collect_all_raw(tbl: &TableManager) -> Vec<(RecordId, bytes::Bytes)> {
    let stream = tbl.list_stream(100);
    futures::pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(batch) = stream.next().await {
        for (rid, cow) in batch.unwrap() {
            let raw = match cow {
                RecordCow::Borrowed(b) => b,
                RecordCow::Owned(iv) => iv.to_bytes().unwrap(),
            };
            out.push((rid, raw));
        }
    }
    out
}

// ============================================================================
// Test 1 — Byte-identity convergence: id-keyed ≡ name-keyed
// ============================================================================

/// The KEY convergence test.
///
/// Take a logical record `{ "name": "Alice", "age": 30 }` and insert it via:
///   - Path A: `values` (name-keyed QueryValue — server interns and encodes).
///   - Path B: `records_idmsgpack` (pre-encoded id-keyed bytes using the same
///     interner, mimicking a client that has already interned all field names).
///
/// Both paths must produce the SAME stored bytes (byte-identical after commit).
/// The records must also de-intern to identical QueryValues.
#[tokio::test]
async fn idmsgpack_byte_identity_convergence() {
    let (table, repo) = setup_empty_table().await;
    let interner = table.interner().get().await.unwrap();

    // Persist the interner so path A and path B share a stable id namespace.
    // Path A (values) also calls touch_ind for "name" and "age" during execution
    // (idempotent); path B's intern_fn below returns the same ids.
    table.interner().persist().await.unwrap();

    // ── Path A: name-keyed via `values` ──────────────────────────────────
    let op_a = write::insert("users")
        .row(mpack!({ "name": "Alice", "age": 30 }))
        .build();
    let res_a = insert_via_tx(&repo, &table, &op_a, true).await.unwrap();
    assert_eq!(res_a.affected, 1, "path-A must insert 1 record");

    let id_a_str = res_a.records[0]
        .get_value_owned("_id")
        .and_then(|v| v.as_str().map(str::to_owned))
        .expect("path-A: _id must be present in result");
    let id_a: RecordId = id_a_str.parse().expect("_id must be a valid RecordId");

    // ── Path B: id-keyed via `records_idmsgpack` ─────────────────────────
    // Pre-encode the same logical record using `query_value_to_storage_bytes`
    // and the server interner — the canonical client-side pre-encoding path.
    // This produces byte-identical output to what the server's `values` path
    // would produce (same codec, same interned key ids, same field order).
    let record_qv = mpack!({ "name": "Alice", "age": 30 });
    let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
        interner
            .touch_ind(key)
            .map(|ti| ti.into_key())
            .map_err(|e| {
                shamir_types::codecs::CodecError::Decode(format!("intern '{}': {}", key, e))
            })
    };
    let pre_encoded = query_value_to_storage_bytes(&record_qv, &intern_fn).unwrap();

    let op_b = idmsgpack_op("users", vec![ByteBuf::from(pre_encoded.to_vec())]);
    let table_b = table.clone();
    let res_b = repo
        .run_implicit_batch_tx(Actor::System, "idmsgpack_convergence_B", move |tx| {
            Box::pin(async move { table_b.execute_insert_tx(&op_b, tx, true).await })
        })
        .await
        .unwrap();
    assert_eq!(res_b.affected, 1, "path-B must insert 1 record");

    // Both records committed — table must have 2.
    assert_eq!(table.count().await.unwrap(), 2, "table must have 2 records");

    // ── Stored bytes comparison ───────────────────────────────────────────
    let all_raw = collect_all_raw(&table).await;
    assert_eq!(all_raw.len(), 2, "store must have 2 entries");

    let bytes_a = all_raw
        .iter()
        .find(|(id, _)| *id == id_a)
        .map(|(_, b)| b.clone())
        .expect("path-A record must be present in the store");
    let bytes_b = all_raw
        .iter()
        .find(|(id, _)| *id != id_a)
        .map(|(_, b)| b.clone())
        .expect("path-B record must be present in the store");

    assert_eq!(
        bytes_a.as_ref(),
        bytes_b.as_ref(),
        "stored bytes must be byte-identical:\n  path-A: {bytes_a:?}\n  path-B: {bytes_b:?}"
    );

    // ── Read-back / de-intern comparison ─────────────────────────────────
    let view_a = RecordView::new(&bytes_a).unwrap();
    let view_b = RecordView::new(&bytes_b).unwrap();
    let qv_a = record_view_to_query_value(&view_a, interner).unwrap();
    let qv_b = record_view_to_query_value(&view_b, interner).unwrap();
    assert_eq!(
        qv_a, qv_b,
        "de-interned QueryValues must match:\n  path-A: {qv_a:?}\n  path-B: {qv_b:?}"
    );
}

// ============================================================================
// Test 2 — Indexed insert via records_idmsgpack
// ============================================================================

/// An indexed field inserted via `records_idmsgpack` must be posted to the
/// index so that a subsequent equality lookup finds the record.
#[tokio::test]
async fn idmsgpack_indexed_insert() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let table = db.get_table("default", "users").await.unwrap();
    let repo = db.get_repo("default").unwrap();

    // Intern field names so we can pre-encode with the server's key ids.
    let interner = table.interner().get().await.unwrap();
    let name_k = interner.touch_ind("name").unwrap().into_key();
    let status_k = interner.touch_ind("status").unwrap().into_key();
    table.interner().persist().await.unwrap();

    // Create an index on "status".
    table.create_index("status_idx", &["status"]).await.unwrap();

    // Pre-encode { name: "Alice", status: "active" } id-keyed.
    let mut m = new_map();
    m.insert(name_k, InnerValue::Str("Alice".into()));
    m.insert(status_k, InnerValue::Str("active".into()));
    let bytes = InnerValue::Map(m).to_bytes().unwrap();

    // Insert via records_idmsgpack through the production implicit-tx path.
    let op = idmsgpack_op("users", vec![ByteBuf::from(bytes.to_vec())]);
    let result = repo
        .run_implicit_batch_tx(Actor::System, "test_idmsgpack_indexed", move |tx| {
            let owned_table = table.clone();
            let owned_op = op.clone();
            Box::pin(async move { owned_table.execute_insert_tx(&owned_op, tx, true).await })
        })
        .await
        .unwrap();
    assert_eq!(result.affected, 1);

    // Query via the index: WHERE status = "active" must find Alice.
    let table2 = db.get_table("default", "users").await.unwrap();
    assert_eq!(table2.count().await.unwrap(), 1);

    let interner2 = table2.interner().get().await.unwrap();
    let refs = shamir_types::types::common::new_map();
    let ctx = crate::query::filter::eval_context::FilterContext::new(interner2, &refs);
    let query = ReadQuery::new("users").filter(shamir_query_types::filter::Filter::Eq {
        field: shamir_query_types::filter::FieldPath::from(vec!["status".into()]),
        value: shamir_query_types::filter::FilterValue::String("active".into()),
    });
    let read_result = table2.read(&query, &ctx).await.unwrap();

    assert_eq!(
        read_result.records.len(),
        1,
        "index lookup must return exactly 1 record"
    );
    assert_eq!(
        read_result
            .stats
            .as_ref()
            .and_then(|s| s.index_used.as_deref()),
        Some("status_idx"),
        "query must be served via the index"
    );
}

// ============================================================================
// Test 3 — Security: unresolved key rejected, nothing staged
// ============================================================================

/// Bytes containing a key id that does NOT exist in the server's interner
/// must be rejected BEFORE staging by `validate_keys_resolve_interner`.
///
/// The whole INSERT must return `Err` and leave no record in the staging
/// write-set.
#[tokio::test]
async fn idmsgpack_security_unresolved_key_rejected() {
    let tbl = make_table().await;
    let interner = tbl.interner().get().await.unwrap();

    // Intern two real keys so the interner is non-empty.
    let _ = interner.touch_ind("real_field_a").unwrap().into_key();
    let _ = interner.touch_ind("real_field_b").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    // Forge a record with key id = 9999 — no such id in the interner.
    let forged_key = InternerKey::new(9999);
    let mut m = new_map();
    m.insert(forged_key, InnerValue::Int(42));
    let forged_bytes = InnerValue::Map(m).to_bytes().unwrap();

    let op = idmsgpack_op("t", vec![ByteBuf::from(forged_bytes.to_vec())]);
    let err = stage_insert(&tbl, &op).await;

    assert!(
        err.is_err(),
        "insert with unresolved key id=9999 must return Err, got: Ok"
    );
}

// ============================================================================
// Test 4 — Validator on id-keyed insert
// ============================================================================

/// A bound Insert validator runs on id-keyed records.
///
/// - `{ name: "Alice", score: 10 }` — valid (score ≥ 0) → `execute_insert_tx` returns `Ok`.
/// - `{ name: "Bob",   score: -5 }` — invalid (score < 0) → returns `Err`.
///
/// No commit is needed: the `execute_insert_tx` return value is authoritative.
/// Rejection happens BEFORE `insert_tx_many_bytes` so the staging is aborted.
#[tokio::test]
async fn idmsgpack_validator_rejects_violating_record() {
    let val_id = RecordId::system("reject_negative_score");
    let reg = Arc::new(ValidatorRegistry::new());
    reg.register(
        val_id,
        "reject_negative_score",
        Arc::new(RejectNegativeScore) as Arc<dyn RecordValidator>,
    )
    .unwrap();

    let bindings = vec![ValidatorBinding {
        validator_id: val_id,
        ops: smallvec![WriteOp::Insert],
        priority: 1000,
    }];

    let tbl = make_table_with_validator(bindings, reg).await;
    let interner = tbl.interner().get().await.unwrap();

    // Intern field names so the id-keyed bytes have valid, resolvable key ids.
    let name_k = interner.touch_ind("name").unwrap().into_key();
    let score_k = interner.touch_ind("score").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    // ── Valid record (score = 10): must be accepted ───────────────────────
    let mut m_ok = new_map();
    m_ok.insert(name_k.clone(), InnerValue::Str("Alice".into()));
    m_ok.insert(score_k.clone(), InnerValue::Int(10));
    let bytes_ok = InnerValue::Map(m_ok).to_bytes().unwrap();

    let op_ok = idmsgpack_op("t", vec![ByteBuf::from(bytes_ok.to_vec())]);
    let res_ok = stage_insert(&tbl, &op_ok).await;
    assert!(
        res_ok.is_ok(),
        "valid record (score=10) must be accepted by the validator, got: {res_ok:?}"
    );

    // ── Invalid record (score = -5): must be rejected ─────────────────────
    let mut m_bad = new_map();
    m_bad.insert(name_k, InnerValue::Str("Bob".into()));
    m_bad.insert(score_k, InnerValue::Int(-5));
    let bytes_bad = InnerValue::Map(m_bad).to_bytes().unwrap();

    let op_bad = idmsgpack_op("t", vec![ByteBuf::from(bytes_bad.to_vec())]);
    let res_bad = stage_insert(&tbl, &op_bad).await;
    assert!(
        res_bad.is_err(),
        "record with score=-5 must be rejected by the validator, got: Ok"
    );
}

// ============================================================================
// Test 5 — return_result: id-keyed records appear in RETURNING rows
// ============================================================================

/// INSERT via `records_idmsgpack` with `return_result=true` must return
/// `records.len() == affected` and each returned row must carry the correct
/// name-keyed fields (de-interned from the id-keyed bytes).
///
/// This is the regression test for the bug where `build_insert_result_records`
/// only produced rows for the `values` branch, silently dropping id-keyed
/// rows from the RETURNING list.
#[tokio::test]
async fn idmsgpack_return_result_records_match_affected() {
    let (table, repo) = setup_empty_table().await;
    let interner = table.interner().get().await.unwrap();

    // Pre-encode two records id-keyed, simulating a v2 pass-through client.
    let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
        interner
            .touch_ind(key)
            .map(|ti| ti.into_key())
            .map_err(|e| {
                shamir_types::codecs::CodecError::Decode(format!("intern '{}': {}", key, e))
            })
    };

    let rec_a = mpack!({
        "name": "Alice",
        "age": 30
    });
    let rec_b = mpack!({
        "name": "Bob",
        "age": 25
    });

    let bytes_a = query_value_to_storage_bytes(&rec_a, &intern_fn).unwrap();
    let bytes_b = query_value_to_storage_bytes(&rec_b, &intern_fn).unwrap();

    let op = idmsgpack_op(
        "users",
        vec![
            ByteBuf::from(bytes_a.to_vec()),
            ByteBuf::from(bytes_b.to_vec()),
        ],
    );

    let table_c = table.clone();
    let result = repo
        .run_implicit_batch_tx(Actor::System, "idmsgpack_returning", move |tx| {
            Box::pin(async move { table_c.execute_insert_tx(&op, tx, true).await })
        })
        .await
        .unwrap();

    // Core invariant: records.len() == affected.
    assert_eq!(result.affected, 2);
    assert_eq!(
        result.records.len(),
        result.affected as usize,
        "records.len() must equal affected — every inserted record \
         gets a RETURNING row"
    );

    // Each returned row must carry an `_id` and the correct field names.
    for (i, rec) in result.records.iter().enumerate() {
        assert!(
            rec.get_value_owned("_id").is_some(),
            "record[{i}] must have _id in RETURNING row"
        );
        assert!(
            rec.get_value_owned("name").is_some(),
            "record[{i}] must have 'name' (de-interned) in RETURNING row"
        );
        assert!(
            rec.get_value_owned("age").is_some(),
            "record[{i}] must have 'age' (de-interned) in RETURNING row"
        );
    }

    // Verify field values match what was inserted.
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice".to_string()
        ))
    );
    assert_eq!(
        result.records[0].get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(30))
    );
    assert_eq!(
        result.records[1].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Bob".to_string()
        ))
    );
    assert_eq!(
        result.records[1].get_value_owned("age"),
        Some(shamir_types::types::value::QueryValue::Int(25))
    );
}

/// Mixed INSERT: both `values` and `records_idmsgpack` populated,
/// `return_result=true`. The combined RETURNING list must have
/// `records.len() == affected` with values-branch rows first, then
/// id-keyed rows.
#[tokio::test]
async fn mixed_values_and_idmsgpack_return_result() {
    let (table, repo) = setup_empty_table().await;
    let interner = table.interner().get().await.unwrap();

    // Pre-encode one id-keyed record.
    let intern_fn = |key: &str| -> Result<InternerKey, shamir_types::codecs::CodecError> {
        interner
            .touch_ind(key)
            .map(|ti| ti.into_key())
            .map_err(|e| {
                shamir_types::codecs::CodecError::Decode(format!("intern '{}': {}", key, e))
            })
    };

    let rec_id = mpack!({
        "name": "Bob",
        "age": 25
    });
    let bytes_id = query_value_to_storage_bytes(&rec_id, &intern_fn).unwrap();

    // Build an InsertOp with BOTH branches populated.
    let mut op: InsertOp = write::insert("users")
        .row(mpack!({
            "name": "Alice",
            "age": 30
        }))
        .build();
    op.records_idmsgpack = vec![ByteBuf::from(bytes_id.to_vec())];

    let table_c = table.clone();
    let result = repo
        .run_implicit_batch_tx(Actor::System, "mixed_returning", move |tx| {
            Box::pin(async move { table_c.execute_insert_tx(&op, tx, true).await })
        })
        .await
        .unwrap();

    assert_eq!(result.affected, 2);
    assert_eq!(
        result.records.len(),
        result.affected as usize,
        "mixed insert: records.len() must equal affected"
    );

    // Values-branch row first, then id-keyed row.
    assert_eq!(
        result.records[0].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Alice".to_string()
        ))
    );
    assert_eq!(
        result.records[1].get_value_owned("name"),
        Some(shamir_types::types::value::QueryValue::Str(
            "Bob".to_string()
        ))
    );
}
