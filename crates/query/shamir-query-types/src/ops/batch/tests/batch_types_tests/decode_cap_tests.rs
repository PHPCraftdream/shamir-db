//! RV-COLLECTIONS-DA7D2CD3 — decode-time caps on client-keyed alias
//! collections in `BatchRequest` (`queries`, `interner_epochs`,
//! `return_only`, nested `SubBatchOp.batch`). 1000 entries decode; the 1001st
//! entry is rejected as a serde error (wire code `invalid_request`).

use serde::Serialize;

use shamir_collections::TMap;
use shamir_types::types::value::QueryValue;

use crate::batch::{
    BatchLimits, BatchOp, BatchRequest, IndexVisibility, IsolationLevel, QueryEntry,
    RequestDurability, ResultEncoding, SubBatchOp, VisibilityWait,
};
use crate::read::ReadQuery;
use crate::wire::{DbRequest, CURRENT_QUERY_LANG_VERSION};

fn entry() -> QueryEntry {
    QueryEntry::from(ReadQuery::new("t"))
}

fn queries_map(n: usize) -> TMap<String, QueryEntry> {
    (0..n).map(|i| (format!("a{i}"), entry())).collect()
}

/// Full `BatchRequest` literal with the given `queries` map.
fn batch_with(queries: TMap<String, QueryEntry>) -> BatchRequest {
    BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None::<IsolationLevel>,
        durability: None::<RequestDurability>,
        index_visibility: None::<IndexVisibility>,
        visibility_wait: None::<VisibilityWait>,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: shamir_collections::new_map(),
        result_encoding: ResultEncoding::default(),
    }
}

#[derive(Serialize)]
struct WireQueries<'a> {
    id: QueryValue,
    queries: &'a TMap<String, QueryEntry>,
}

#[derive(Serialize)]
struct WireEpochs<'a> {
    id: QueryValue,
    interner_epochs: &'a TMap<String, u64>,
}

#[derive(Serialize)]
struct WireReturnOnly<'a> {
    id: QueryValue,
    return_only: &'a [String],
}

/// Full wire-required shape for the null-compat test: `queries` is a
/// required field, so the request must carry it even when only the
/// `return_only: null` default is under test.
#[derive(Serialize)]
struct WireReturnOnlyOpt<'a> {
    id: QueryValue,
    queries: &'a TMap<String, QueryEntry>,
    return_only: Option<&'a [String]>,
}

#[derive(Serialize)]
struct NestedWireEntry {
    #[serde(rename = "totally_unknown_op")]
    unknown: i64,
}

#[derive(Serialize)]
struct NestedWireBatch {
    id: QueryValue,
    queries: TMap<String, NestedWireEntry>,
}

#[derive(Serialize)]
struct NestedWireOp {
    batch: NestedWireBatch,
}

#[derive(Serialize)]
struct NestedOuterRequest {
    id: QueryValue,
    queries: TMap<String, NestedWireOp>,
}

fn decode(bytes: &[u8]) -> Result<BatchRequest, String> {
    rmp_serde::from_slice(bytes).map_err(|e| e.to_string())
}

#[test]
fn rv_collections_da7d2cd3_stream_preflight_counts_nested_duplicate_aliases() {
    // Structural raw-wire test: this crate cannot depend on query-builder
    // (query-builder depends on query-types); real builder coverage is in
    // shamir-server's nested_duplicate_preflight_tests module.
    #[derive(Serialize)]
    struct Inner {
        id: QueryValue,
        queries: DupQueries,
    }
    #[derive(Serialize)]
    struct NestedOp {
        batch: Inner,
    }
    #[derive(Serialize)]
    struct Outer {
        id: QueryValue,
        queries: TMap<String, NestedOp>,
    }
    #[derive(Serialize)]
    struct Request {
        op: &'static str,
        db: &'static str,
        batch: Outer,
    }
    fn wire(count: usize) -> Vec<u8> {
        let mut queries = shamir_collections::new_map();
        queries.insert(
            "sub".to_string(),
            NestedOp {
                batch: Inner {
                    id: QueryValue::Null,
                    queries: DupQueries(count),
                },
            },
        );
        rmp_serde::to_vec_named(&Request {
            op: "execute",
            db: "x",
            batch: Outer {
                id: QueryValue::Null,
                queries,
            },
        })
        .unwrap()
    }
    let good = wire(BatchLimits::CEILING.max_queries);
    assert!(crate::validate_request_alias_caps(&good).is_ok());
    let bytes = wire(BatchLimits::CEILING.max_queries + 1);
    let error =
        crate::validate_request_alias_caps(&bytes).expect_err("nested 1001 aliases must fail");
    assert!(error.to_string().contains("decode cap"));
    let truncated = &bytes[..bytes.len() - 1];
    let error =
        crate::validate_request_alias_caps(truncated).expect_err("cap must precede final value");
    assert!(
        error.to_string().contains("decode cap"),
        "unexpected truncation error: {error}"
    );
}

