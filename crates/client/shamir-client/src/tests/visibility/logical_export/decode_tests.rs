//! The online logical-dump export operations send the TYPED
//! `StartExport` / `ExportStatus` / `CancelExport` batch operations through
//! the real framed duplex and decode the ACTUAL emitted op (envelope
//! placeholder database, scope, options) plus the typed receipts from
//! server-faithful `QueryValue::Map` fixtures (named serde fields, integer
//! counters, snake_case phase/failure strings). Negative arms prove the
//! decode fails CLOSED on a two-record result, a non-direct record, an
//! unknown receipt field — the payload is otherwise a VALID done receipt,
//! so the failure is attributable solely to the unknown field even if
//! `deny_unknown_fields` were ever lifted — and a missing result — the
//! `ClientError::Db` code
//! and message of a definitive rejection are preserved verbatim.

use serde_bytes::ByteBuf;
use std::future::Future;

use shamir_query_builder::ddl::migration;
use shamir_query_types::admin::ExportPhase;
use shamir_query_types::batch::{BatchOp, BatchResponse, QueryEntry};
use shamir_query_types::read::{QueryRecord, QueryResult};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_types::types::value::QueryValue;

use crate::error::ClientError;
use crate::tests::schema_migration::harness::{
    admin_batch_response, client_on, read_request, respond, respond_admin_payload, respond_db_error,
};

/// The node-local envelope placeholder database the client methods are
/// called with — deliberately DIFFERENT from the op's export scope database
/// (the export scope travels flat inside the op, not in the envelope).
const ENVELOPE_DB: &str = "export-envelope-placeholder";

/// Extract the envelope database, alias and typed batch operation of a
/// scripted request (the ACTUAL wire shape the client emitted — nothing
/// hand-written).
fn export_op(request: &DbRequest) -> (String, String, BatchOp) {
    let DbRequest::Execute { db, batch, .. } = request else {
        panic!("expected DbRequest::Execute, got {request:?}")
    };
    assert_eq!(
        batch.queries.len(),
        1,
        "each export call sends exactly one admin operation"
    );
    let (alias, entry): (&String, &QueryEntry) =
        batch.queries.iter().next().expect("single query entry");
    (db.clone(), alias.clone(), entry.op.clone())
}

fn str_value(text: &str) -> QueryValue {
    QueryValue::Str(text.to_string())
}

fn receipt_map(fields: &[(&str, QueryValue)]) -> QueryValue {
    let mut map = shamir_collections::new_map();
    for (name, value) in fields {
        map.insert((*name).to_string(), value.clone());
    }
    QueryValue::Map(map)
}

/// The server-faithful done receipt fixture every negative arm reuses —
/// a FULLY valid `ExportStatusReceipt` map, so any failure a negative
/// fixture induces is attributable ONLY to the fault under test.
fn valid_done_status_payload() -> QueryValue {
    receipt_map(&[
        ("export_id", str_value("exp-1")),
        ("phase", str_value("done")),
        ("sections", QueryValue::Int(12)),
        ("items", QueryValue::Int(340)),
        ("bytes", QueryValue::Int(8192)),
        ("artifact_name", str_value("dump-2026.opaque")),
    ])
}

/// Bound any await in THIS file (client call or harness read) by the
/// file's own 120 s budget: the shared schema-migration harness `bound`
/// inherits that folder's 30 s `BOUND`, which is below the at-least-120 s
/// CI hang-detector budget this file must honor (the Windows runner can
/// be up to 10x slower).
async fn bound<F: Future>(fut: F) -> F::Output {
    tokio::time::timeout(EXPORT_BOUND, fut)
        .await
        .expect("bounded await must resolve, not hang")
}

/// A start call sends the typed `StartExport` op — the envelope database is
/// the node-local placeholder while the export scope travels flat inside
/// the op — and decodes the typed start receipt.
#[tokio::test]
async fn start_export_sends_typed_op_and_decodes_receipt() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the start request");
        let (envelope_db, alias, op) = export_op(&request);
        assert_eq!(envelope_db, ENVELOPE_DB, "envelope db is the placeholder");
        assert_eq!(alias, "start_export");
        let BatchOp::StartExport(start) = op else {
            panic!("expected a typed StartExport op");
        };
        assert_eq!(start.database, "live-db");
        assert_eq!(start.repo.as_deref(), Some("repo-one"));
        assert_eq!(start.table.as_deref(), Some("table-one"));
        assert!(start.schema_only);
        assert!(!start.include_artifacts);
        respond_admin_payload(
            &mut writer,
            rid,
            &alias,
            receipt_map(&[("export_id", str_value("exp-1"))]),
        )
        .await;
    });
    let start = migration::start_export("live-db")
        .repo("repo-one")
        .table("table-one")
        .schema_only(true);
    let receipt = bound(client.start_export(ENVELOPE_DB, start))
        .await
        .expect("start receipt decodes");
    assert_eq!(receipt.export_id, "exp-1");
    bound(peer).await.expect("peer task");
}

