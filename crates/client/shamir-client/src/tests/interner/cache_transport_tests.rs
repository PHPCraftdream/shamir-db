//! Client-side interner field-map cache over the FAKE transport
//! (`Client::for_test` duplex pipes + an emulated server that decodes and
//! answers real frames): dump retry after a protocol error, touch-response
//! validation, sparse-touch refresh-cursor semantics, per-repo
//! de-intering, and the cold-dump stampede guard.

use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::future::join_all;
use futures::poll;
use shamir_collections::new_map;
use shamir_connect::common::envelope::{RequestEnvelope, ResponseEnvelope};
use shamir_query_builder::batch::Batch;
use shamir_query_types::batch::{BatchOp, BatchResponse};
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_transport_tcp::framing::{read_frame, write_frame, MAX_FRAME_SIZE_DEFAULT};
use shamir_types::mpack;
use shamir_types::types::value::Value;
use tokio::io::{duplex, split};
use tokio::sync::oneshot;

use crate::client::LifecycleHooks;
use crate::Client;

#[tokio::test]
async fn rv_client_4beb304c_real_dump_retry_after_protocol_error() {
    let (c2s_client, mut c2s_server) = duplex(64 * 1024);
    let (mut s2c_server, c2s_client_reader) = duplex(64 * 1024);
    let (client_reader, _server_write) = split(c2s_client_reader);
    let (_client_read, client_writer) = split(c2s_client);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let frame = read_frame(&mut c2s_server, MAX_FRAME_SIZE_DEFAULT)
                .await
                .unwrap();
            let env = RequestEnvelope::from_msgpack(&frame).unwrap();
            let req: DbRequest = rmp_serde::from_slice(&env.req).unwrap();
            let DbRequest::Execute { batch, .. } = req else {
                panic!("expected Execute")
            };
            let alias = batch.queries.keys().next().unwrap().clone();
            let mut result = QueryResult::default();
            if attempt == 1 {
                let mut fields = new_map();
                fields.insert("epoch".to_string(), Value::Int(1));
                fields.insert("entries".to_string(), Value::List(vec![]));
                result.records.push(QueryRecord::Direct(Value::Map(fields)));
            }
            let mut results = new_map();
            results.insert(alias, result);
            let response = BatchResponse {
                id: Value::Null,
                results,
                execution_plan: Vec::new(),
                edge_provenance: new_map(),
                execution_time_us: 0,
                transaction: None,
                interner_delta: new_map(),
            };
            let payload = rmp_serde::to_vec_named(&DbResponse::Batch { response }).unwrap();
            let bytes = ResponseEnvelope::ok(env.request_id, payload)
                .to_msgpack()
                .unwrap();
            write_frame(&mut s2c_server, &bytes).await.unwrap();
        }
    });
    assert!(client.dump_repo("ic", "main").await.is_err());
    client
        .dump_repo("ic", "main")
        .await
        .expect("retry dump succeeds");
    assert!(client
        .interner_cache()
        .get_or_create("ic", "main")
        .is_populated());
    drop(client);
    drop(_client_read);
    drop(_server_write);
    server.await.unwrap();
}

#[tokio::test]
async fn rv_client_2b10814d_real_touch_rejects_omitted_name() {
    let (c2s_client, mut c2s_server) = duplex(64 * 1024);
    let (mut s2c_server, c2s_client_reader) = duplex(64 * 1024);
    let (client_reader, _server_write) = split(c2s_client_reader);
    let (_client_read, client_writer) = split(c2s_client);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());
    let server = tokio::spawn(async move {
        let frame = read_frame(&mut c2s_server, MAX_FRAME_SIZE_DEFAULT)
            .await
            .unwrap();
        let env = RequestEnvelope::from_msgpack(&frame).unwrap();
        let mut fields = new_map();
        fields.insert("epoch".to_string(), Value::Int(1));
        fields.insert(
            "mappings".to_string(),
            Value::List(vec![Value::List(vec![
                Value::Str("other".into()),
                Value::Int(9),
            ])]),
        );
        let mut result = QueryResult::default();
        result.records.push(QueryRecord::Direct(Value::Map(fields)));
        let mut results = new_map();
        results.insert("_ic_touch".to_string(), result);
        let response = BatchResponse {
            id: Value::Null,
            results,
            execution_plan: Vec::new(),
            edge_provenance: new_map(),
            execution_time_us: 0,
            transaction: None,
            interner_delta: new_map(),
        };
        let payload = rmp_serde::to_vec_named(&DbResponse::Batch { response }).unwrap();
        let bytes = ResponseEnvelope::ok(env.request_id, payload)
            .to_msgpack()
            .unwrap();
        write_frame(&mut s2c_server, &bytes).await.unwrap();
    });
    let result = client.touch_fields("ic", "main", &["wanted"]).await;
    assert!(matches!(
        result,
        Err(crate::error::ClientError::Protocol(message)) if message.contains("wanted")
    ));
    assert!(client.resolve_field("ic", "main", "other").is_some());
    assert!(client.resolve_field("ic", "main", "wanted").is_none());
    drop(client);
    drop(_client_read);
    drop(_server_write);
    server.await.unwrap();
}

