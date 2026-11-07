//! Behavioral tests for the `IdEncodingWithRefs` guard in
//! `execute_with_touch` (RV-CLIENT-5212ED16 follow-up, review FIX 1): a
//! caller who EXPLICITLY set `result_encoding = Id` on a ref-bearing batch
//! must be rejected BEFORE the batch is sent, while Id-without-refs and
//! refs-with-Name still flow unchanged.
//!
//! These run the REAL `Client` over `Client::for_test` duplex pipes against
//! an emulated server, so the wire frames actually produced are asserted.
//!
//! REVERT ORACLE (guard tests): reverting ONLY the new guard in
//! `execute_with_touch` (returning to `if !batch_has_refs { … = Id }`
//! silently skipping) makes `rv_client_5212ed16_id_encoding_with_refs_is_rejected_before_send`
//! fail — the server then receives bytes (non-empty `read_to_end`) instead of
//! zero bytes and the client returns Ok instead of
//! `ClientError::IdEncodingWithRefs` — while the other tests still pass.
//! Reverting or moving the guard BELOW the touch loop makes the new
//! write-batch test fail (touch bytes appear, or the typed error is lost).

use shamir_collections::{new_map, TMap};
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_types::batch::{BatchOp, BatchRequest, BatchResponse, QueryEntry, ResultEncoding};
use shamir_query_types::filter::{Filter, FilterValue};
use shamir_query_types::read::{QueryResult, ReadQuery, Select, Temporal};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_types::types::value::QueryValue;
use std::time::Duration;
use tokio::io::{duplex, split, AsyncReadExt};

/// No-hang revert oracle: the client-side `execute_with_touch` calls in the
/// negative tests are wrapped in this budget so a revert that makes the
/// client await a reply forever fails with a timeout instead of hanging CI.
const SEND_BUDGET: Duration = Duration::from_secs(30);

use crate::client::LifecycleHooks;
use crate::Client;

fn read_op(field: &str, value: FilterValue) -> BatchOp {
    BatchOp::Read(ReadQuery {
        from: shamir_query_types::TableRef::new("t"),
        select: Select::all(),
        r#where: Some(Filter::Eq {
            field: vec![field.to_string()],
            value,
        }),
        group_by: None,
        order_by: None,
        pagination: Default::default(),
        count_total: false,
        temporal: Temporal::Latest,
        with_version: false,
        explain: false,
    })
}

/// Read-only batch with a single aliased query under `result_encoding`.
fn read_batch(encoding: ResultEncoding, where_value: FilterValue) -> BatchRequest {
    let mut queries: TMap<String, QueryEntry> = TMap::default();
    queries.insert(
        "q".to_string(),
        QueryEntry {
            op: read_op("name", where_value),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        result_encoding: encoding,
        interner_epochs: TMap::default(),
    }
}

/// Build a `Client` over duplex halves; returns the client plus the
/// test-held server-side halves.
fn client_over_duplex() -> (
    Client,
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    tokio::io::ReadHalf<tokio::io::DuplexStream>,
) {
    let (c2s_client_side, c2s_server_side) = duplex(64 * 1024);
    let (s2c_client_side, s2c_server_side) = duplex(64 * 1024);
    let (client_reader, _unused_s2c_write) = split(s2c_client_side);
    let (_unused_c2s_read, client_writer) = split(c2s_client_side);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());
    (client, c2s_server_side, s2c_server_side, _unused_c2s_read)
}

/// Craft a minimal valid empty `BatchResponse` the client parser accepts.
fn empty_batch_response(alias: &str) -> BatchResponse {
    let mut results = new_map();
    results.insert(alias.to_string(), QueryResult::default());
    BatchResponse {
        id: QueryValue::Null,
        results,
        execution_plan: Vec::new(),
        edge_provenance: new_map(),
        execution_time_us: 0,
        transaction: None,
        interner_delta: new_map(),
    }
}

