//! Finding 2b-ii regression: reactive `Batch` deliveries must honour the
//! operator-configured `BatchLimits`, not `BatchLimits::default()`.
//!
//! A cheap external insert can fan out to N reactive re-queries under the
//! subscribing actor's identity; those re-queries must be bounded by the
//! actor's configured limits. We prove this by driving the reactive wrapper
//! with a restrictive limit (`max_queries = 0`) and asserting the resulting
//! payload carries an execution error — whereas default limits succeed.

use std::sync::Arc;

use shamir_db::access::Actor;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_types::batch::{BatchLimits, SubBatchOp};
use shamir_tx::changefeed::RecordChange;
use shamir_tx::ChangeOp;

use crate::subscriptions::reactive::execute_reactive_batch;

fn dummy_change() -> RecordChange {
    RecordChange {
        table: "users".into(),
        key: bytes::Bytes::from_static(b"k"),
        op: ChangeOp::Put,
        value: None,
    }
}

fn sub_batch() -> SubBatchOp {
    let query = Query::with_repo("main", "users").build();
    let mut b = Batch::new();
    b.query("_q", query);
    b.return_all();
    SubBatchOp {
        batch: b.build(),
        bind: Default::default(),
    }
}

/// The reactive output is a msgpack blob; helper to check whether it mentions
/// an execution error (limit rejection) anywhere in the encoded bytes.
fn payload_mentions_error(bytes: &[u8], needle: &str) -> bool {
    // The reactive wrapper serialises `BatchResponse`; a per-query failure
    // surfaces as an error entry. Scan the raw utf8-ish bytes for the marker
    // rather than fully decoding the nested response shape.
    let text = String::from_utf8_lossy(bytes);
    text.contains(needle)
}

#[tokio::test]
async fn reactive_batch_honours_restrictive_query_limits() {
    let db = Arc::new(ShamirDb::init_memory().await.unwrap());
    // The reactive wrapper is executed against a real database; create it so
    // `execute_as` reaches the planner (where the query-limit check lives)
    // rather than failing early on "database not found".
    db.create_db("test_db").await;
    let change = dummy_change();
    let sb = sub_batch();

    // Restrictive operator limit: 0 queries allowed. The reactive wrapper
    // holds exactly one query, so honouring this limit must fail the batch.
    let restrictive = BatchLimits {
        max_queries: 0,
        ..BatchLimits::default()
    };
    let out_restrictive = execute_reactive_batch(
        &db,
        "test_db",
        &Actor::System,
        &sb,
        &change,
        1,
        &restrictive,
    )
    .await;

    // Default (permissive) limit: the same wrapper must NOT be rejected on
    // the query-count limit.
    let out_default = execute_reactive_batch(
        &db,
        "test_db",
        &Actor::System,
        &sb,
        &change,
        1,
        &BatchLimits::default(),
    )
    .await;

    // "Too many queries" is the exact `BatchError::TooManyQueries` Display
    // text — it appears ONLY on the max_queries-limit rejection path, so it
    // cleanly distinguishes a limit rejection from any unrelated per-query
    // error (e.g. a missing table).
    assert!(
        payload_mentions_error(&out_restrictive, "Too many queries"),
        "restrictive max_queries=0 limit must reject the reactive batch \
         (was BatchLimits::default() being used instead of query_limits?)"
    );
    assert!(
        !payload_mentions_error(&out_default, "Too many queries"),
        "default limits must not trigger a query-count rejection"
    );
}