/// Status decodes phase, integer counters, the opaque artifact name in the
/// done phase and the typed failure string in the failed phase.
#[tokio::test]
async fn status_decodes_phase_counters_and_failure_strings() {
    let cases = vec![
        (
            vec![
                ("phase", str_value("waiting_for_gate")),
                ("sections", QueryValue::Int(0)),
                ("items", QueryValue::Int(0)),
                ("bytes", QueryValue::Int(0)),
            ],
            ExportPhase::WaitingForGate,
        ),
        (
            vec![
                ("phase", str_value("exporting")),
                ("sections", QueryValue::Int(3)),
                ("items", QueryValue::Int(120)),
                ("bytes", QueryValue::Int(4096)),
            ],
            ExportPhase::Exporting,
        ),
        (
            vec![
                ("phase", str_value("done")),
                ("sections", QueryValue::Int(12)),
                ("items", QueryValue::Int(340)),
                ("bytes", QueryValue::Int(8192)),
                ("artifact_name", str_value("dump-2026.opaque")),
            ],
            ExportPhase::Done,
        ),
        (
            vec![
                ("phase", str_value("failed")),
                ("sections", QueryValue::Int(0)),
                ("items", QueryValue::Int(0)),
                ("bytes", QueryValue::Int(0)),
                ("failure", str_value("timed_out")),
            ],
            ExportPhase::Failed,
        ),
        (
            vec![
                ("phase", str_value("cancelled")),
                ("sections", QueryValue::Int(0)),
                ("items", QueryValue::Int(0)),
                ("bytes", QueryValue::Int(0)),
            ],
            ExportPhase::Cancelled,
        ),
    ];
    for (fields, phase) in cases {
        let mut all = vec![("export_id", str_value("exp-1"))];
        all.extend(fields);
        let payload = receipt_map(&all);
        let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
        let client = client_on(client_reader, client_to_server);
        let peer = tokio::spawn(async move {
            let mut writer = server_writer;
            let mut reader = server_reader;
            let (rid, request) = bound(read_request(&mut reader))
                .await
                .expect("peer reads the status request");
            let (_, alias, op) = export_op(&request);
            assert_eq!(alias, "export_status");
            assert!(matches!(op, BatchOp::ExportStatus(_)), "typed status op");
            respond_admin_payload(&mut writer, rid, &alias, payload).await;
        });
        let receipt = bound(client.export_status(ENVELOPE_DB))
            .await
            .expect("status receipt decodes");
        assert_eq!(receipt.phase, phase);
        if receipt.phase == ExportPhase::Exporting {
            assert_eq!(
                (receipt.sections, receipt.items, receipt.bytes),
                (3, 120, 4096)
            );
        }
        if receipt.phase == ExportPhase::Done {
            assert_eq!(receipt.artifact_name.as_deref(), Some("dump-2026.opaque"));
            assert_eq!(
                (receipt.sections, receipt.items, receipt.bytes),
                (12, 340, 8192)
            );
        }
        if receipt.phase == ExportPhase::Failed {
            assert_eq!(
                receipt.failure,
                Some(shamir_query_types::admin::ExportFailure::TimedOut)
            );
        }
        bound(peer).await.expect("peer task");
    }
}

/// A definitive server rejection keeps the typed `Db` code and message;
/// nothing is retried client-side.
#[tokio::test]
async fn db_error_preserves_typed_code_and_message() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, _request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the start request");
        respond_db_error(&mut writer, rid, "export_in_progress", "a job already runs").await;
    });
    let error = bound(client.start_export(ENVELOPE_DB, migration::start_export("live-db")))
        .await
        .expect_err("definitive rejection");
    match error {
        ClientError::Db { code, message } => {
            assert_eq!(code, "export_in_progress");
            assert_eq!(message, "a job already runs");
        }
        other => panic!("expected the preserved Db error, got {other:?}"),
    }
    bound(peer).await.expect("peer task");
}

/// A result carrying MORE than one record fails CLOSED: the client accepts
/// exactly the single direct record of the server's one-result dispatch
/// convention, nothing wider.
#[tokio::test]
async fn two_record_result_fails_closed() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the status request");
        let (_, alias, _) = export_op(&request);
        let record = QueryRecord::Direct(QueryValue::Map(shamir_collections::new_map()));
        let mut results = shamir_collections::new_map();
        results.insert(
            alias.clone(),
            QueryResult {
                records: vec![record.clone(), record],
                ..QueryResult::default()
            },
        );
        let response = BatchResponse {
            id: QueryValue::Null,
            results,
            execution_plan: Vec::new(),
            edge_provenance: shamir_collections::new_map(),
            execution_time_us: 0,
            transaction: None,
            interner_delta: shamir_collections::new_map(),
        };
        respond(&mut writer, rid, DbResponse::Batch { response }).await;
    });
    let error = bound(client.export_status(ENVELOPE_DB))
        .await
        .expect_err("fail closed");
    let ClientError::Protocol(text) = error else {
        panic!("expected a typed protocol failure");
    };
    assert!(
        text.contains("exactly one record"),
        "the failure names the violated shape: {text}"
    );
    bound(peer).await.expect("peer task");
}

