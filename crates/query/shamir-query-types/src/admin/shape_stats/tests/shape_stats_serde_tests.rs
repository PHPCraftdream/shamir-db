//! Deterministic serde tests for the shape-statistics DTOs (R4b step 4).
//!
//! Fixed fixtures only — no randomness, no timing: the assertions pin the
//! exact wire shape (flat `list_shape_stats` / `reset_shape_stats` tags,
//! camel-free snake_case page fields) so a shape change is a conscious
//! protocol change.

use serde_json::{json, Value};

use crate::admin::shape_stats::{
    ListShapeStatsReq, ResetShapeStatsReq, ShapeStatsPage, ShapeStatsRow, ShapeStatsTotals,
};
use crate::batch::BatchOp;
use crate::wire::DbResponse;

fn full_request() -> ListShapeStatsReq {
    ListShapeStatsReq {
        repo: "main".to_string(),
        limit: Some(64),
    }
}

fn sample_page() -> ShapeStatsPage {
    ShapeStatsPage {
        rows: vec![
            ShapeStatsRow {
                shape_hex: "0123456789abcdef0123456789abcdef".to_string(),
                executions: 7,
                total_nanos: 9_000,
                errors: 1,
            },
            ShapeStatsRow {
                shape_hex: "fedcba9876543210fedcba9876543210".to_string(),
                executions: 2,
                total_nanos: 500,
                errors: 0,
            },
        ],
        other: ShapeStatsTotals {
            executions: 3,
            total_nanos: 120,
            errors: 1,
        },
        tracked: 2,
        truncated: true,
    }
}

#[test]
fn list_shape_stats_req_wire_shape_is_flat_tagged() {
    let value = serde_json::to_value(full_request()).expect("serialize");
    assert_eq!(
        value,
        json!({
            "list_shape_stats": true,
            "repo": "main",
            "limit": 64,
        })
    );
}

#[test]
fn list_shape_stats_req_omits_unset_limit_and_defaults_to_none() {
    // mutant: serialize `limit: null` instead of omitting the key => the
    // empty-option wire shape below changes.
    let request = ListShapeStatsReq {
        repo: "main".to_string(),
        limit: None,
    };
    let value = serde_json::to_value(&request).expect("serialize");
    assert_eq!(
        value,
        json!({
            "list_shape_stats": true,
            "repo": "main",
        })
    );
    assert!(
        value.get("limit").is_none(),
        "absent limit must not render as null"
    );
    // mutant: default a missing `limit` to Some(0) instead of None.
    let decoded: ListShapeStatsReq = serde_json::from_value(value).expect("decode");
    assert_eq!(decoded, request);
    assert_eq!(decoded.limit, None);
}

#[test]
fn list_shape_stats_req_msgpack_round_trips() {
    // mutant: a tag/field rename or a non-round-tripping custom Serialize.
    let request = full_request();
    let bytes = rmp_serde::to_vec_named(&request).expect("encode");
    let decoded: ListShapeStatsReq = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, request);
}

#[test]
fn list_shape_stats_req_rejects_unknown_fields() {
    // mutant: a lenient visitor that ignores unknown keys would accept
    // `shape_hex` here (a privacy/smuggling channel).
    let bytes = rmp_serde::to_vec_named(&json!({
        "list_shape_stats": true,
        "repo": "main",
        "shape_hex": "0123456789abcdef0123456789abcdef",
    }))
    .expect("encode");
    let result: Result<ListShapeStatsReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "unknown fields must be denied");
}

#[test]
fn list_shape_stats_req_requires_repo() {
    // mutant: defaulting `repo` (or tolerating its absence) would list some
    // other repository's shapes.
    let bytes = rmp_serde::to_vec_named(&json!({
        "list_shape_stats": true,
    }))
    .expect("encode");
    let result: Result<ListShapeStatsReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "missing repo must be rejected");
}

#[test]
fn reset_shape_stats_req_wire_shape_and_round_trip() {
    // mutant: omitting the discriminator on serialize => the op map no
    // longer decodes as a batch operation.
    let request = ResetShapeStatsReq {
        repo: "main".to_string(),
    };
    let expected = json!({
        "reset_shape_stats": true,
        "repo": "main",
    });
    assert_eq!(serde_json::to_value(&request).expect("serialize"), expected);
    let bytes = rmp_serde::to_vec_named(&request).expect("encode");
    assert_eq!(
        rmp_serde::from_slice::<Value>(&bytes).expect("wire map"),
        expected
    );
    let decoded: ResetShapeStatsReq = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, request);
}

