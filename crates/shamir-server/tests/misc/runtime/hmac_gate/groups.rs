use std::sync::Arc;

use crate::acl::make_db_with_user;
use crate::common::{
    decode, encode, execute_built, expect_batch_ok, expect_error, root_session, session_key,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::hmac as canon;
use shamir_server::db_handler::ShamirDbHandler;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_group("g", ddl::create_group("devs"));
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
async fn create_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.create_group("g", ddl::create_group("devs").hmac("deadbeef".repeat(8)));
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.create_group("g", ddl::create_group("devs").hmac(&tag));
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
    assert!(!resp.results["g"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_group(
        "d",
        ddl::drop_group(ddl::GroupRef::Name {
            name: "devs".to_string(),
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
async fn drop_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.drop_group(
        "d",
        ddl::drop_group(ddl::GroupRef::Name {
            name: "devs".to_string(),
        })
        .hmac("deadbeef".repeat(8)),
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    // Seed the group so the drop resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(&session_key(&session), &canon::canonical_drop_group(&group));
    let mut b = Batch::new();
    b.id(1);
    b.drop_group("d", ddl::drop_group(group).hmac(&tag));
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
    assert!(!resp.results["d"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_group_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.rename_group(
        "r",
        ddl::rename_group(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
        ),
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
async fn rename_group_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.rename_group(
        "r",
        ddl::rename_group(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
        )
        .hmac("deadbeef".repeat(8)),
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_group_with_correct_hmac_accepted() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    // Seed the group so the rename resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_rename_group(&group, "engineers"),
    );
    let mut b = Batch::new();
    b.id(1);
    b.rename_group("r", ddl::rename_group(group, "engineers").hmac(&tag));
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
    assert!(!resp.results["r"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_group_member_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "a",
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        ),
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
async fn add_group_member_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.add_group_member(
        "a",
        ddl::add_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        )
        .hmac("deadbeef".repeat(8)),
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn add_group_member_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "bob").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Seed the group so the membership add resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_add_group_member(&group, 42),
    );
    let mut b = Batch::new();
    b.id(1);
    b.add_group_member("a", ddl::add_group_member(group, 42).hmac(&tag));
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
    assert!(!resp.results["a"].records.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_group_member_without_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member(
        "r",
        ddl::remove_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        ),
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
async fn remove_group_member_with_wrong_hmac_rejected() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("scratch").await;
    let handler = ShamirDbHandler::new(Arc::new(shamir));
    let session = root_session();

    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member(
        "r",
        ddl::remove_group_member(
            ddl::GroupRef::Name {
                name: "devs".to_string(),
            },
            42,
        )
        .hmac("deadbeef".repeat(8)),
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_group_member_with_correct_hmac_accepted() {
    let shamir = make_db_with_user("scratch", "bob").await;
    let handler = ShamirDbHandler::new(shamir);
    let session = root_session();

    // Seed the group so the membership removal resolves to a real group.
    let create_tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_create_group("devs"),
    );
    let mut seed = Batch::new();
    seed.id(1);
    seed.create_group("g", ddl::create_group("devs").hmac(&create_tag));
    let seed_req = execute_built("scratch", seed.build());
    decode(
        &handler
            .handle(
                &session,
                &encode(&seed_req),
                &ConnectionServices::without_push(0),
            )
            .await
            .unwrap(),
    );

    let group = ddl::GroupRef::Name {
        name: "devs".to_string(),
    };
    let tag = canon::compute_tag_hex(
        &session_key(&session),
        &canon::canonical_remove_group_member(&group, 42),
    );
    let mut b = Batch::new();
    b.id(1);
    b.remove_group_member("r", ddl::remove_group_member(group, 42).hmac(&tag));
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
    assert!(!resp.results["r"].records.is_empty());
}