/// A NON-direct record fails CLOSED; the error names the local alias and
/// carries no peer payload.
#[tokio::test]
async fn non_direct_record_fails_closed() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the status request");
        let (_, alias, _) = export_op(&request);
        let mut results = shamir_collections::new_map();
        results.insert(
            alias.clone(),
            QueryResult {
                records: vec![QueryRecord::IdBytes(ByteBuf::from(vec![1, 2, 3]))],
                ..QueryResult::default()
            },
        );
        let response = BatchResponse {
            id: QueryValue::Null,
            results,
            execution_plan: Vec::new(),
            edge_provenance: shamir_collections::new_map(),
            execution_time_us: 0,
            transaction: None,
            interner_delta: shamir_collections::new_map(),
        };
        respond(&mut writer, rid, DbResponse::Batch { response }).await;
    });
    let error = bound(client.export_status(ENVELOPE_DB))
        .await
        .expect_err("fail closed");
    let ClientError::Protocol(text) = error else {
        panic!("expected a typed protocol failure");
    };
    assert!(
        text.contains("not a direct record") && text.contains("export_status"),
        "the failure names the local alias and the shape: {text}"
    );
    bound(peer).await.expect("peer task");
}

/// An unknown field inside an OTHERWISE-VALID done receipt fails the
/// decode (`deny_unknown_fields`): the fixture carries every mandatory
/// field plus the artifact, so the only fault is the extra field — the
/// oracle is not vacuous. No path, secret or future field is silently
/// ignored, and the static failure message does not echo the peer value.
#[tokio::test]
async fn unknown_receipt_field_fails_closed_without_echo() {
    let payload = receipt_map(&[
        ("export_id", str_value("exp-1")),
        ("phase", str_value("done")),
        ("sections", QueryValue::Int(1)),
        ("items", QueryValue::Int(2)),
        ("bytes", QueryValue::Int(3)),
        ("artifact_name", str_value("dump-2026.opaque")),
        ("path", str_value("/srv/private/out")),
    ]);
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, _request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the status request");
        respond_admin_payload(&mut writer, rid, "export_status", payload).await;
    });
    let error = bound(client.export_status(ENVELOPE_DB))
        .await
        .expect_err("fail closed");
    let ClientError::Protocol(text) = error else {
        panic!("expected a typed protocol failure");
    };
    assert!(
        text.contains("decode failed") && !text.contains("/srv/private/out"),
        "static failure, no peer data echoed: {text}"
    );
    bound(peer).await.expect("peer task");
}

/// A response without the operation alias fails CLOSED — the client does
/// not guess which result to decode, so the one-result leniency applies
/// only to the exact alias it sent. The peer answers the one-result
/// dispatch convention faithfully: exactly ONE result, one direct record,
/// a FULLY valid done `ExportStatusReceipt` — but under a WRONG alias.
/// The failure must therefore be the STATIC missing-alias error naming
/// only the local alias, with the valid payload never decoded and never
/// echoed: a decoder that accepted an arbitrary sole alias would decode
/// this fixture and fail the test.
#[tokio::test]
async fn missing_result_fails_closed() {
    const WRONG_ALIAS: &str = "not_the_sent_alias";
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the status request");
        let (_, alias, _) = export_op(&request);
        assert_eq!(alias, "export_status", "the sent alias stays local");
        // One result, one direct record, a fully valid receipt — only the
        // alias is wrong (typed helper, no hand-built query wire).
        let response = admin_batch_response(WRONG_ALIAS, valid_done_status_payload());
        respond(&mut writer, rid, DbResponse::Batch { response }).await;
    });
    let error = bound(client.export_status(ENVELOPE_DB))
        .await
        .expect_err("fail closed");
    let ClientError::Protocol(text) = error else {
        panic!("expected a typed protocol failure");
    };
    // STATIC exact message: the local alias only, no peer value echoed.
    assert_eq!(
        text, "export_status result missing from batch response",
        "the exact static missing-alias error, nothing decoded from the peer: {text}"
    );
    assert!(!text.contains(WRONG_ALIAS));
    assert!(!text.contains("dump-2026.opaque"));
    bound(peer).await.expect("peer task");
}