#[tokio::test]
async fn rv_client_3594376f_real_sparse_touch_does_not_advance_refresh_cursor() {
    let (c2s_client, mut c2s_server) = duplex(64 * 1024);
    let (mut s2c_server, c2s_client_reader) = duplex(64 * 1024);
    let (client_reader, _server_write) = split(c2s_client_reader);
    let (_client_read, client_writer) = split(c2s_client);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            let frame = read_frame(&mut c2s_server, MAX_FRAME_SIZE_DEFAULT)
                .await
                .unwrap();
            let env = RequestEnvelope::from_msgpack(&frame).unwrap();
            let req: DbRequest = rmp_serde::from_slice(&env.req).unwrap();
            let DbRequest::Execute { batch, .. } = req else {
                panic!("expected Execute")
            };
            if attempt == 1 {
                let BatchOp::InternerDump(op) = &batch.queries.values().next().unwrap().op else {
                    panic!("refresh should send InternerDump")
                };
                assert_eq!(op.since, Some(0), "refresh uses complete cursor 0");
            }
            let alias = batch.queries.keys().next().unwrap().clone();
            let mut fields = new_map();
            fields.insert("epoch".to_string(), Value::Int(8));
            if attempt == 0 {
                fields.insert(
                    "mappings".to_string(),
                    Value::List(vec![Value::List(vec![
                        Value::Str("sparse".into()),
                        Value::Int(8),
                    ])]),
                );
            } else {
                fields.insert(
                    "entries".to_string(),
                    Value::List(vec![
                        Value::List(vec![Value::Int(1), Value::Str("older".into())]),
                        Value::List(vec![Value::Int(8), Value::Str("sparse".into())]),
                    ]),
                );
            }
            let mut result = QueryResult::default();
            result.records.push(QueryRecord::Direct(Value::Map(fields)));
            let mut results = new_map();
            results.insert(alias, result);
            let response = BatchResponse {
                id: Value::Null,
                results,
                execution_plan: Vec::new(),
                edge_provenance: new_map(),
                execution_time_us: 0,
                transaction: None,
                interner_delta: new_map(),
            };
            let payload = rmp_serde::to_vec_named(&DbResponse::Batch { response }).unwrap();
            let bytes = ResponseEnvelope::ok(env.request_id, payload)
                .to_msgpack()
                .unwrap();
            write_frame(&mut s2c_server, &bytes).await.unwrap();
        }
    });
    client
        .touch_fields("ic", "main", &["sparse"])
        .await
        .unwrap();
    let fm = client.interner_cache().get_or_create("ic", "main");
    assert_eq!(fm.epoch(), 0);
    assert_eq!(fm.observed_epoch(), 8);
    client.refresh_repo("ic", "main").await.unwrap();
    assert_eq!(fm.epoch(), 8);
    assert_eq!(client.resolve_field("ic", "main", "older"), Some(1));
    drop(client);
    drop(_client_read);
    drop(_server_write);
    server.await.unwrap();
}

