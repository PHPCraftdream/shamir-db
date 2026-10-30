use std::error::Error;

use crate::batch::BuildError;
use crate::ddl::CreateIndexBuildError as E;
use crate::query::QueryBuildError;
use crate::response::ResponseError;
use crate::wire::WireError;
use crate::write::BuilderError;

fn check_no_source(error: &dyn Error, message: &str) {
    assert_eq!(error.to_string(), message);
    assert!(error.source().is_none());
}

#[test]
fn build_error_messages_and_sources() {
    let cases = [
        (BuildError::UnknownAlias { alias: "missing".into(), referenced_by: "reader".into() }, "unknown alias 'missing' referenced by 'reader'"),
        (BuildError::SelfReference { alias: "self".into() }, "alias 'self' references itself"),
        (BuildError::AfterPathIgnored { alias: "reader".into(), raw: "item[0]".into() }, "'after' entry 'item[0]' on 'reader' carries a value-path tail, but 'after' is alias-only ordering and never resolves a path; use a bare alias, or a '$query' reference if you need the value"),
        (BuildError::DuplicateAlias { alias: "dup".into() }, "alias 'dup' is already registered in this batch; the first registered operation is kept and the duplicate was discarded"),
        (BuildError::UnknownOrderingTarget { alias: "ghost".into(), operation: "after" }, "'after' received a handle for alias 'ghost', which is not registered in this batch"),
        (BuildError::SerializationFailed { alias: "reader".into(), reason: "codec".into() }, "entry 'reader' could not be validated: codec"),
    ];
    for (error, message) in cases {
        check_no_source(&error, message);
    }
}

#[test]
fn builder_error_messages_and_sources() {
    let cases = [
        (BuilderError::MissingWhereClause, "Delete::build() requires a where clause — call .where_(filter) before .build()"),
        (BuilderError::MissingSetValue, "Update::build() requires a set payload — call .set(doc) before .build()"),
        (BuilderError::MissingKey, "Upsert::build() requires a key — call .key(doc) before .build()"),
        (BuilderError::MissingValue, "Upsert::build() requires a value — call .value(doc) before .build()"),
        (BuilderError::MissingRule, "AddSchemaRuleBuilder::build() requires a rule — call .rule(r) before .build()"),
        (BuilderError::MissingAction, "AlterSubscriptionBuilder::build() requires a terminal action — call .pause() / .resume() / .set_profile(_) before .build()"),
        (BuilderError::FieldValueCodec("bad value".into()), "Doc::set() could not convert the field value to a QueryValue: bad value"),
        (BuilderError::MissingHmac, "this op is HMAC-gated — call .hmac(tag) before .build()"),
    ];
    for (error, message) in cases {
        check_no_source(&error, message);
    }
}

#[test]
fn query_error_messages_and_sources() {
    let cases = [
        (QueryBuildError::HavingWithoutGroupBy, "having() requires a preceding group_by(); a HAVING filter without GROUP BY produces an empty-group query"),
        (QueryBuildError::InvalidPage { page: 7 }, "page() requires a 1-based page number, got 7; the wire Pagination treats page=0 as page=1 (saturating_sub), so 0 is almost always a caller mistake"),
        (QueryBuildError::InvalidPageSize { page_size: 9 }, "page() requires a non-zero page size, got 9; a zero-size page returns no rows"),
    ];
    for (error, message) in cases {
        check_no_source(&error, message);
    }
}

#[test]
fn index_error_messages_and_sources() {
    let cases = [
        (E::UniqueAndSorted, "index cannot be both sorted and unique; call either .unique() or .sorted(), not both — the server (admin_table_index) and the TS builder reject this combination"),
        (E::IncludeWithoutSorted, ".include() is only valid for sorted indexes; call .sorted() before .include(), or drop the .include() call — the server rejects include without sorted"),
        (E::SortedMultiField { field_count: 2 }, "sorted index requires exactly one field, got 2; sorted indexes are single-field scalar columns only (composite TBD) — the server rejects multi-field sorted indexes"),
        (E::EmptyFields, "CREATE INDEX requires at least one field; call .field(...) — the server (admin_table_index) rejects an empty fields list for all index types"),
        (E::UniqueUnsupportedForType { index_type: "vector".into() }, "`unique` is not supported for 'vector' indexes; only btree/hash indexes can be unique — the server (admin_table_index) rejects this combination"),
        (E::SortedUnsupportedForType { index_type: "fts".into() }, "`sorted` is not supported for 'fts' indexes — the server (admin_table_index) rejects this combination"),
        (E::VectorDimRequired, "vector index requires `vector_dim` > 0; call .vector_dim(n) with a positive dimension — the server (admin_table_index) rejects a missing or zero dimension"),
        (E::UnknownVectorMetric { metric: "bad".into() }, "unknown vector_metric 'bad'; expected 'l2', 'dot', or 'cosine' — the server (admin_table_index) rejects unrecognized metric strings"),
        (E::VectorOptionsOnNonVectorIndex, "vector_dim/vector_metric/vector_quantization are only valid for 'vector' indexes — the server (admin_table_index) rejects these options on any non-vector index type"),
        (E::FtsOptionsOnNonFtsIndex, "fts_tokenizer/fts_language are only valid for 'fts' indexes — the server (admin_table_index) rejects these options on any non-fts index type"),
        (E::FunctionalOptionsOnNonFunctionalIndex, "functional_op/functional_args are only valid for 'functional' indexes — the server (admin_table_index) rejects these options on any non-functional index type"),
        (E::IncludeUnsupportedForType { index_type: "vector".into() }, "`include` is not supported for 'vector' indexes; covering fields are only valid for sorted indexes — the server (admin_table_index) rejects this combination"),
        (E::ConflictingBuilderState { method: "hash", field: "unique" }, "the typed constructor .hash() does not read the builder's .unique() state; the parameter(s) passed to .hash() are authoritative, so the prior .unique() call would be silently discarded — drop the .unique() call, or switch to the typed constructor that matches your intent (this is a client-side builder-misuse check, not a server-side rejection)"),
    ];
    for (error, message) in cases {
        check_no_source(&error, message);
    }
}

#[test]
fn response_error_messages_and_source_chain() {
    check_no_source(
        &ResponseError::MissingAlias("users".into()),
        "alias 'users' not found in response results",
    );
    check_no_source(
        &ResponseError::RowOutOfRange {
            alias: "users".into(),
            index: 3,
            len: 2,
        },
        "row index 3 out of range for alias 'users' (len 2)",
    );
    for (source, expected) in [
        (
            WireError::Encode("bad".into()),
            "MessagePack encode failed: bad",
        ),
        (
            WireError::Decode("bad".into()),
            "MessagePack decode failed: bad",
        ),
    ] {
        let error = ResponseError::Deserialize {
            alias: "users".into(),
            source,
        };
        assert_eq!(
            error.to_string(),
            format!("failed to deserialize record for alias 'users': {expected}")
        );
        let source = error.source().expect("wire source");
        assert!(source.downcast_ref::<WireError>().is_some());
        assert_eq!(source.to_string(), expected);
        assert!(source.source().is_none());
    }
}
