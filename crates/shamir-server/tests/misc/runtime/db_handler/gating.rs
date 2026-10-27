//! Admin DDL gating (superuser vs non-superuser), server-side query-limits
//! cap and query-language version dispatch.

use std::sync::Arc;

use shamir_types::mpack;

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::upsert;
use shamir_query_builder::Query;

use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_server::db_handler::{DbResponse, QueryLimitsCap, ShamirDbHandler};

use crate::common::{
    decode, encode, execute_built, execute_built_with_version, make_db_with_table, root_session,
    user_session,
};

// --------------------------------------------------------------------------
// Admin DDL through the wire — superuser allowed.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_batch_allowed_for_superuser() {
    // Start with the DB present but no `inventory` table yet; create it via
    // a wire-side admin batch.
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db("prod").await.expect("create_db");
    let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory());
    shamir.add_repo("prod", cfg).await.expect("add repo");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id("ddl");
    b.create_table("mk", ddl::create_table("inventory").repo("main"));
    let admin = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&admin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let resp = match res {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch from create_table, got {:?}", other),
    };
    assert!(resp.results.contains_key("mk"), "mk result present");

    // Verify table is usable: insert + read back.
    let mut b = Batch::new();
    b.id("rw");
    b.try_upsert(
        "ins",
        upsert("inventory")
            .key(mpack!({"sku": "X1"}))
            .value(doc! { "sku" => "X1", "stock" => 42 }),
    )
    .unwrap();
    b.query("rd", Query::from("inventory"));
    let rw = execute_built("prod", b.build());
    let res2 = decode(
        &handler
            .handle(&session, &encode(&rw), &ConnectionServices::without_push(0))
            .await
            .unwrap(),
    );
    let resp2 = match res2 {
        DbResponse::Batch { response } => response,
        other => panic!("expected Batch on read, got {:?}", other),
    };
    let rows = &resp2.results.get("rd").expect("rd alias").records;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get_value_str("sku"), Some("X1"));
    assert_eq!(rows[0].get_value_i64("stock"), Some(42));
}

// --------------------------------------------------------------------------
// Admin DDL through the wire — non-superuser denied with typed error.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_batch_denied_for_non_superuser() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session(); // not superuser

    let mut b = Batch::new();
    b.id("ddl");
    b.drop_table(
        "drop",
        // Unsigned on purpose: the builder requires a tag, so strip it.
        match ddl::drop_table("items")
            .repo("main")
            .hmac("unused")
            .build()
            .expect("build drop table")
        {
            shamir_query_types::batch::BatchOp::DropTable(mut op) => {
                op.hmac = None;
                shamir_query_types::batch::BatchOp::DropTable(op)
            }
            other => other,
        },
    );
    let admin = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&admin),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "permission_denied");
            assert!(message.contains("superuser"), "got {:?}", message);
        }
        other => panic!("expected permission_denied, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Server-side query-limits cap — operator's max wins over client payload.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_query_limits_cap_clamps_max_queries() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    // Hard cap at 2 queries per batch — way below the client default of 50.
    let handler = ShamirDbHandler::new(shamir).with_query_limits(QueryLimitsCap {
        max_result_size_bytes: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_queries_per_batch: 2,
        ..QueryLimitsCap::UNLIMITED
    });
    let session = user_session();

    // Client sends 5 queries with the default `BatchLimits` (max_queries=50).
    // Server-side cap clamps to 2 → planner returns TooManyQueries with
    // max=2, not max=50.
    let mut b = Batch::new();
    b.id("v");
    b.query("q1", Query::from("items"));
    b.query("q2", Query::from("items"));
    b.query("q3", Query::from("items"));
    b.query("q4", Query::from("items"));
    b.query("q5", Query::from("items"));
    let req = execute_built("prod", b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "limits", "TooManyQueries → 'limits' code");
            assert!(
                message.contains("max: 2"),
                "error message must surface the SERVER cap (2), not the client default (50); got {:?}",
                message
            );
        }
        other => panic!("expected limits error, got {:?}", other),
    }
}

// --------------------------------------------------------------------------
// Query-language version dispatch — unsupported version → typed error.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_query_version_rejected_before_db_work() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    // 99 is not in `SUPPORTED_QUERY_LANG_VERSIONS`. Expect a typed error
    // BEFORE the batch hits the DB layer.
    let mut b = Batch::new();
    b.id("v");
    b.query("all", Query::from("items"));
    let req = execute_built_with_version("prod", 99, b.build());
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    match res {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "unsupported_query_version");
            assert!(message.contains("99"), "got {:?}", message);
        }
        other => panic!("expected unsupported_query_version, got {:?}", other),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn current_query_version_accepted() {
    let shamir = make_db_with_table("prod", "main", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = user_session();

    let mut b = Batch::new();
    b.id("v");
    b.query("all", Query::from("items"));
    let req = execute_built_with_version(
        "prod",
        shamir_server::version::CURRENT_QUERY_LANG_VERSION,
        b.build(),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    assert!(
        matches!(res, DbResponse::Batch { .. }),
        "current version must be accepted; got {:?}",
        res
    );
}