#[tokio::test]
async fn rv_client_f2bd01c0_real_execute_with_touch_deinterns_by_repo() {
    let (c2s_client, mut c2s_server) = duplex(64 * 1024);
    let (mut s2c_server, c2s_client_reader) = duplex(64 * 1024);
    let (client_reader, _server_write) = split(c2s_client_reader);
    let (_client_read, client_writer) = split(c2s_client);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());
    client
        .interner_cache()
        .get_or_create("ic", "repo_a")
        .insert_entry("alpha", 7);
    client
        .interner_cache()
        .get_or_create("ic", "repo_b")
        .insert_entry("beta", 7);
    client.set_server_query_version_for_test(2);
    let server = tokio::spawn(async move {
        let frame = read_frame(&mut c2s_server, MAX_FRAME_SIZE_DEFAULT)
            .await
            .unwrap();
        let env = RequestEnvelope::from_msgpack(&frame).unwrap();
        let req: DbRequest = rmp_serde::from_slice(&env.req).unwrap();
        let DbRequest::Execute { batch, .. } = req else {
            panic!("expected Execute")
        };
        assert_eq!(
            batch.result_encoding,
            shamir_query_types::batch::ResultEncoding::Id
        );
        let mut results = new_map();
        for (alias, id_value) in [("a", 1), ("b", 2)] {
            let bytes = shamir_types::codecs::interned::query_value_to_storage_bytes(
                &mpack!({ "7": @(Value::Int(id_value)) }),
                &|name| {
                    if name == "7" {
                        Ok(shamir_types::core::interner::InternerKey::new(7))
                    } else {
                        Err(shamir_types::codecs::CodecError::Encode(
                            "unexpected key".into(),
                        ))
                    }
                },
            )
            .unwrap();
            let mut result = QueryResult::default();
            result
                .records
                .push(shamir_query_types::read::QueryRecord::IdBytes(
                    serde_bytes::ByteBuf::from(bytes.to_vec()),
                ));
            results.insert(alias.to_string(), result);
        }
        let response = BatchResponse {
            id: Value::Null,
            results,
            execution_plan: Vec::new(),
            edge_provenance: new_map(),
            execution_time_us: 0,
            transaction: None,
            interner_delta: new_map(),
        };
        let payload = rmp_serde::to_vec_named(&DbResponse::Batch { response }).unwrap();
        let bytes = ResponseEnvelope::ok(env.request_id, payload)
            .to_msgpack()
            .unwrap();
        write_frame(&mut s2c_server, &bytes).await.unwrap();
    });
    let mut batch = Batch::new();
    batch.insert(
        "a",
        shamir_query_builder::write::Insert::with_repo("repo_a", "items").row(mpack!({"alpha": 1})),
    );
    batch.insert(
        "b",
        shamir_query_builder::write::Insert::with_repo("repo_b", "items").row(mpack!({"beta": 2})),
    );
    let response = client
        .execute_with_touch("ic", batch.build())
        .await
        .unwrap();
    let row_a = response.results.get("a").unwrap().records[0].as_value();
    let row_b = response.results.get("b").unwrap().records[0].as_value();
    assert_eq!(row_a.get("alpha"), Some(&Value::Int(1)));
    assert_eq!(row_b.get("beta"), Some(&Value::Int(2)));
    drop(client);
    drop(_client_read);
    drop(_server_write);
    server.await.unwrap();
}

