//! Leaf-level wire policy checks over mixed Batch/ForEach trees.

use std::sync::Arc;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{describe_table, drop_table};
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::val::lit;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::DropTableOp;
use shamir_query_types::batch::{BatchOp, BatchRequest};
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex};
use shamir_types::types::value::QueryValue;

use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

fn session(superuser: bool) -> Session {
    Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(superuser, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0; 32],
        0,
    )
}

async fn fixture(mode: NodeMode) -> (Arc<ShamirDb>, ShamirDbHandler) {
    let db = ShamirDb::init_memory().await.expect("init memory database");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
        owner,
    )
    .await
    .expect("add repo");
    let db = Arc::new(db);
    let handler = ShamirDbHandler::new(db.clone()).with_node_mode(mode);
    (db, handler)
}

fn destructive(tag: Option<&str>) -> BatchRequest {
    let mut batch = Batch::new();
    let op = if let Some(tag) = tag {
        drop_table("items")
            .repo("main")
            .hmac(tag)
            .build()
            .expect("build drop table")
    } else {
        // Unsigned on purpose: the builder requires a tag, so strip it.
        match drop_table("items")
            .repo("main")
            .hmac("unused")
            .build()
            .expect("build drop table")
        {
            shamir_query_types::batch::BatchOp::DropTable(mut unsigned) => {
                unsigned.hmac = None;
                shamir_query_types::batch::BatchOp::DropTable(unsigned)
            }
            other => other,
        }
    };
    batch.drop_table("destroy", op);
    batch.build()
}

fn nested(inner: BatchRequest, levels: usize) -> BatchRequest {
    let mut inner = inner;
    for level in 0..levels {
        let mut outer = Batch::new();
        if level % 2 == 0 {
            // Exactly one iteration: a valid tag must cause real execution,
            // not merely acceptance of an empty loop.
            outer.for_each("loop", vec![lit(1)], "row", inner);
        } else {
            outer.sub_batch_no_bind("body", inner);
        }
        inner = outer.build();
    }
    inner
}

/// Executing nested Batch/ForEach levels builds deep async frames that exceed the 2 MiB
/// test-thread stack in debug builds, so every test body runs on a roomy thread.
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn big-stack test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

async fn execute(handler: &ShamirDbHandler, session: &Session, batch: BatchRequest) -> DbResponse {
    handler
        .execute(
            session,
            CURRENT_QUERY_LANG_VERSION,
            "app",
            batch,
            &ConnectionServices::without_push(0),
        )
        .await
}

fn error(response: DbResponse, expected: &str, alias: &str) {
    match response {
        DbResponse::Error { code, message } => {
            assert_eq!(code, expected, "{message}");
            // Quoted, so a container message that merely says "is a write" cannot match.
            assert!(
                message.contains(&format!("'{alias}'")),
                "missing leaf alias: {message}"
            );
        }
        other => panic!("expected {expected}, got {other:?}"),
    }
}

fn accepted(response: DbResponse, alias: &str, leaves: &[&str]) {
    match response {
        DbResponse::Batch { response } => {
            let result = response.results.get(alias).expect("returned root result");
            assert!(!result.skipped, "root must execute");
            // Sub-batches serialize their result maps into value; ForEach
            // serializes one such map per iteration. Inspect those actual
            // shapes, not just the outer container's skipped flag.
            let mut pending = vec![result.value.as_ref().expect("nested result value")];
            let mut found = vec![false; leaves.len()];
            while let Some(value) = pending.pop() {
                match value {
                    QueryValue::Map(map) => {
                        assert!(!map.contains_key("error"), "nested error: {value:?}");
                        assert_ne!(map.get("skipped"), Some(&QueryValue::Bool(true)));
                        for (index, leaf) in leaves.iter().enumerate() {
                            if let Some(leaf_result) = map.get(*leaf) {
                                let QueryValue::Map(fields) = leaf_result else {
                                    panic!("expected serialized QueryResult: {leaf_result:?}");
                                };
                                assert!(fields.contains_key("records"));
                                found[index] = true;
                            }
                        }
                        pending.extend(map.values());
                    }
                    QueryValue::List(values) => pending.extend(values),
                    _ => {}
                }
            }
            assert!(found.iter().all(|found| *found), "missing nested leaves");
        }
        other => panic!("expected executed batch, got {other:?}"),
    }
}

