//! CRIT-5 (#439): per-table read-ACL on Subscribe — ONE unreadable source
//! refuses the WHOLE Subscribe op, so nothing is granted and nothing is
//! delivered.

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

use shamir_server::db_handler::{DbResponse, ShamirDbHandler};
use shamir_server::subscriptions::SubscriptionRegistry;

use super::common::{
    capture, conn_with, decode, encode, execute_built, next_event_for, next_push, settle,
    sub_id_of, unwrap_batch, user_session,
};

// ============================================================================
// 8. CRIT-5 (#439): per-table read-ACL on Subscribe — an unreadable source
//    refuses the WHOLE op: no grant, no subscription id, no delivery.
// ============================================================================
//
// Regression: `BatchOp::Subscribe` carries no `required_access` entry, so a
// user with global read but no read on `secrets` used to receive every live
// insert/update/delete from `secrets`. Every source's table is now classified
// at the ACL seam BEFORE any subscription manager is resolved: a table the
// actor cannot `Read` — or one that does not exist — answers the layer's ONE
// fixed table refusal, `code: "unknown_table"` with the constant text
// `"table is unavailable"`. The refusal REPLACES the old "denied sources are
// silently excluded from delivery" behaviour: the whole operation is refused,
// so a mixed allowed+denied op cannot be partially honoured either.
//
// Two consequences the tests below pin down:
//   * the server answers a top-level `DbResponse::Error` (not a
//     `DbResponse::Batch`): the runner never builds the grant map / sub id,
//     and the subscribe handler's post-processing only injects `sub` into an
//     existing `results[alias].value`, so no bridge is spawned; and
//   * the refusal is byte-identical to an absent table's — it never echoes
//     the table name and never says `access denied`, so it is not an
//     existence oracle.

/// Fixture: one repo with two tables — `messages` (open) and `secrets`
/// (restricted to a DIFFERENT owner). The default user session ("alice")
/// can read `messages` but is denied `secrets`.
async fn make_db_one_repo_split_acl(db: &str, repo: &str) -> Arc<ShamirDb> {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    shamir.create_db(db).await.expect("create_db");
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

/// Assert `resp` is the layer's ONE fixed table refusal — a top-level
/// `DbResponse::Error` with `code == "unknown_table"` and the constant text
/// `"table is unavailable"` — and that it leaks NOTHING about the source:
/// no table name in `forbidden` (no existence oracle) and no `access denied`
/// diagnostic (a read denial must be indistinguishable from an absent table).
fn assert_table_unavailable_refusal(resp: DbResponse, forbidden: &[&str]) {
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "unknown_table",
                "a refused Subscribe source must answer the layer's one table refusal, got {:?}",
                message
            );
            assert!(
                message.contains("table is unavailable"),
                "the refusal must carry the fixed text `table is unavailable`, got {:?}",
                message
            );
            for name in forbidden {
                assert!(
                    !message.contains(name),
                    "the refusal must not echo the table name `{}` (existence oracle): {:?}",
                    name,
                    message
                );
            }
            assert!(
                !message.contains("access denied"),
                "a table-Read denial must not answer `access denied` (existence oracle): {:?}",
                message
            );
        }
        other => panic!(
            "expected the fixed `unknown_table` refusal (DbResponse::Error), got {:?}",
            other
        ),
    }
}

/// A Subscribe whose only source is a table alice cannot `Read` is refused
/// as a WHOLE: the client gets the fixed `unknown_table` refusal instead of a
/// grant, and no frame (not even `Ready`) ever reaches the push sink.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crit5_acl_denied_table_delivers_nothing() {
    let shamir = make_db_one_repo_split_acl("app", "main").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let alice = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Alice subscribes to `secrets` — a table she has NO read access to. The
    // source is classified before any manager is resolved, so the whole op is
    // refused: no grant map, no subscription id, no bridge.
    let sub_op = Subscribe::table(TableRef::with_repo("main", "secrets")).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = decode(
        &handler
            .handle(&alice, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    );
    assert_table_unavailable_refusal(resp, &["secrets"]);
    assert_eq!(
        registry.count(),
        0,
        "a refused Subscribe must not register a subscription"
    );

    // System writes a row into `secrets`. Alice has no bridge, so the push
    // sink must stay COMPLETELY silent — not merely "no Event frames".
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

    let frame = next_push(&mut rx, Duration::from_millis(400)).await;
    assert!(
        frame.is_none(),
        "CRIT-5: a refused Subscribe must deliver NO frame at all, got {:?}",
        frame
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

/// Mixed: one subscription over BOTH `messages` (allowed) and `secrets`
/// (denied). There is NO partial delivery any more — one unavailable source
/// refuses the whole op with the same fixed `unknown_table` refusal, so not
/// even the readable `messages` source is granted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crit5_mixed_subscription_with_a_denied_source_is_refused_whole() {
    let shamir = make_db_one_repo_split_acl("app", "main").await;
    let handler = ShamirDbHandler::new(shamir.clone());
    let alice = user_session();

    let (push, mut rx) = capture();
    let registry = Arc::new(SubscriptionRegistry::new());
    let conn = conn_with(push, registry.clone());

    // Two sources, same repo — this is allowed (single-repo, multi-table).
    // `secrets` is unreadable, which refuses the entire operation.
    let src_msgs = SourceBuilder::table(TableRef::with_repo("main", "messages")).build();
    let src_secrets = SourceBuilder::table(TableRef::with_repo("main", "secrets")).build();
    let sub_op = Subscribe::sources(vec![src_msgs, src_secrets]).build();
    let mut sb = Batch::new();
    sb.id(1);
    sb.subscribe("m", sub_op);
    let resp = decode(
        &handler
            .handle(&alice, &encode(&execute_built("app", sb.build())), &conn)
            .await
            .unwrap(),
    );
    // The refusal must not name EITHER source: it is indistinguishable from a
    // missing table, and it must not single out the one that was denied.
    assert_table_unavailable_refusal(resp, &["messages", "secrets"]);
    assert_eq!(
        registry.count(),
        0,
        "a refused Subscribe must not register a subscription"
    );

    // System inserts into BOTH tables. Since nothing was granted, Alice must
    // not receive ANY frame — the readable `messages` source is not honoured
    // partially either.
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

    let frame = next_push(&mut rx, Duration::from_millis(400)).await;
    assert!(
        frame.is_none(),
        concat!(
            "CRIT-5: a mixed allowed+denied Subscribe must be refused whole ",
            "and deliver NO frame at all, got {:?}",
        ),
        frame
    );
}