/// RV-CLIENT-47644B74 (drain/stampede portion): 8 concurrent COLD `dump_repo`
/// callers on a never-dumped repo must coalesce onto the per-`FieldMap`
/// `OnceCell` stampede guard. REAL frame-count oracle: the client runs over
/// `Client::for_test` duplex pipes and an emulated server decodes every
/// client→server frame, counts the `interner_dump` Execute requests and
/// replies with a valid dump response the parser accepts.
///
/// REVERT ORACLE: reverting the OnceCell stampede guard in
/// `FieldMap::ensure_populated` (fetching unconditionally) makes the count 8
/// and this test fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rv_client_47644b74_concurrent_cold_dump_single_fetch() {
    // client→server: the client writes; the emulated server reads.
    let (c2s_client_side, mut c2s_server_side) = duplex(64 * 1024);
    // server→client: the emulated server writes; the client reads.
    let (s2c_client_side, mut s2c_server_side) = duplex(512 * 1024);
    let (client_reader, _unused_s2c_write) = split(s2c_client_side);
    let (_unused_c2s_read, client_writer) = split(c2s_client_side);
    let client = Client::for_test(client_reader, client_writer, LifecycleHooks::default());

    let dumps_seen = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&dumps_seen);
    // Withhold the first reply until every caller has been polled once, so
    // all 8 overlap the in-flight dump (join_all polls sequentially; a reply
    // landing inside the first caller's first poll would make the rest
    // legitimate post-population re-dumps).
    let (release_tx, release_rx) = oneshot::channel::<()>();
    let mut release_rx = Some(release_rx);
    let server = tokio::spawn(async move {
        loop {
            let frame = match read_frame(&mut c2s_server_side, MAX_FRAME_SIZE_DEFAULT).await {
                Ok(f) => f,
                Err(_) => break, // client dropped — connection over
            };
            let env = RequestEnvelope::from_msgpack(&frame).expect("request envelope");
            let req: DbRequest = rmp_serde::from_slice(&env.req).expect("decode DbRequest");
            let DbRequest::Execute { batch, .. } = &req else {
                panic!("emulated server: unexpected request {req:?}");
            };
            let mut dump_alias = None;
            for (alias, entry) in &batch.queries {
                if matches!(entry.op, BatchOp::InternerDump(_)) {
                    dump_alias = Some(alias.clone());
                }
            }
            let Some(alias) = dump_alias else {
                panic!("emulated server: unexpected non-dump request {req:?}");
            };
            seen.fetch_add(1, Ordering::SeqCst);
            if let Some(rx) = release_rx.take() {
                rx.await.expect("release gate dropped");
            }

            // Craft a valid `interner_dump` response the client parser
            // accepts: `{ "epoch": u64, "entries": [[id, name], ...] }`.
            let mut fields = new_map();
            fields.insert("epoch".to_string(), Value::Int(1));
            fields.insert(
                "entries".to_string(),
                Value::List(vec![
                    Value::List(vec![Value::Int(1), Value::Str("sku".into())]),
                    Value::List(vec![Value::Int(2), Value::Str("qty".into())]),
                ]),
            );
            let mut result = QueryResult::default();
            result.records.push(QueryRecord::Direct(Value::Map(fields)));
            let mut results = new_map();
            results.insert(alias, result);
            let response = BatchResponse {
                id: Value::Null,
                results,
                execution_plan: Vec::new(),
                edge_provenance: new_map(),
                execution_time_us: 0,
                transaction: None,
                interner_delta: new_map(),
            };
            let payload =
                rmp_serde::to_vec_named(&DbResponse::Batch { response }).expect("encode response");
            let bytes = ResponseEnvelope::ok(env.request_id, payload)
                .to_msgpack()
                .expect("encode envelope");
            write_frame(&mut s2c_server_side, &bytes)
                .await
                .expect("write reply");
        }
    });

    // 8 concurrent COLD dump_repo callers (futures created BEFORE polling).
    let dumps: Vec<_> = (0..8).map(|_| client.dump_repo("ic", "main")).collect();
    {
        let mut all = pin!(join_all(dumps));
        // One pass: each caller enters ensure_populated while the reply is withheld.
        assert!(poll!(all.as_mut()).is_pending());
        release_tx.send(()).expect("server task alive");
        for r in all.await {
            r.expect("concurrent cold dump must not fail");
        }
    }

    // THE frame-count oracle: exactly ONE dump request reached the server.
    assert_eq!(
        dumps_seen.load(Ordering::SeqCst),
        1,
        "OnceCell stampede guard must coalesce 8 cold dumps into ONE fetch"
    );

    // Convergence: all dumped entries resolve consistently.
    let sku = client
        .resolve_field("ic", "main", "sku")
        .expect("sku resolved from the coalesced dump");
    assert_eq!(client.field_name("ic", "main", sku).as_deref(), Some("sku"));
    let qty = client
        .resolve_field("ic", "main", "qty")
        .expect("qty resolved from the coalesced dump");
    assert_eq!(client.field_name("ic", "main", qty).as_deref(), Some("qty"));

    drop(client);
    // A split duplex only reaches EOF once BOTH halves are gone: release the
    // test-held read half so the emulated server sees the connection end.
    drop(_unused_c2s_read);
    drop(_unused_s2c_write);
    server.await.expect("emulated server task panicked");
}