async fn non_superuser_nested_admin_rejected_at_one_and_three_levels_body() {
    let (db, handler) = fixture(NodeMode::ReadWrite).await;
    let session = session(false);
    for levels in [1, 3] {
        error(
            execute(&handler, &session, nested(destructive(None), levels)).await,
            "permission_denied",
            "destroy",
        );
        assert!(db.get_table("app", "main", "items").await.is_ok());
    }
}

async fn tx_execute_rejects_nested_admin_before_execution_body() {
    let (db, handler) = fixture(NodeMode::ReadWrite).await;
    let session = session(false);
    let handle = match handler
        .tx_begin(&session, CURRENT_QUERY_LANG_VERSION, "app", "main", None)
        .await
    {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("expected open transaction, got {other:?}"),
    };
    for levels in [1, 3] {
        error(
            handler
                .tx_execute(
                    &session,
                    CURRENT_QUERY_LANG_VERSION,
                    "app",
                    handle,
                    nested(destructive(None), levels),
                )
                .await,
            "permission_denied",
            "destroy",
        );
    }
    assert!(db.get_table("app", "main", "items").await.is_ok());
    // Release the real transaction's guard rather than leaving it parked.
    let response = handler.tx_rollback(&session, "app", handle).await;
    assert!(
        matches!(response, DbResponse::TxRolledBack { .. }),
        "{response:?}",
    );
}

async fn superuser_nested_drop_requires_correct_hmac_and_executes_body() {
    let (db, handler) = fixture(NodeMode::ReadWrite).await;
    let session = session(true);
    error(
        execute(&handler, &session, nested(destructive(None), 3)).await,
        "hmac_required",
        "destroy",
    );
    assert!(db.get_table("app", "main", "items").await.is_ok());
    let wrong_op = BatchOp::DropTable(DropTableOp {
        drop_table: "different_table".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    });
    let wrong = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &wrong_op).expect("drop-table canonical form"),
    );
    for tag in ["not-hex", wrong.as_str()] {
        error(
            execute(&handler, &session, nested(destructive(Some(tag)), 3)).await,
            "hmac_mismatch",
            "destroy",
        );
        assert!(db.get_table("app", "main", "items").await.is_ok());
    }
    let op = BatchOp::DropTable(DropTableOp {
        drop_table: "items".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    });
    let tag = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &op).expect("drop-table canonical form"),
    );
    accepted(
        execute(&handler, &session, nested(destructive(Some(&tag)), 3)).await,
        "loop",
        &["destroy"],
    );
    assert!(
        db.get_table("app", "main", "items").await.is_err(),
        "accepted nested drop must actually remove the table",
    );
}

async fn non_superuser_data_only_sub_batch_executes_write_body() {
    let (_, handler) = fixture(NodeMode::ReadWrite).await;
    let session = session(false);
    let mut inner = Batch::new();
    inner.insert("write", insert("items").row(doc! { "id" => "k1" }));
    let mut outer = Batch::new();
    outer.sub_batch_no_bind("body", inner.build());
    accepted(
        execute(&handler, &session, outer.build()).await,
        "body",
        &["write"],
    );
    let mut read = Batch::new();
    read.query("read", Query::from("items"));
    match execute(&handler, &session, read.build()).await {
        DbResponse::Batch { response } => assert_eq!(response.results["read"].records.len(), 1),
        other => panic!("expected persisted nested insert, got {other:?}"),
    }
}