#[test]
fn rv_collections_da7d2cd3_stream_preflight_valid_batch_controls() {
    #[derive(Serialize)]
    struct Request<'a> {
        op: &'static str,
        db: &'static str,
        batch: &'a BatchRequest,
    }
    let mut batch = batch_with(queries_map(1));
    batch.interner_epochs = (0..BatchLimits::CEILING.max_queries)
        .map(|i| (format!("r{i}"), i as u64))
        .collect();
    batch.return_only = Some(
        (0..BatchLimits::CEILING.max_queries)
            .map(|i| format!("a{i}"))
            .collect(),
    );
    let bytes = rmp_serde::to_vec_named(&Request {
        op: "execute",
        db: "x",
        batch: &batch,
    })
    .unwrap();
    assert!(crate::validate_request_alias_caps(&bytes).is_ok());
    let mut null_batch = batch_with(queries_map(1));
    null_batch.return_only = None;
    let bytes = rmp_serde::to_vec_named(&Request {
        op: "execute",
        db: "x",
        batch: &null_batch,
    })
    .unwrap();
    assert!(crate::validate_request_alias_caps(&bytes).is_ok());

    #[derive(Serialize)]
    struct WireNull<'a> {
        id: QueryValue,
        queries: &'a TMap<String, QueryEntry>,
        return_only: Option<&'a [String]>,
    }
    #[derive(Serialize)]
    struct NullRequest<'a> {
        op: &'static str,
        db: &'static str,
        batch: WireNull<'a>,
    }
    let queries = queries_map(1);
    let nil = rmp_serde::to_vec_named(&NullRequest {
        op: "execute",
        db: "x",
        batch: WireNull {
            id: QueryValue::Null,
            queries: &queries,
            return_only: None,
        },
    })
    .unwrap();
    assert!(crate::validate_request_alias_caps(&nil).is_ok());
}

#[test]
fn rv_collections_da7d2cd3_stream_preflight_does_not_cap_arbitrary_record_maps() {
    // Real wire shape of an Insert op (field-name-keyed record values),
    // serialized structurally: the record map holds 1001 distinct field
    // names and must NOT be capped as an alias collection.
    #[derive(Serialize)]
    struct InsertWire {
        insert_into: &'static str,
        values: Vec<TMap<String, i64>>,
    }
    #[derive(Serialize)]
    struct QueryWire {
        id: QueryValue,
        queries: TMap<String, InsertWire>,
    }
    #[derive(Serialize)]
    struct RequestWire {
        op: &'static str,
        query_version: u32,
        db: &'static str,
        batch: QueryWire,
    }
    let record: TMap<String, i64> = (0..BatchLimits::CEILING.max_queries + 1)
        .map(|i| (format!("f{i}"), i as i64))
        .collect();
    let mut queries = shamir_collections::new_map();
    queries.insert(
        "q".to_string(),
        InsertWire {
            insert_into: "t",
            values: vec![record],
        },
    );
    let bytes = rmp_serde::to_vec_named(&RequestWire {
        op: "execute",
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "x",
        batch: QueryWire {
            id: QueryValue::Null,
            queries,
        },
    })
    .unwrap();
    assert!(crate::validate_request_alias_caps(&bytes).is_ok());
    assert!(rmp_serde::from_slice::<DbRequest>(&bytes).is_ok());
}

#[test]
fn rv_collections_da7d2cd3_queries_at_cap_decode() {
    let wire = WireQueries {
        id: QueryValue::Null,
        queries: &queries_map(BatchLimits::CEILING.max_queries),
    };
    let bytes = rmp_serde::to_vec_named(&wire).unwrap();
    let req = decode(&bytes).expect("1000 entries must decode");
    assert_eq!(req.queries.len(), BatchLimits::CEILING.max_queries);
}

#[test]
fn rv_collections_da7d2cd3_queries_over_cap_rejected() {
    let wire = WireQueries {
        id: QueryValue::Null,
        queries: &queries_map(BatchLimits::CEILING.max_queries + 1),
    };
    let bytes = rmp_serde::to_vec_named(&wire).unwrap();
    let err = decode(&bytes).expect_err("1001 entries must be rejected");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}

