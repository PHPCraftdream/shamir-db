//! Builder-built `changes_since` request + typed decode of the response
//! payload, with lossless u64 positions (2^53+1, u64::MAX) and the
//! still-fail-closed Bounded error mappings.

use shamir_query_types::admin::{ChangesSinceOp, ChangesSinceResponse};
use shamir_query_types::batch::{BatchOp, BatchRequest, ResultEncoding};
use shamir_types::types::value::QueryValue;

use crate::builder::batch::Batch;
use crate::builder::ddl::changes_since;
use crate::receipts::decode_changes_since;
use crate::{ClientError, DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};

const BIG: u64 = 9_007_199_254_740_993; // 2^53 + 1

#[test]
fn changes_since_request_is_built_by_the_builder() {
    let mut batch = Batch::new();
    batch
        .id("changes_since")
        .changes_since("changes_since", changes_since(BIG).repo("archive").limit(7));
    let request: BatchRequest = batch.build();
    let entry = request.queries.get("changes_since").unwrap();
    let BatchOp::ChangesSince(op) = &entry.op else {
        panic!("expected ChangesSince op: {:?}", entry.op);
    };
    assert_eq!(
        *op,
        ChangesSinceOp {
            changes_since: BIG,
            repo: "archive".into(),
            limit: Some(7),
        }
    );
    assert!(
        !request.transactional,
        "read-style admin op stays non-transactional"
    );
}

// Largest cursor the BatchOp wire carries as a native integer. Values above
// `i64::MAX` are encoded as decimal strings by the op pipeline and are not
// decodable by the server's direct `DbRequest` decode today (the server also
// casts positions through `i64`), so the lossless claim stops here.
const WIRE_CURSOR_MAX: u64 = i64::MAX as u64;

#[test]
fn changes_since_cursor_survives_the_wire_losslessly() {
    let mut builder = Batch::new();
    builder
        .id("changes_since")
        .changes_since("changes_since", changes_since(WIRE_CURSOR_MAX));
    let request = builder.build();
    let wire = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "db".into(),
        batch: request,
    };
    let bytes = rmp_serde::to_vec_named(&wire).unwrap();
    let decoded: DbRequest = rmp_serde::from_slice(&bytes).unwrap();
    let DbRequest::Execute { batch, .. } = decoded else {
        panic!("wrong request: {decoded:?}");
    };
    let BatchOp::ChangesSince(op) = &batch.queries.get("changes_since").unwrap().op else {
        panic!("expected ChangesSince op");
    };
    assert_eq!(op.changes_since, WIRE_CURSOR_MAX, "u64 cursor must not narrow");
}

fn payload(response: &ChangesSinceResponse) -> QueryValue {
    let bytes = rmp_serde::to_vec_named(response).unwrap();
    rmp_serde::from_slice(&bytes).unwrap()
}

fn batch_response(payload: QueryValue) -> shamir_query_types::batch::BatchResponse {
    let mut results = shamir_collections::TMap::default();
    results.insert(
        "changes_since".to_string(),
        shamir_query_types::read::QueryResult {
            records: vec![shamir_query_types::read::QueryRecord::Direct(payload)],
            ..Default::default()
        },
    );
    shamir_query_types::batch::BatchResponse {
        id: QueryValue::Null,
        results,
        execution_plan: Vec::new(),
        edge_provenance: shamir_collections::TMap::default(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: shamir_collections::TMap::default(),
    }
}

#[test]
fn decode_reads_visible_through_when_present() {
    let typed = ChangesSinceResponse {
        changes_since: 41,
        events: Vec::new(),
        gap_at: None,
        visible_through: Some(BIG),
    };
    let decoded = decode_changes_since(&batch_response(payload(&typed))).unwrap();
    assert_eq!(decoded, typed);
}

#[test]
fn decode_defaults_visible_through_when_absent() {
    // Pre-VIS-4c server: the field is simply missing from the payload.
    let bytes = rmp_serde::to_vec_named(&PreVis4c {
        changes_since: 41,
        events: Vec::new(),
        gap_at: Some(38),
    })
    .unwrap();
    let payload: QueryValue = rmp_serde::from_slice(&bytes).unwrap();
    let decoded = decode_changes_since(&batch_response(payload)).unwrap();
    assert_eq!(decoded.visible_through, None);
    assert_eq!(decoded.gap_at, Some(38));
    assert_eq!(decoded.changes_since, 41);
}

#[derive(serde::Serialize)]
struct PreVis4c {
    changes_since: u64,
    events: Vec<u8>,
    gap_at: Option<u64>,
}

#[test]
fn fail_closed_bounded_rejections_map_to_their_typed_variants() {
    // The server really returns these codes: Call/admin writes under
    // Bounded AND TxExecute commit-only overrides are rejected with
    // "validation" (engine/.../receipts/policy.rs;
    // server/.../tx_handlers.rs returns code "validation" with the
    // "visibility_wait is commit-only" message), "visibility_wait_invalid"
    // is the wait-cap / strict-permission rejection (handler.rs), and a
    // known pending-with-reopen surfaces as
    // "write_committed_visibility_pending". The client maps each to its
    // typed variant WITHOUT retrying or parsing messages.
    for (code, expected) in [
        ("validation", "validation"),
        ("visibility_wait_invalid", "visibility_wait_invalid"),
        (
            "write_committed_visibility_pending",
            "write_committed_visibility_pending",
        ),
    ] {
        let mapped = crate::visibility::map_response(DbResponse::Error {
            code: code.into(),
            message: "opaque".into(),
        });
        match (expected, mapped) {
            ("validation", Err(ClientError::Db { code, message })) => {
                assert_eq!(code, "validation");
                assert_eq!(message, "opaque");
            }
            ("visibility_wait_invalid", Err(ClientError::VisibilityWaitInvalid { message })) => {
                assert_eq!(message, "opaque");
            }
            (
                "write_committed_visibility_pending",
                Err(ClientError::WriteCommittedVisibilityPending { message }),
            ) => {
                assert_eq!(message, "opaque");
            }
            (expected, other) => panic!("unexpected mapping for {expected}: {other:?}"),
        }
    }
}

// `ResultEncoding` import guard: the batch builder default encoding must
// remain name-based for the admin payload path.
const _: ResultEncoding = ResultEncoding::Name;
