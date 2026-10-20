//! CRIT-5 (#439): per-table read-ACL on Subscribe — denied tables leak
//! NOTHING to the unauthorized subscriber.

use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::push_envelope::PushKind;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::subscribe::{SourceBuilder, Subscribe};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;
use shamir_query_types::TableRef;

use shamir_server::db_handler::ShamirDbHandler;
use shamir_server::subscriptions::SubscriptionRegistry;

use crate::common::{
    capture, conn_with, decode, encode, execute_built, next_event_for, settle, sub_id_of,
    unwrap_batch, user_session,
};

// ============================================================================
// 8. CRIT-5 (#439): per-table read-ACL on Subscribe — denied table leaks
//    NOTHING to the unauthorized subscriber.
// ============================================================================
//
// Regression: `BatchOp::Subscribe` is classified as `(Read, Global)` at the
// session layer. Before CRIT-5 the bridge never checked per-table read
// access, so a user with global read but no read on `secrets` received every
// live insert/update/delete from `secrets`. The fix authorizes each source's
// table in `bridge_task` before subscribing to the changefeed; denied sources
// are silently excluded from delivery.

/// Fixture: one repo with two tables — `messages` (open) and `secrets`
/// (restricted to a DIFFERENT owner). The default user session ("alice")
/// can read `messages` but is denied `secrets`.
async fn make_db_one_repo_split_acl(db: &str, repo: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await;
    let cfg = RepoConfig::new(repo, BoxRepoFactory::in_memory())
        .add_table(TableConfig::new("messages"))
        .add_table(TableConfig::new("secrets"));
    shamir.add_repo(db, cfg).await.expect("add repo");

    // Open the db + repo ancestors so alice can traverse.
    let open = shamir_types::access::ResourceMeta::open();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::database(db), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&shamir_types::access::ResourcePath::store(db, repo), &open)
        .await
        .unwrap();
    // `messages` — open (alice can read).
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table(db, repo, "messages"),
            &open,
        )
        .await
        .unwrap();
    // `secrets` — private to a DIFFERENT user (not alice). Mode 0o700
    // means only the owner can rwx; alice is neither owner nor in any
    // group, so she is denied Read.
    let restricted = shamir_types::access::ResourceMeta {
        owner: shamir_db::access::Actor::User(999_999), // a different user
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(
            &shamir_types::access::ResourcePath::table(db, repo, "secrets"),
            &restricted,
        )
        .await
        .unwrap();
    Arc::new(shamir)
}

/// A system session — bypasses all ACL checks. Used to WRITE into the
/// restricted `secrets` table (alice cannot write to it).
fn system_session() -> Session {
    let mut perms = SessionPermissions::from_roles(vec!["read_write".into()]);
    perms.is_superuser = true;
    let mut s = Session::new(
        [0xCD; 16],
        "system".into(),
        perms,
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0x22; 32];
    s
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crit5_acl_denied_table_delivers_nothing() {
    let shamir = make_db_one_repo_split_acl("app", "main").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let alice = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Alice subscribes to `secrets` — a table she has NO read access to.
    let sub_op = Subscribe::table(TableRef::with_repo("main", "secrets")).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&alice, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    // System writes a row into `secrets`. Alice must NOT receive any Event.
    let sys = system_session();
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("secrets").row(doc! { "_id" => "leak", "payload" => "TOP_SECRET" }),
    );
    let _ = handler
        .handle(&sys, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let leak = next_event_for(&mut rx, sub_id, Duration::from_millis(400)).await;
    assert!(
        leak.is_none(),
        "CRIT-5: ACL-denied subscriber must NOT receive events from `secrets`, got {:?}",
        leak
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crit5_acl_authorized_table_still_delivers() {
    // Positive control: the ACL fix must NOT break legitimate subscriptions.
    // Alice CAN read `messages` — events must flow as before.
    let shamir = make_db_one_repo_split_acl("app", "main").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let alice = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    let sub_op = Subscribe::table(TableRef::with_repo("main", "messages")).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&alice, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    // Alice inserts into `messages` (she has read+write via open mode).
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins",
        insert("messages").row(doc! { "_id" => "ok", "thread_id" => 1 }),
    );
    let _ = handler
        .handle(&alice, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    let env = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("authorized subscriber must still receive events from `messages`");
    assert_eq!(env.sub, sub_id);
    assert_eq!(env.push, PushKind::Event);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crit5_mixed_subscription_delivers_only_authorized() {
    // Mixed: subscribe to BOTH `messages` (allowed) and `secrets` (denied)
    // in one subscription. Only `messages` events must arrive.
    let shamir = make_db_one_repo_split_acl("app", "main").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let alice = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Two sources, same repo — this is allowed (single-repo, multi-table).
    let src_msgs = SourceBuilder::table(TableRef::with_repo("main", "messages")).build();
    let src_secrets = SourceBuilder::table(TableRef::with_repo("main", "secrets")).build();
    let sub_op = Subscribe::sources(vec![src_msgs, src_secrets]).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = unwrap_batch(decode(
        &handler
            .handle(&alice, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    ));
    let sub_id = sub_id_of(&resp, "m");

    settle().await;

    // System inserts into BOTH tables.
    let sys = system_session();
    let mut wb = Batch::new();
    wb.id(2);
    wb.insert(
        "ins_msgs",
        insert("messages").row(doc! { "_id" => "pub1", "thread_id" => 1 }),
    );
    wb.insert(
        "ins_secrets",
        insert("secrets").row(doc! { "_id" => "priv1", "payload" => "LEAK" }),
    );
    let _ = handler
        .handle(&sys, &encode(&execute_built("app", wb.build())), &conn)
        .await
        .unwrap();

    // Alice should receive the `messages` event...
    let env_ok = next_event_for(&mut rx, sub_id, Duration::from_millis(500))
        .await
        .expect("authorized `messages` event must be delivered");
    assert_eq!(env_ok.sub, sub_id);

    // ...but NOT the `secrets` event. Drain remaining frames for a moment
    // to ensure nothing leaks through.
    let leak = next_event_for(&mut rx, sub_id, Duration::from_millis(300)).await;
    assert!(
        leak.is_none(),
        "CRIT-5: mixed subscription must not deliver events from ACL-denied `secrets`, got {:?}",
        leak
    );
}