#[test]
fn rv_collections_da7d2cd3_interner_epochs_over_cap_rejected() {
    let over: TMap<String, u64> = (0..BatchLimits::CEILING.max_queries + 1)
        .map(|i| (format!("r{i}"), i as u64))
        .collect();
    let bytes = rmp_serde::to_vec_named(&WireEpochs {
        id: QueryValue::Null,
        interner_epochs: &over,
    })
    .unwrap();
    let err = decode(&bytes).expect_err("1001 epochs must be rejected");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}

#[test]
fn rv_collections_da7d2cd3_return_only_over_cap_rejected() {
    let aliases: Vec<String> = (0..BatchLimits::CEILING.max_queries + 1)
        .map(|i| format!("a{i}"))
        .collect();
    let bytes = rmp_serde::to_vec_named(&WireReturnOnly {
        id: QueryValue::Null,
        return_only: &aliases,
    })
    .unwrap();
    let err = decode(&bytes).expect_err("1001 return_only entries must be rejected");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}

#[test]
fn rv_collections_da7d2cd3_return_only_null_decodes_to_none() {
    let mut one: TMap<String, QueryEntry> = shamir_collections::new_map();
    one.insert("q1".to_string(), entry());
    let bytes = rmp_serde::to_vec_named(&WireReturnOnlyOpt {
        id: QueryValue::Null,
        queries: &one,
        return_only: None,
    })
    .unwrap();
    let req = decode(&bytes).expect("null return_only must decode");
    assert_eq!(req.return_only, None);
}

/// A wire map with `n` entries ALL sharing the same alias key. A `TMap`
/// cannot hold duplicate keys, so this emits them at the serializer level
/// (`collect_map` writes one entry per pair, duplicates included).
struct DupQueries(usize);

impl Serialize for DupQueries {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map((0..self.0).map(|_| ("dup", entry())))
    }
}

#[derive(Serialize)]
struct DupKeyRequest {
    id: QueryValue,
    queries: DupQueries,
}

#[test]
fn rv_collections_da7d2cd3_duplicate_keys_over_cap_rejected() {
    // 1001 wire entries ALL sharing the alias "dup": the old distinct-len
    // visitor saw map.len() == 1 and accepted the whole payload.
    let bytes = rmp_serde::to_vec_named(&DupKeyRequest {
        id: QueryValue::Null,
        queries: DupQueries(BatchLimits::CEILING.max_queries + 1),
    })
    .unwrap();
    let err = decode(&bytes).expect_err("1001 duplicate-key entries must be rejected");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}

#[test]
fn rv_collections_da7d2cd3_nested_batch_capped_before_typed_decode() {
    // Inner values cannot deserialize as QueryEntry (unresolvable op via
    // flatten), so without the pre-decode nested-cap walk the typed
    // SubBatchOp decode would fail on entry 1 with a NON-cap error.
    let inner: TMap<String, NestedWireEntry> = (0..BatchLimits::CEILING.max_queries + 1)
        .map(|i| (format!("a{i}"), NestedWireEntry { unknown: 1 }))
        .collect();
    let nested = NestedWireBatch {
        id: QueryValue::Null,
        queries: inner,
    };
    let mut outer_queries = shamir_collections::new_map();
    outer_queries.insert("nested".to_string(), NestedWireOp { batch: nested });
    let bytes = rmp_serde::to_vec_named(&NestedOuterRequest {
        id: QueryValue::Null,
        queries: outer_queries,
    })
    .unwrap();
    let err = decode(&bytes).expect_err("nested over-cap batch must fail with the cap error");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}

#[test]
fn rv_collections_da7d2cd3_nested_sub_batch_over_cap_rejected() {
    let inner = batch_with(queries_map(BatchLimits::CEILING.max_queries + 1));
    let sub = QueryEntry {
        op: BatchOp::Batch(SubBatchOp {
            batch: inner,
            bind: shamir_collections::new_map(),
        }),
        return_result: true,
        after: Vec::new(),
        when: None,
    };
    let mut outer: TMap<String, QueryEntry> = shamir_collections::new_map();
    outer.insert("sub".to_string(), sub);
    let bytes = rmp_serde::to_vec_named(&WireQueries {
        id: QueryValue::Null,
        queries: &outer,
    })
    .unwrap();
    let err = decode(&bytes).expect_err("nested over-cap sub-batch must be rejected");
    assert!(err.contains("decode cap"), "unexpected error: {err}");
}
