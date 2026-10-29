use std::sync::Arc;

use super::common::{
    decode, encode, execute_built, expect_batch_ok, expect_error, make_db_with_table, root_session,
    session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::{
    CreateFunctionOp, CreateValidatorOp, DropPublicationOp, DropReplicationProfileOp,
    DropSubscriptionOp,
};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::ShamirDbHandler;
// --------------------------------------------------------------------------
// drop_table
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        shamir_query_types::batch::BatchOp::DropTable(shamir_query_types::admin::DropTableOp {
            drop_table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            cascade: false,
        }),
    );
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
    let (code, _msg) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_with_wrong_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("items")
            .repo("main")
            .hmac("deadbeef".repeat(8))
            .build()
            .expect("build HMAC test op"), // 64 hex chars but bogus
    );
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
    let (code, _msg) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_with_correct_hmac_accepted_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let key = session_key(&session);
    let op =
        shamir_query_types::batch::BatchOp::DropTable(shamir_query_types::admin::DropTableOp {
            drop_table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            cascade: false,
        });
    let tag = canon::compute_tag_hex(
        &key,
        &canon::canonical_batch_op("prod", &op).expect("drop-table canonical form"),
    );

    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("items")
            .repo("main")
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
    );
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
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped_table"), Some("items"));
    assert_eq!(rec.get_value_bool("existed"), Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_tag_bound_to_target_table_a3d93402() {
    // A tag computed for table A must not work against table B.
    let shamir = make_db_with_table("prod", "items").await;
    // Also seed a second table inside the same repo.
    {
        let db = shamir.get_db("prod").unwrap();
        db.create_table("main", "other").await.unwrap();
    }
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let key = session_key(&session);
    let op =
        shamir_query_types::batch::BatchOp::DropTable(shamir_query_types::admin::DropTableOp {
            drop_table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            cascade: false,
        });
    let tag_for_items = canon::compute_tag_hex(
        &key,
        &canon::canonical_batch_op("prod", &op).expect("drop-table canonical form"),
    );

    // Submit drop_table for "other" with the items-tag.
    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("other")
            .repo("main")
            .hmac(&tag_for_items)
            .build()
            .expect("build HMAC test op"),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

// --------------------------------------------------------------------------
// drop_db
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_without_hmac_rejected_a3d93402() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    shamir.create_db("victim").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_db(
        "d",
        shamir_query_types::batch::BatchOp::DropDb(shamir_query_types::admin::DropDbOp {
            drop_db: "victim".into(),
            hmac: None,
            cascade: false,
            if_exists: false,
        }),
    );
    let req = execute_built("scratch", b.build());
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_cascade_hmac_binding_rv_query_types_9cde03ef() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    shamir.create_db("victim").await.expect("create_db");
    let cfg = shamir_db::engine::repo::RepoConfig::new(
        "main",
        shamir_db::engine::repo::BoxRepoFactory::in_memory(),
    );
    shamir.add_repo("victim", cfg).await.expect("add repo");
    let shamir = Arc::new(shamir);
    let handler = ShamirDbHandler::new(Arc::clone(&shamir));
    let session = root_session();

    // Sign the complete typed operation with cascade=false; the submitted
    // operation changes that signed field to true.
    let signed_op =
        shamir_query_types::batch::BatchOp::DropDb(shamir_query_types::admin::DropDbOp {
            drop_db: "victim".into(),
            hmac: None,
            cascade: false,
            if_exists: false,
        });
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("scratch", &signed_op).expect("drop-db canonical form"),
    );

    let mut b = Batch::new();
    b.id(1);
    b.drop_db(
        "d",
        ddl::drop_db("victim")
            .cascade()
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
    );
    let req = execute_built("scratch", b.build());
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");

    // Reaching this assertion demonstrates neither the database nor its repo
    // was removed by the rejected request.
    assert!(
        shamir
            .get_db("victim")
            .is_some_and(|db| db.get_repo("main").is_some()),
        "victim database and repo remain",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_rejects_v1_tag_boundary_rv_query_types_f1dc8e9a() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    shamir.create_db("victim").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();
    let typed_op = BatchOp::DropDb(shamir_query_types::admin::DropDbOp {
        drop_db: "victim".into(),
        hmac: None,
        cascade: false,
        if_exists: false,
    });
    let key = session_key(&session);
    let v1_tag = canon::compute_tag_hex(&key, b"drop_db\0victim");
    let mut typed_op = typed_op;
    if let BatchOp::DropDb(op) = &mut typed_op {
        op.hmac = Some(v1_tag);
    }

    let mut batch = Batch::new();
    batch.id(1);
    batch.op("d", typed_op);
    let req = execute_built("scratch", batch.build());
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_db_with_correct_hmac_accepted_a3d93402() {
    // Need a routing db that exists — ShamirDb::execute(db_name, ..)
    // looks up `db_name` BEFORE dispatching the batch. The drop_db op
    // can target any db; we route the batch through `scratch` so the
    // lookup succeeds, and drop `victim` inside the op.
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    shamir.create_db("victim").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let op = shamir_query_types::batch::BatchOp::DropDb(shamir_query_types::admin::DropDbOp {
        drop_db: "victim".into(),
        hmac: None,
        cascade: false,
        if_exists: false,
    });
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("scratch", &op).expect("drop-db canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_db(
        "d",
        ddl::drop_db("victim")
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
    );
    let req = execute_built("scratch", b.build());
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
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped"), Some("victim"));
}

// --------------------------------------------------------------------------
// drop_index
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_index_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Pre-create an index (no HMAC needed for create_index).
    let mut b = Batch::new();
    b.id(0);
    b.create_index("i", ddl::create_index("by_id", "items").field("id"));
    let mk = execute_built("prod", b.build());
    let _ = handler
        .handle(&session, &encode(&mk), &ConnectionServices::without_push(0))
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        shamir_query_types::batch::BatchOp::DropIndex(shamir_query_types::admin::DropIndexOp {
            drop_index: "by_id".into(),
            table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            request_id: None,
        }),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_index_with_correct_hmac_accepted_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(0);
    b.create_index("i", ddl::create_index("by_id", "items").field("id"));
    let mk = execute_built("prod", b.build());
    let _ = handler
        .handle(&session, &encode(&mk), &ConnectionServices::without_push(0))
        .await
        .unwrap();

    let op =
        shamir_query_types::batch::BatchOp::DropIndex(shamir_query_types::admin::DropIndexOp {
            drop_index: "by_id".into(),
            table: "items".into(),
            repo: "main".into(),
            hmac: None,
            if_exists: false,
            request_id: None,
        });
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("prod", &op).expect("drop-index canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_index(
        "d",
        ddl::drop_index("by_id", "items")
            .hmac(&tag)
            .build()
            .expect("build HMAC test op"),
    );
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
    let resp = expect_batch_ok(res);
    let rec = &resp.results["d"].records[0];
    assert_eq!(rec.get_value_str("dropped_index"), Some("by_id"));
    assert_eq!(rec.get_value_bool("existed"), Some(true));
}

// --------------------------------------------------------------------------
// drop_function and drop_validator
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_function_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.drop_function(
        "d",
        shamir_query_types::batch::BatchOp::DropFunction(
            shamir_query_types::admin::DropFunctionOp {
                drop_function: "missing_fn".into(),
                if_exists: true,
                hmac: None,
            },
        ),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_function_with_wrong_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.drop_function(
        "d",
        ddl::drop_function("missing_fn")
            .if_exists()
            .hmac("deadbeef".repeat(8))
            .build()
            .unwrap(),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_function_with_valid_hmac_if_exists_is_noop_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let unsigned = shamir_query_types::batch::BatchOp::DropFunction(
        shamir_query_types::admin::DropFunctionOp {
            drop_function: "missing_fn".into(),
            if_exists: true,
            hmac: None,
        },
    );
    let mut b = Batch::new();
    b.id(0);
    // The builder refuses an unsigned drop; submit the raw op to reach the gate.
    b.op("d", unsigned.clone());
    let rejected = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", b.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(rejected);
    assert_eq!(code, "hmac_required");
    let op = unsigned;
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("prod", &op).expect("drop-function canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_function(
        "d",
        ddl::drop_function("missing_fn")
            .if_exists()
            .hmac(tag)
            .build()
            .unwrap(),
    );
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
    let resp = expect_batch_ok(res);
    assert_eq!(
        resp.results["d"].records[0].get_value_bool("existed"),
        Some(false)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_validator_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.drop_validator(
        "d",
        shamir_query_types::batch::BatchOp::DropValidator(
            shamir_query_types::admin::DropValidatorOp {
                drop_validator: "missing_validator".into(),
                if_exists: true,
                hmac: None,
            },
        ),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_validator_with_wrong_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.drop_validator(
        "d",
        ddl::drop_validator("missing_validator")
            .if_exists()
            .hmac("deadbeef".repeat(8))
            .build()
            .unwrap(),
    );
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_validator_with_valid_hmac_if_exists_is_noop_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let unsigned = shamir_query_types::batch::BatchOp::DropValidator(
        shamir_query_types::admin::DropValidatorOp {
            drop_validator: "missing_validator".into(),
            if_exists: true,
            hmac: None,
        },
    );
    let mut missing = Batch::new();
    missing.id(0);
    // The builder refuses an unsigned drop; submit the raw op to reach the gate.
    missing.op("missing", unsigned.clone());
    let rejected = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", missing.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(rejected);
    assert_eq!(code, "hmac_required");
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_batch_op("prod", &unsigned).expect("drop-validator canonical form"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_validator(
        "d",
        ddl::drop_validator("missing_validator")
            .if_exists()
            .hmac(tag)
            .build()
            .unwrap(),
    );
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
    let resp = expect_batch_ok(res);
    assert_eq!(
        resp.results["d"].records[0].get_value_bool("existed"),
        Some(false)
    );
}

// --------------------------------------------------------------------------
// Replication catalogue drops
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_replication_catalogue_ops_hmac_gate_a3d93402() {
    async fn submit(
        op: BatchOp,
        session: &shamir_connect::server::session::Session,
    ) -> shamir_query_types::wire::DbResponse {
        let shamir = make_db_with_table("prod", "items").await;
        let handler = ShamirDbHandler::new(shamir);
        let mut batch = Batch::new();
        batch.id(1);
        batch.op("d", op);
        let req = execute_built("prod", batch.build());
        decode(
            &handler
                .handle(session, &encode(&req), &ConnectionServices::without_push(0))
                .await
                .unwrap(),
        )
    }

    let session = root_session();
    let cases = [
        (
            "profile",
            BatchOp::DropReplicationProfile(DropReplicationProfileOp {
                drop_replication_profile: "absent_profile".into(),
                hmac: None,
            }),
            0,
        ),
        (
            "publication",
            BatchOp::DropPublication(DropPublicationOp {
                drop_publication: "absent_publication".into(),
                hmac: None,
            }),
            1,
        ),
        (
            "subscription",
            BatchOp::DropSubscription(DropSubscriptionOp {
                drop_subscription: "absent_subscription".into(),
                hmac: None,
            }),
            2,
        ),
    ];
    for (name, op, kind) in cases {
        let missing = match submit(op.clone(), &session).await {
            shamir_query_types::wire::DbResponse::Error { code, .. } => code,
            other => panic!("expected missing-HMAC rejection, got {other:?}"),
        };
        assert_eq!(missing, "hmac_required", "{name}");

        let wrong_op = match op.clone() {
            BatchOp::DropReplicationProfile(mut x) => {
                x.hmac = Some("deadbeef".repeat(8));
                BatchOp::DropReplicationProfile(x)
            }
            BatchOp::DropPublication(mut x) => {
                x.hmac = Some("deadbeef".repeat(8));
                BatchOp::DropPublication(x)
            }
            BatchOp::DropSubscription(mut x) => {
                x.hmac = Some("deadbeef".repeat(8));
                BatchOp::DropSubscription(x)
            }
            _ => unreachable!(),
        };
        let wrong = match submit(wrong_op, &session).await {
            shamir_query_types::wire::DbResponse::Error { code, .. } => code,
            other => panic!("expected wrong-HMAC rejection, got {other:?}"),
        };
        assert_eq!(wrong, "hmac_mismatch", "{name}");

        let tag = canon::compute_tag_hex(
            &session_key(&session),
            &canon::canonical_batch_op("prod", &op).expect("canonical replication drop"),
        );
        let signed = match op {
            BatchOp::DropReplicationProfile(mut x) => {
                x.hmac = Some(tag);
                BatchOp::DropReplicationProfile(x)
            }
            BatchOp::DropPublication(mut x) => {
                x.hmac = Some(tag);
                BatchOp::DropPublication(x)
            }
            BatchOp::DropSubscription(mut x) => {
                x.hmac = Some(tag);
                BatchOp::DropSubscription(x)
            }
            _ => unreachable!(),
        };
        let valid = submit(signed, &session).await;
        let response = expect_batch_ok(valid);
        if kind == 0 || kind == 1 {
            assert_eq!(
                response.results["d"].records[0].get_value_bool("existed"),
                Some(false)
            );
        } else {
            // The absent subscription is still accepted after authentication; its no-op result varies by runtime path.
            assert!(response.results.contains_key("d"));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_user_requires_hmac_a3d93402() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await.expect("create_db");
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_user(
        "d",
        shamir_query_types::batch::BatchOp::DropUser(shamir_query_types::auth::DropUserOp {
            drop_user: "bob".into(),
            hmac: None,
            if_exists: false,
        }),
    );
    let req = execute_built("scratch", b.build());
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
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

// --------------------------------------------------------------------------
// replace function / validator HMAC gates
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_replace_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "f",
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: "fn".into(),
            source: Some("pub fn shamir_call() {}".into()),
            wasm: None,
            replace: true,
            visibility: None,
            security: None,
            secret_grants: vec![],
            net_grants: vec![],
            hmac: None,
        }),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", b.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_function_replace_with_wrong_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "f",
        BatchOp::CreateFunction(CreateFunctionOp {
            create_function: "fn".into(),
            source: Some("pub fn shamir_call() {}".into()),
            wasm: None,
            replace: true,
            visibility: None,
            security: None,
            secret_grants: vec![],
            net_grants: vec![],
            hmac: Some("deadbeef".repeat(8)),
        }),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", b.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_validator_replace_without_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "v",
        BatchOp::CreateValidator(CreateValidatorOp {
            create_validator: "validator".into(),
            source: Some("pub fn shamir_call() {}".into()),
            wasm: None,
            replace: true,
            hmac: None,
        }),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", b.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_required");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_validator_replace_with_wrong_hmac_rejected_a3d93402() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "v",
        BatchOp::CreateValidator(CreateValidatorOp {
            create_validator: "validator".into(),
            source: Some("pub fn shamir_call() {}".into()),
            wasm: None,
            replace: true,
            hmac: Some("deadbeef".repeat(8)),
        }),
    );
    let res = decode(
        &handler
            .handle(
                &session,
                &encode(&execute_built("prod", b.build())),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );
    let (code, _) = expect_error(res);
    assert_eq!(code, "hmac_mismatch");
}