#[test]
fn reset_shape_stats_req_rejects_unknown_fields_and_missing_repo() {
    let bytes = rmp_serde::to_vec_named(&json!({
        "reset_shape_stats": true,
        "repo": "main",
        "limit": 1,
    }))
    .expect("encode");
    let result: Result<ResetShapeStatsReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "unknown fields must be denied");

    let bytes = rmp_serde::to_vec_named(&json!({
        "reset_shape_stats": true,
    }))
    .expect("encode");
    let result: Result<ResetShapeStatsReq, _> = rmp_serde::from_slice(&bytes);
    assert!(result.is_err(), "missing repo must be rejected");
}

#[test]
fn shape_stats_page_msgpack_round_trips() {
    // mutant: dropping a row field (e.g. `errors`) or defaulting `tracked`.
    let page = sample_page();
    let bytes = rmp_serde::to_vec_named(&page).expect("encode");
    let decoded: ShapeStatsPage = rmp_serde::from_slice(&bytes).expect("decode");
    assert_eq!(decoded, page);
}

#[test]
fn shape_stats_page_serializes_rows_and_other_fold() {
    // mutant: renaming a field or folding `other` into the row list.
    let value = serde_json::to_value(sample_page()).expect("serialize");
    assert_eq!(
        value,
        json!({
            "rows": [
                {
                    "shape_hex": "0123456789abcdef0123456789abcdef",
                    "executions": 7,
                    "total_nanos": 9000,
                    "errors": 1,
                },
                {
                    "shape_hex": "fedcba9876543210fedcba9876543210",
                    "executions": 2,
                    "total_nanos": 500,
                    "errors": 0,
                },
            ],
            "other": {
                "executions": 3,
                "total_nanos": 120,
                "errors": 1,
            },
            "tracked": 2,
            "truncated": true,
        })
    );
}

#[test]
fn db_response_shape_stats_variants_round_trip() {
    // mutant: a wrong serde tag (`kind` name) or a dropped payload.
    let page = sample_page();
    let response = DbResponse::ShapeStatsPage { page: page.clone() };
    let bytes = rmp_serde::to_vec_named(&response).expect("encode");
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).expect("decode");
    match &decoded {
        DbResponse::ShapeStatsPage { page: decoded_page } => assert_eq!(decoded_page, &page),
        _ => panic!("expected ShapeStatsPage variant"),
    }
    assert_eq!(
        serde_json::to_value(&decoded).expect("serialize"),
        json!({"kind": "shape_stats_page", "page": value_of(page)})
    );

    let reset = DbResponse::ShapeStatsReset;
    let bytes = rmp_serde::to_vec_named(&reset).expect("encode");
    let decoded: DbResponse = rmp_serde::from_slice(&bytes).expect("decode");
    assert!(matches!(decoded, DbResponse::ShapeStatsReset));
    assert_eq!(
        serde_json::to_value(&decoded).expect("serialize"),
        json!({"kind": "shape_stats_reset"})
    );
}

fn value_of(page: ShapeStatsPage) -> Value {
    serde_json::to_value(page).expect("serialize page")
}

#[test]
fn shape_stats_ops_wire_shape_and_classification() {
    // mutant: classifying either op as a write (breaking follower reads), as
    // non-admin (skipping the audit/gate path), or naming them wrongly in
    // the activity view.
    let list = BatchOp::ListShapeStats(full_request());
    assert_eq!(
        serde_json::to_value(&list).expect("encode JSON"),
        json!({"list_shape_stats": true, "repo": "main", "limit": 64})
    );
    assert_eq!(
        serde_json::from_value::<BatchOp>(serde_json::to_value(&list).expect("encode JSON"))
            .expect("decode JSON"),
        list
    );
    assert!(list.is_admin());
    assert!(!list.is_write());
    assert_eq!(list.table_ref(), None);
    assert_eq!(list.required_access("main"), None);
    assert_eq!(list.activity_name(), "admin:list_shape_stats");

    let reset = BatchOp::ResetShapeStats(ResetShapeStatsReq {
        repo: "main".to_string(),
    });
    assert_eq!(
        serde_json::to_value(&reset).expect("encode JSON"),
        json!({"reset_shape_stats": true, "repo": "main"})
    );
    assert_eq!(
        serde_json::from_value::<BatchOp>(serde_json::to_value(&reset).expect("encode JSON"))
            .expect("decode JSON"),
        reset
    );
    assert!(reset.is_admin());
    assert!(!reset.is_write());
    assert_eq!(reset.table_ref(), None);
    assert_eq!(reset.required_access("main"), None);
    assert_eq!(reset.activity_name(), "admin:reset_shape_stats");
}