/// RV-CLIENT-5212ED16 (guard): `execute_with_touch` on a v2 client with an
/// EXPLICITLY id-encoded, ref-bearing READ-only batch must return
/// `ClientError::IdEncodingWithRefs` naming the alias WITHOUT sending
/// anything — the emulated server observes EOF with zero frames received
/// (read-only ⇒ no touch roundtrip, so nothing else could have been sent).
///
/// REVERT ORACLE: reverting ONLY the `IdEncodingWithRefs` guard makes this
/// fail — the batch is sent unchanged, so the server's read yields a frame
/// (not EOF) and the client call returns Ok instead of the typed error.
#[tokio::test]
async fn rv_client_5212ed16_id_encoding_with_refs_is_rejected_before_send() {
    let (client, mut c2s, mut _s2c, _unused_c2s_read) = client_over_duplex();
    client.set_server_query_version_for_test(2);

    let server = tokio::spawn(async move {
        // ZERO-BYTE ORACLE: nothing may arrive — the guard fires BEFORE the send.
        let mut rest = Vec::new();
        c2s.read_to_end(&mut rest).await.expect("read to eof");
        assert!(
            rest.is_empty(),
            "server must receive ZERO bytes, got {rest:?} — a ref-bearing Id-encoded batch must never be sent"
        );
    });

    let batch = read_batch(
        ResultEncoding::Id,
        FilterValue::QueryRef {
            alias: "dep".into(),
            path: None,
        },
    );
    // No-hang revert oracle: without the guard the client awaits a reply forever.
    let sent = tokio::time::timeout(SEND_BUDGET, client.execute_with_touch("db", batch)).await;
    let err = sent
        .expect("execute_with_touch must not hang")
        .expect_err("ref-bearing Id encoding must be rejected");
    match err {
        crate::error::ClientError::IdEncodingWithRefs { alias } => {
            assert_eq!(alias, "q", "error must name the ref-bearing alias");
        }
        other => panic!("expected IdEncodingWithRefs, got {other:?}"),
    }

    drop(client);
    drop(_unused_c2s_read);
    server.await.expect("emulated server task panicked");
}

/// Id encoding WITHOUT refs flows unchanged: the emulated server decodes the
/// Execute request and asserts `result_encoding == Id`; the client call
/// succeeds on the empty BatchResponse reply.
///
/// REVERT ORACLE: unaffected by the guard revert (must keep passing).
#[tokio::test]
async fn rv_client_5212ed16_id_encoding_without_refs_is_sent_unchanged() {
    let (client, mut c2s, mut s2c, _unused_c2s_read) = client_over_duplex();
    client.set_server_query_version_for_test(2);

    let server = tokio::spawn(async move {
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("execute frame");
        let env = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
        let req: DbRequest = rmp_serde::from_slice(&env.req).expect("decode DbRequest");
        let DbRequest::Execute { batch, .. } = &req else {
            panic!("emulated server: unexpected request {req:?}");
        };
        assert_eq!(
            batch.result_encoding,
            ResultEncoding::Id,
            "caller-supplied Id encoding without refs must be sent unchanged"
        );
        let response = empty_batch_response("q");
        let payload =
            rmp_serde::to_vec_named(&DbResponse::Batch { response }).expect("encode response");
        let bytes = ResponseEnvelope::ok(env.request_id, payload)
            .to_msgpack()
            .expect("encode envelope");
        write_frame(&mut s2c, &bytes).await.expect("write reply");
    });

    let batch = read_batch(ResultEncoding::Id, FilterValue::String("alice".into()));
    client
        .execute_with_touch("db", batch)
        .await
        .expect("Id-encoded ref-free batch must be sent and succeed");

    drop(client);
    drop(_unused_c2s_read);
    server.await.expect("emulated server task panicked");
}