async fn readonly_nested_reads_and_exempt_admin_leaf_are_accepted_body() {
    let (_, handler) = fixture(NodeMode::ReadOnly).await;
    let session = session(false);
    let mut inner = Batch::new();
    inner.query("read", Query::from("items"));
    inner.describe_table("describe", describe_table("items").repo("main"));
    accepted(
        execute(&handler, &session, nested(inner.build(), 3)).await,
        "loop",
        &["read", "describe"],
    );
}

async fn readonly_nested_write_reports_leaf_alias_body() {
    let (_, handler) = fixture(NodeMode::ReadOnly).await;
    let session = session(false);
    let mut inner = Batch::new();
    inner.insert("write", insert("items").row(doc! { "id" => "k1" }));
    error(
        execute(&handler, &session, nested(inner.build(), 3)).await,
        "read_only_replica",
        "write",
    );
    let mut read = Batch::new();
    read.query("read", Query::from("items"));
    match execute(&handler, &session, read.build()).await {
        DbResponse::Batch { response } => assert!(response.results["read"].records.is_empty()),
        other => panic!("expected unchanged table after rejection, got {other:?}"),
    }
}

#[test]
fn non_superuser_nested_admin_rejected_at_one_and_three_levels() {
    on_big_stack(non_superuser_nested_admin_rejected_at_one_and_three_levels_body);
}

#[test]
fn tx_execute_rejects_nested_admin_before_execution() {
    on_big_stack(tx_execute_rejects_nested_admin_before_execution_body);
}

fn foreach_drop(tag: Option<&str>) -> BatchRequest {
    let mut body = Batch::new();
    let op = if let Some(tag) = tag {
        drop_table("items")
            .repo("main")
            .hmac(tag)
            .build()
            .expect("build drop table")
    } else {
        BatchOp::DropTable(DropTableOp {
            drop_table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            cascade: false,
        })
    };
    body.drop_table("destroy", op);
    let mut outer = Batch::new();
    outer.for_each("loop", vec![lit(1)], "row", body.build());
    outer.build()
}

async fn superuser_foreach_drop_requires_unresolved_template_hmac_body() {
    let (db, handler) = fixture(NodeMode::ReadWrite).await;
    let session = session(true);
    error(
        execute(&handler, &session, foreach_drop(None)).await,
        "hmac_required",
        "destroy",
    );
    assert!(db.get_table("app", "main", "items").await.is_ok());

    let wrong_op = BatchOp::DropTable(DropTableOp {
        drop_table: "other".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    });
    let wrong = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &wrong_op).expect("canonical drop table"),
    );
    error(
        execute(&handler, &session, foreach_drop(Some(&wrong))).await,
        "hmac_mismatch",
        "destroy",
    );
    assert!(db.get_table("app", "main", "items").await.is_ok());

    let op = BatchOp::DropTable(DropTableOp {
        drop_table: "items".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    });
    let valid = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &op).expect("canonical drop table"),
    );
    accepted(
        execute(&handler, &session, foreach_drop(Some(&valid))).await,
        "loop",
        &["destroy"],
    );
    assert!(db.get_table("app", "main", "items").await.is_err());
}

#[test]
fn rv_query_types_a3d93402_foreach_template_hmac_checked_before_iteration() {
    on_big_stack(superuser_foreach_drop_requires_unresolved_template_hmac_body);
}

#[test]
fn superuser_nested_drop_requires_correct_hmac_and_executes() {
    on_big_stack(superuser_nested_drop_requires_correct_hmac_and_executes_body);
}

#[test]
fn non_superuser_data_only_sub_batch_executes_write() {
    on_big_stack(non_superuser_data_only_sub_batch_executes_write_body);
}

#[test]
fn readonly_nested_reads_and_exempt_admin_leaf_are_accepted() {
    on_big_stack(readonly_nested_reads_and_exempt_admin_leaf_are_accepted_body);
}

#[test]
fn readonly_nested_write_reports_leaf_alias() {
    on_big_stack(readonly_nested_write_reports_leaf_alias_body);
}
