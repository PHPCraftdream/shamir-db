use std::sync::Arc;

use crate::common::{
    decode, encode, execute_built, expect_batch_ok, expect_error, make_db_with_table, root_session,
    session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::ShamirDbHandler;

// --------------------------------------------------------------------------
// drop_table
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_table_without_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("d", ddl::drop_table("items").repo("main"));
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
async fn drop_table_with_wrong_hmac_rejected() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("items")
            .repo("main")
            .hmac("deadbeef".repeat(8)), // 64 hex chars but bogus
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
async fn drop_table_with_correct_hmac_accepted() {
    let shamir = make_db_with_table("prod", "items").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    let key = session_key(&session);
    let tag = canon::compute_tag_hex(&key, &canon::canonical_drop_table("prod", "main", "items"));

    let mut b = Batch::new();
    b.id(1);
    b.drop_table("d", ddl::drop_table("items").repo("main").hmac(&tag));
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
async fn drop_table_tag_bound_to_target_table() {
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
    let tag_for_items =
        canon::compute_tag_hex(&key, &canon::canonical_drop_table("prod", "main", "items"));

    // Submit drop_table for "other" with the items-tag.
    let mut b = Batch::new();
    b.id(1);
    b.drop_table(
        "d",
        ddl::drop_table("other").repo("main").hmac(&tag_for_items),
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
async fn drop_db_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    shamir.create_db("victim").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_db("d", ddl::drop_db("victim"));
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
async fn drop_db_with_correct_hmac_accepted() {
    // Need a routing db that exists — ShamirDb::execute(db_name, ..)
    // looks up `db_name` BEFORE dispatching the batch. The drop_db op
    // can target any db; we route the batch through `scratch` so the
    // lookup succeeds, and drop `victim` inside the op.
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    shamir.create_db("victim").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(&session_key(&session), &canon::canonical_drop_db("victim"));
    let mut b = Batch::new();
    b.id(1);
    b.drop_db("d", ddl::drop_db("victim").hmac(&tag));
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
async fn drop_index_without_hmac_rejected() {
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
    b.drop_index("d", ddl::drop_index("by_id", "items"));
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
async fn drop_index_with_correct_hmac_accepted() {
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

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_drop_index("prod", "main", "items", "by_id"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.drop_index("d", ddl::drop_index("by_id", "items").hmac(&tag));
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
// drop_user
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_user_requires_hmac() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_user("d", ddl::drop_user("bob"));
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