/// Wait bound for the whole file: at least 120 s (the CI hang-detector
/// budget; the Windows runner can be up to 10x slower). The file-local
/// `bound` helper above wraps every await in it; the lost-start test
/// below adds explicit per-phase timeouts with the same value.
const EXPORT_BOUND: std::time::Duration = std::time::Duration::from_secs(120);

/// A start whose response future is dropped AFTER the request was fully
/// sent is resolved by `export_status` on the SAME session — the client
/// never resends the start: the peer sees exactly the status operation
/// next (no second start) and answers with the job's stable receipt.
#[tokio::test]
async fn dropped_start_response_future_resolves_via_same_session_status() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let (start_seen, start_seen_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (_start_rid, request) = tokio::time::timeout(EXPORT_BOUND, read_request(&mut reader))
            .await
            .expect("bounded start read")
            .expect("peer reads the start request");
        let (envelope_db, alias, _) = export_op(&request);
        assert_eq!(envelope_db, ENVELOPE_DB);
        assert_eq!(alias, "start_export");
        start_seen
            .send(())
            .expect("the caller is still waiting on the start send");
        // The caller drops its start future; the NEXT request on this
        // session must be the status resolution, NOT a resent start.
        let (rid, request) = tokio::time::timeout(EXPORT_BOUND, read_request(&mut reader))
            .await
            .expect("bounded status read")
            .expect("session still usable after the dropped start future");
        let (_, alias, op) = export_op(&request);
        assert_eq!(
            alias, "export_status",
            "exactly status next, never a resend"
        );
        assert!(matches!(op, BatchOp::ExportStatus(_)), "typed status op");
        respond_admin_payload(
            &mut writer,
            rid,
            &alias,
            receipt_map(&[
                ("export_id", str_value("exp-9")),
                ("phase", str_value("exporting")),
                ("sections", QueryValue::Int(4)),
                ("items", QueryValue::Int(5)),
                ("bytes", QueryValue::Int(6)),
            ]),
        )
        .await;
    });
    let start = migration::start_export("live-db");
    // The select (and the peer join below) are bounded by the same
    // CI-hang-detector budget as every other await in this test.
    tokio::time::timeout(EXPORT_BOUND, async {
        // The owning `Box::pin` lives only inside this scope: dropping the
        // scope's binding drops the UNDERLYING start future (a bare
        // `tokio::pin!` would shadow it and dropping the `Pin<&mut>` would
        // leave the future alive).
        let mut start_fut = Box::pin(client.start_export(ENVELOPE_DB, start));
        tokio::select! {
            out = &mut start_fut => {
                panic!("start returned before the deliberate drop: {out:?}")
            }
            signal = start_seen_rx => {
                signal.expect("peer signalled the complete start send");
            }
        }
        // End of scope: the complete request is on the wire and the owning
        // future (task, retry logic and all) is dropped here.
    })
    .await
    .expect("bounded start-send wait");
    let receipt = tokio::time::timeout(EXPORT_BOUND, client.export_status(ENVELOPE_DB))
        .await
        .expect("bounded status call")
        .expect("status resolves the uncertain start");
    assert_eq!(receipt.export_id, "exp-9");
    assert_eq!(receipt.phase, ExportPhase::Exporting);
    assert_eq!((receipt.sections, receipt.items, receipt.bytes), (4, 5, 6));
    assert_eq!(receipt.artifact_name, None);
    assert_eq!(receipt.failure, None);
    tokio::time::timeout(EXPORT_BOUND, peer)
        .await
        .expect("bounded peer join")
        .expect("peer task");
}

/// Cancel sends the typed `CancelExport` op and decodes the typed
/// cancelled receipt.
#[tokio::test]
async fn cancel_sends_typed_op_and_decodes_receipt() {
    let (client_to_server, server_reader) = tokio::io::duplex(64 * 1024);
    let (server_writer, client_reader) = tokio::io::duplex(64 * 1024);
    let client = client_on(client_reader, client_to_server);
    let peer = tokio::spawn(async move {
        let mut writer = server_writer;
        let mut reader = server_reader;
        let (rid, request) = bound(read_request(&mut reader))
            .await
            .expect("peer reads the cancel request");
        let (envelope_db, alias, op) = export_op(&request);
        assert_eq!(envelope_db, ENVELOPE_DB);
        assert_eq!(alias, "cancel_export");
        assert!(matches!(op, BatchOp::CancelExport(_)), "typed cancel op");
        respond_admin_payload(
            &mut writer,
            rid,
            &alias,
            receipt_map(&[("cancelled", QueryValue::Bool(true))]),
        )
        .await;
    });
    let receipt = bound(client.cancel_export(ENVELOPE_DB))
        .await
        .expect("cancel receipt decodes");
    assert!(receipt.cancelled);
    bound(peer).await.expect("peer task");
}