/// Refs with (caller-supplied) Name encoding stay Name: the guard must NOT
/// touch a Name-encoded batch, and the existing `!batch_has_refs` downgrade
/// must not fire on a ref-bearing one. Server asserts `result_encoding ==
/// Name`; the client call succeeds.
///
/// REVERT ORACLE: unaffected by the guard revert (must keep passing).
#[tokio::test]
async fn rv_client_5212ed16_refs_with_name_encoding_stay_name() {
    let (client, mut c2s, mut s2c, _unused_c2s_read) = client_over_duplex();
    client.set_server_query_version_for_test(2);

    let server = tokio::spawn(async move {
        let frame = read_frame(&mut c2s, MAX_FRAME_SIZE_DEFAULT)
            .await
            .expect("execute frame");
        let env = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
        let req: DbRequest = rmp_serde::from_slice(&env.req).expect("decode DbRequest");
        let DbRequest::Execute { batch, .. } = &req else {
            panic!("emulated server: unexpected request {req:?}");
        };
        assert_eq!(
            batch.result_encoding,
            ResultEncoding::Name,
            "a Name-encoded ref-bearing batch must keep Name on the wire"
        );
        let response = empty_batch_response("q");
        let payload =
            rmp_serde::to_vec_named(&DbResponse::Batch { response }).expect("encode response");
        let bytes = ResponseEnvelope::ok(env.request_id, payload)
            .to_msgpack()
            .expect("encode envelope");
        write_frame(&mut s2c, &bytes).await.expect("write reply");
    });

    let batch = read_batch(
        ResultEncoding::Name,
        FilterValue::QueryRef {
            alias: "dep".into(),
            path: None,
        },
    );
    client
        .execute_with_touch("db", batch)
        .await
        .expect("Name-encoded ref-bearing batch must be sent and succeed");

    drop(client);
    drop(_unused_c2s_read);
    server.await.expect("emulated server task panicked");
}

/// RV-CLIENT-5212ED16 (guard PREFLIGHT): a WRITE batch that is (a) explicitly
/// Id-encoded, (b) ref-bearing, and (c) carries UNCACHED field names must be
/// rejected by the guard BEFORE the touch loop runs — the emulated server
/// observes ZERO bytes (no interner-touch request, no execute).
///
/// REVERT ORACLE: reverting the guard, or moving it BELOW the
/// `collect_field_names`/`touch_fields` loop in `execute_with_touch`, makes
/// this fail — touch/execute bytes appear on the wire, or the call does not
/// return `ClientError::IdEncodingWithRefs`.
#[tokio::test]
async fn rv_client_5212ed16_id_refs_write_batch_with_uncached_fields_never_touches() {
    let (client, mut c2s, mut _s2c, _unused_c2s_read) = client_over_duplex();
    client.set_server_query_version_for_test(2);

    let server = tokio::spawn(async move {
        // ZERO-BYTE ORACLE: no touch request and no execute may be sent —
        // the guard is a pure preflight.
        let mut rest = Vec::new();
        c2s.read_to_end(&mut rest).await.expect("read to eof");
        assert!(
            rest.is_empty(),
            "server must receive ZERO bytes, got {rest:?} — no interner touch or execute may be sent before the guard fires"
        );
    });

    // TMap = IndexMap: insertion order is deterministic, so the ref-bearing
    // read inserted FIRST is the alias the guard names.
    let mut queries: TMap<String, QueryEntry> = TMap::default();
    queries.insert(
        "refq".to_string(),
        QueryEntry {
            op: read_op(
                "name",
                FilterValue::QueryRef {
                    alias: "dep".into(),
                    path: None,
                },
            ),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let mut record: TMap<String, QueryValue> = TMap::default();
    record.insert(
        "brand_new_field_a".to_string(),
        QueryValue::Str("x".to_string()),
    );
    record.insert(
        "brand_new_field_b".to_string(),
        QueryValue::Str("y".to_string()),
    );
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(shamir_query_types::write::InsertOp {
                insert_into: shamir_query_types::TableRef::new("t"),
                values: vec![QueryValue::Map(record)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    let batch = BatchRequest {
        id: QueryValue::Null,
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: Default::default(),
        result_encoding: ResultEncoding::Id,
        interner_epochs: TMap::default(),
    };

    // No-hang revert oracle: with the guard moved below the touch loop the
    // client awaits a touch reply forever.
    let sent = tokio::time::timeout(SEND_BUDGET, client.execute_with_touch("db", batch)).await;
    let err = sent
        .expect("execute_with_touch must not hang")
        .expect_err("ref-bearing Id-encoded WRITE batch must be rejected preflight");
    match err {
        crate::error::ClientError::IdEncodingWithRefs { alias } => {
            assert_eq!(
                alias, "refq",
                "error must name the FIRST ref-bearing alias (TMap insertion order)"
            );
        }
        other => panic!("expected IdEncodingWithRefs, got {other:?}"),
    }

    drop(client);
    drop(_unused_c2s_read);
    server.await.expect("emulated server task panicked");
}
