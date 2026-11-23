//! Point-read authorization-binding and name-bound tests for
//! `DbRequest::GetRecord` (admin-shell follow-up).
//!
//! Covers the TOCTOU window the point read had when it authorized names,
//! parked on the global response byte budget, and then resolved those names
//! again: a concurrent ACL revocation or a drop+recreate of the table could
//! slip in while the request was parked. Also covers the cold-resolver
//! retention defect (an unregistered table name must never reach
//! `ShamirDb::get_table`) and the caller-name length bound.
//!
//! The park rendezvous is deterministic and sleep/yield free: the test holds
//! the whole byte budget, and the session's single request slot is the
//! observable — the handler runs the synchronous parent resolution,
//! configured-table check and incarnation capture BEFORE taking the slot,
//! then suspends at the budget acquire, so "slot taken" implies
//! "pre-budget resolution finished". The read future is polled
//! directly (not spawned), so nothing can race the mutation.
//!
//! Fixture mirrors `get_record_tests.rs` (in-memory `ShamirDb`, owner alice,
//! repo `main` with table `items`, one seeded row; requests built ONLY
//! through `shamir_query_builder` and dispatched through the real handler;
//! responses decoded as wire types). Every await is bounded by a 120 s
//! timeout (the Windows CI runner may be up to 10x slower).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{create_table, drop_table};
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{DropTableOp, ResourceRef};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex};
use shamir_query_types::read::{QueryRecord, ReadQuery};
use shamir_query_types::wire::{DbRequest, DbResponse};

use shamir_types::types::record_id::RecordId;

use crate::byte_budget::ByteBudget;
use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap};
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const ALICE_SID: [u8; 32] = [0xAA; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// The byte budget the TEST holds for the whole parked window. The handler's
/// `max_result_size_bytes` is deliberately bigger, so its upfront
/// reservation cannot fit while the test holds this slice.
const SMALL_BUDGET: usize = 64;
/// The handler result cap used by the parked fixtures (bigger than
/// `SMALL_BUDGET`, bigger than the seeded record's encoded size).
const PARKED_RESULT_CAP: usize = 1024;
/// Payload value of the seeded control row.
const SEEDED_V: i64 = 42;
/// Payload value of the row seeded in the REPLACEMENT table.
const REPLACEMENT_V: i64 = 99;
/// The fixed-text refusal of a table whose authorized registration changed
/// while the read was parked. Asserted exactly, so a payload can never
/// satisfy it.
const REGISTRATION_CHANGED: &str = "the authorized table registration changed during the request";

fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

/// Superuser-class alice: `session_actor` maps `is_superuser` to
/// `Actor::Admin`, which short-circuits `authorize_access` — used ONLY
/// where the test's subject is name RESOLUTION, not ACL.
fn superuser_alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(true, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = ALICE_SID;
    s
}

fn parked_limits() -> QueryLimitsCap {
    QueryLimitsCap {
        max_result_size_bytes: PARKED_RESULT_CAP,
        max_in_flight_requests_per_session: 1,
        ..QueryLimitsCap::UNLIMITED
    }
}

/// Handler over an in-memory `ShamirDb` with `app.main.items` owned by alice
/// (principal `[0xAB; 16]`), plus one seeded row. Returns the handler and the
/// seeded row's storage `RecordId`.
async fn build_handler(
    byte_budget: ByteBudget,
    query_limits: QueryLimitsCap,
) -> (ShamirDbHandler, RecordId) {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner))
        .await
        .expect("add repo must complete within the 120s bound")
        .expect("add repo");

    let handler = ShamirDbHandler::new(Arc::new(shamir))
        .with_query_limits(query_limits)
        .with_byte_budget(byte_budget);
    let seeded = seed_row(&handler).await;
    (handler, seeded)
}

/// Insert the control row through the ENGINE's real `execute_as` path and
/// return its STORAGE `RecordId` (never a payload field). The insert stays
/// below the fixture's budget/caps: it does not go through the handler.
async fn seed_row(handler: &ShamirDbHandler) -> RecordId {
    let mut b = Batch::new();
    b.insert(
        "seed",
        insert("items").row(doc! { "id" => "k000", "v" => SEEDED_V }),
    );
    let response = tokio::time::timeout(
        CALL_TIMEOUT,
        handler.db().execute_as(Actor::System, "app", &b.build()),
    )
    .await
    .expect("engine insert must complete within the 120s bound")
    .expect("engine insert must succeed");
    let result = response.results.get("seed").expect("insert alias returned");
    match result.records.first() {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    }
}

/// Send a wire request through the REAL `RequestHandler::handle`, bounded by
/// `CALL_TIMEOUT`.
async fn send(handler: &ShamirDbHandler, session: &Session, req: DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let resp_bytes = tokio::time::timeout(
        CALL_TIMEOUT,
        Box::pin(handler.handle(session, &bytes, &conn)),
    )
    .await
    .expect("handle must complete within the 120s bound")
    .expect("handle must not error at the protocol level");
    rmp_serde::from_slice(&resp_bytes).expect("decode response")
}

fn get_record_req(key: RecordId) -> DbRequest {
    get_record("app", "main", "items", key)
}

fn expect_error(resp: DbResponse, code: &str, context: &str) {
    match resp {
        DbResponse::Error { code: got, .. } => assert_eq!(got, code, "{context}"),
        other => panic!("{context}: expected error code '{code}', got {other:?}"),
    }
}

/// Drive the point read until it is parked at the upfront budget reservation
/// with the pre-budget resolution already done. The session's single request
/// slot is the observable: while we can still take it ourselves the child has
/// not reached the slot gate, so poll the child once (it must stay Pending,
/// the budget is held); once we cannot take it, the child has taken the slot
/// after its synchronous resolution/availability code, so the capture is done.
async fn drive_to_capture<F: Future<Output = DbResponse>>(
    read: &mut Pin<Box<F>>,
    session: &Session,
) {
    // The spin bound turns an unexpected liveness bug into a fast, clear
    // failure instead of a hang (a healthy in-memory read advances or parks
    // in a handful of polls).
    let mut polls = 0u32;
    loop {
        if let Some(probe) = session.try_acquire_request_slot(1) {
            drop(probe);
            let polled =
                std::future::poll_fn(|cx| std::task::Poll::Ready(Future::poll(read.as_mut(), cx)))
                    .await;
            assert!(
                polled.is_pending(),
                "the point read must park on the held budget, not complete"
            );
            polls += 1;
            assert!(
                polls <= 100_000,
                "the point read never reached the budget park"
            );
        } else {
            return;
        }
    }
}

/// Chmod `app.main.items` to `mode` as the System actor (the same ACL mutation
/// style the existing point-read tests use).
async fn chmod_items_table(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "table",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        )
        .hmac("get-record-authz-test")
        .build()
        .expect("build chmod table"),
    );
    tokio::time::timeout(
        CALL_TIMEOUT,
        handler
            .db()
            .execute_as(Actor::System, "app", &batch.build()),
    )
    .await
    .expect("admin chmod must complete within the 120s bound")
    .expect("admin chmod must succeed");
}

/// The LIVE registration of `app.main.items`: its `store_id` (as carried by
/// the repository catalogue) and whether the name is registered at all.
///
/// Read through the repository handle the request would resolve, never from
/// the request itself, so the assertions below observe the real state a
/// parked read compares against.
fn items_registration(handler: &ShamirDbHandler) -> (Option<RecordId>, bool) {
    let repo = handler
        .db()
        .get_db("app")
        .and_then(|db| db.get_repo("main"))
        .expect("app.main must be registered");
    let store_id = repo
        .table_config("items")
        .expect("items must be registered")
        .store_id;
    (store_id, repo.has_table("items"))
}

/// Drop `app.main.items` and create a fresh `items` in the same repo, as
/// alice, through the REAL handler dispatch; then seed one row in the
/// REPLACEMENT table with a unique payload. A second handler over the SAME
/// `ShamirDb` is used with an unbounded budget: dispatching the DDL on the
/// bounded handler would park it on the very reservation the test holds (its
/// upfront reserve is `batch.limits.max_result_size`, 10 MB by default), which
/// would deadlock the rendezvous.
///
/// Returns the live `store_id` read BEFORE the drop, the one read back AFTER
/// the create, the unbounded handler (so the caller can drive the control
/// read) and the replacement row's storage `RecordId`. The id assertions are
/// made HERE: a silently no-op DDL would otherwise leave the parked read
/// comparing the table with itself and the test would pass vacuously.
async fn replace_items_table(
    handler: &ShamirDbHandler,
) -> (
    Option<RecordId>,
    Option<RecordId>,
    ShamirDbHandler,
    RecordId,
) {
    let (pre_drop_store_id, _) = items_registration(handler);
    assert!(
        pre_drop_store_id.is_some(),
        "the fixture's table must carry a live store id before the drop"
    );
    // DROP TABLE and CREATE TABLE through the handler require a superuser
    // session. It shares the session id, hence the confirmation-tag key, with
    // the reading alice.
    let session = superuser_alice_session();
    // The builder refuses to build without the confirmation tag, so the
    // canonical form is taken from the unsigned wire op itself.
    let unsigned = BatchOp::DropTable(DropTableOp {
        drop_table: "items".into(),
        repo: "main".into(),
        hmac: None,
        if_exists: false,
        cascade: false,
    });
    let tag = compute_tag_hex(
        &session.hmac_key(),
        &canonical_batch_op("app", &unsigned).expect("drop-table canonical form"),
    );
    let drop_op = drop_table("items")
        .repo("main")
        .hmac(tag)
        .build()
        .expect("build signed drop table");

    let mut drop_batch = Batch::new();
    drop_batch.drop_table("destroy", drop_op);
    let mut create_batch = Batch::new();
    create_batch.create_table("recreate", create_table("items").repo("main"));

    let ddl_handler =
        ShamirDbHandler::new(Arc::clone(handler.db())).with_byte_budget(ByteBudget::unbounded());

    // Two separate dispatches: one batch would leave the drop/create order
    // to the batch executor, and a create-before-drop would merely fail.
    let drop_resp = send(
        &ddl_handler,
        &session,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: drop_batch.build(),
        },
    )
    .await;
    assert!(
        matches!(drop_resp, DbResponse::Batch { .. }),
        "drop through the handler must succeed, got {drop_resp:?}"
    );

    let create_resp = send(
        &ddl_handler,
        &session,
        DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: create_batch.build(),
        },
    )
    .await;
    assert!(
        matches!(create_resp, DbResponse::Batch { .. }),
        "create through the handler must succeed, got {create_resp:?}"
    );

    let (post_create_store_id, has_table) = items_registration(handler);
    assert!(
        post_create_store_id.is_some(),
        "the recreated table must carry a live store id"
    );
    assert!(has_table, "the recreated table must be registered");
    assert_ne!(
        pre_drop_store_id, post_create_store_id,
        "the DDL must actually replace the table registration"
    );

    // Seed one row in the REPLACEMENT table with a UNIQUE payload: the
    // caller's control read returns THIS row, so it can only pass when a
    // NON-admin alice really holds the ACL to read the replacement.
    let mut seed_batch = Batch::new();
    seed_batch.insert(
        "replacement",
        insert("items").row(doc! { "id" => "replacement", "v" => REPLACEMENT_V }),
    );
    let seeded = tokio::time::timeout(
        CALL_TIMEOUT,
        handler
            .db()
            .execute_as(Actor::System, "app", &seed_batch.build()),
    )
    .await
    .expect("the replacement insert must complete within the 120s bound")
    .expect("the replacement insert must succeed");
    let replacement_id = match seeded
        .results
        .get("replacement")
        .expect("insert alias returned")
        .records
        .first()
    {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    };

    (
        pre_drop_store_id,
        post_create_store_id,
        ddl_handler,
        replacement_id,
    )
}

/// Replace the whole `app.main` REGISTRATION with a LEGACY one
/// (`table_config("items").store_id == None`), minting no store id.
///
/// `ShamirDb::add_repo_as` mints a fresh store id for every id-less inline
/// table (`add_repo_as_with_durability_locked` loops over `config.tables` and
/// calls `with_new_store_id`), so it can NEVER produce a legacy live
/// registration. `DbInstance::add_repo` installs the supplied `RepoConfig` as
/// given and is the route that keeps `store_id: None` — which is what makes
/// the parked comparison `None == None` before the identity fix. The
/// `remove_repo` bool is asserted here so a silent no-op removal cannot make
/// the replacement vacuous.
async fn replace_repo_with_legacy_items(handler: &ShamirDbHandler) {
    let removed = tokio::time::timeout(CALL_TIMEOUT, handler.db().remove_repo("app", "main"))
        .await
        .expect("remove_repo must complete within the 120s bound")
        .expect("remove_repo must succeed");
    assert!(removed, "the fixture's `main` repository must be removed");

    let db = handler.db().get_db("app").expect("app database");
    let config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    tokio::time::timeout(CALL_TIMEOUT, db.add_repo(config))
        .await
        .expect("add_repo must complete within the 120s bound")
        .expect("the legacy repository registration must succeed");
}

// ---------------------------------------------------------------------------
// P1#1 — authorization must stay bound to the incarnation that is read.
// ---------------------------------------------------------------------------

/// The read passes the early ACL, parks on the held budget, the table's Read
/// right is revoked, the budget is released: the reply must be the typed
/// `unknown_table` (the SAME code an absent table answers — the table ACL
/// cannot be used as an existence oracle), never a record payload.
///
/// WITHOUT the fix the handler never re-authorizes, so the parked read
/// resolves the table and returns the seeded `DbResponse::Record`.
#[tokio::test]
async fn get_record_refuses_when_table_read_is_revoked_while_parked_on_budget() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());
    assert_ne!(session.activity_handle(), 0, "store-stamped session");

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(&handler, &session, get_record_req(seeded)));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    // Mutate while the read is parked: revoke alice's Table Read.
    chmod_items_table(&handler, 0o000).await;
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    expect_error(
        resp,
        "unknown_table",
        "table Read revoked while parked on the budget",
    );
}

/// The read passes the early ACL and parks on the held budget; the table is
/// then dropped and recreated under the SAME name in the same repo, and a row
/// is seeded in the replacement. The name is configured and the ACL still
/// passes — the CONTROL below reads the replacement's row back as the SAME
/// non-admin alice — so only the captured registration comparison can refuse
/// it, with the fixed typed `access_denied`.
///
/// WITHOUT the identity fix the read resolves the NEW table and returns
/// `not_found` (or the replacement's row once it holds that key), never the
/// fixed refusal.
#[tokio::test]
async fn get_record_refuses_when_the_authorized_table_is_replaced_while_parked() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, _seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(
        &handler,
        &session,
        get_record_req(RecordId([7u8; 16])),
    ));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    let (pre_store_id, post_store_id, ddl_handler, replacement_id) =
        replace_items_table(&handler).await;
    assert_ne!(
        pre_store_id, post_store_id,
        "the parked read must be comparing two DIFFERENT registrations"
    );

    // CONTROL: the same NON-admin alice principal must read the replacement
    // row, so the parked refusal cannot be attributed to the ACL.
    let control = send(
        &ddl_handler,
        &alice_session(),
        get_record_req(replacement_id),
    )
    .await;
    match control {
        DbResponse::Record { record } => assert_eq!(
            record.get_value_i64("v"),
            Some(REPLACEMENT_V),
            "the control must return the replacement's row"
        ),
        other => panic!("control: alice must read the replacement row, got {other:?}"),
    }

    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "authorized table registration replaced while parked"
            );
            assert_eq!(
                message, REGISTRATION_CHANGED,
                "the parked refusal must be the fixed registration text, never a payload"
            );
        }
        other => panic!("expected the fixed access_denied refusal, got {other:?}"),
    }
}

/// P1 (legacy case): a table replaced by ANOTHER **legacy** registration —
/// `table_config("items").store_id` is `None` on BOTH sides — must be refused
/// exactly like an id-bearing replacement.
///
/// The fixture is built with `add_repo_as`, which mints a store id, so the
/// repository is first re-registered through
/// [`replace_repo_with_legacy_items`] (the only route that installs an
/// id-less table as-is). While the read is parked the whole registration is
/// replaced the same legacy way: no store id is ever minted, and the
/// `None`-before / `None`-after assertions below are what make this
/// legacy-vs-legacy.
///
/// WITHOUT the registration identity the comparison is `None == None`, the
/// read proceeds into the replacement repository and returns `not_found`, so
/// this test fails without the fix.
#[tokio::test]
async fn get_record_refuses_when_a_legacy_table_is_replaced_by_a_legacy_registration() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, _seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());

    // Legacy on the AUTHORIZED side too: the parked read must capture `None`.
    replace_repo_with_legacy_items(&handler).await;
    let (authorized_store_id, has_table) = items_registration(&handler);
    assert_eq!(
        authorized_store_id, None,
        "the authorized table must be a LEGACY registration (no store id)"
    );
    assert!(has_table, "the legacy registration must be registered");

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(
        &handler,
        &session,
        get_record_req(RecordId([0x42; 16])),
    ));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");

    replace_repo_with_legacy_items(&handler).await;
    let (replacement_store_id, has_table) = items_registration(&handler);
    assert_eq!(
        replacement_store_id, None,
        "the replacement must stay LEGACY: no store id may be minted"
    );
    assert_eq!(
        replacement_store_id, authorized_store_id,
        "legacy-vs-legacy: the captured store ids both stay `None`, so the \
         store-id comparison alone can never tell the two registrations apart"
    );
    assert!(has_table, "the replacement table must be registered");

    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(
                code, "access_denied",
                "legacy registration replaced while parked"
            );
            assert_eq!(
                message, REGISTRATION_CHANGED,
                "the legacy replacement must use the fixed registration text"
            );
        }
        other => panic!("expected the fixed access_denied refusal, got {other:?}"),
    }
}

/// Control: identical park-and-release with NO mutation returns the seeded
/// record (the parking machinery itself must not refuse anything).
#[tokio::test]
async fn get_record_granted_control_returns_the_seeded_row_after_parking() {
    let budget = ByteBudget::new(Some(SMALL_BUDGET));
    let (handler, seeded) = build_handler(budget.clone(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());

    let held = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(SMALL_BUDGET))
        .await
        .expect("the test must reserve the byte budget within the 120s bound");
    let mut read = Box::pin(send(&handler, &session, get_record_req(seeded)));
    tokio::time::timeout(CALL_TIMEOUT, drive_to_capture(&mut read, &session))
        .await
        .expect("the read must reach the budget park within the 120s bound");
    drop(held);

    let resp = tokio::time::timeout(CALL_TIMEOUT, read)
        .await
        .expect("the released read must complete within the 120s bound");
    match resp {
        DbResponse::Record { record } => {
            assert_eq!(
                record.get_value_i64("v"),
                Some(SEEDED_V),
                "the seeded row's payload must come back"
            );
        }
        other => panic!("expected the seeded Record, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// P1#2 — an unregistered table name must not reach the cold table resolver.
// ---------------------------------------------------------------------------

/// Eight DISTINCT never-created table names, each a typed `unknown_table`,
/// must leave the repository's direct cell map byte-for-byte unchanged:
/// `ShamirDb::get_table` would otherwise insert one catalogue cell per probe
/// before checking configuration.
///
/// WITHOUT the fix these eight requests grow the cell map by eight.
#[tokio::test]
async fn missing_table_names_do_not_grow_the_catalogue_cells() {
    let (handler, _seeded) =
        build_handler(ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = superuser_alice_session();

    let repo = handler
        .db()
        .get_db("app")
        .expect("app database")
        .get_repo("main")
        .expect("main repo");
    let before = repo.table_cell_count();

    for i in 0..8u8 {
        let name = format!("absent_table_{i}");
        let resp = send(
            &handler,
            &session,
            get_record("app", "main", name.as_str(), RecordId([9u8; 16])),
        )
        .await;
        expect_error(resp, "unknown_table", "unregistered table name");
    }

    assert_eq!(
        repo.table_cell_count(),
        before,
        "a refused missing-table probe must not insert a catalogue cell"
    );
}

// ---------------------------------------------------------------------------
// P2#3 — bound every caller-supplied name before it can be echoed/resolved.
// ---------------------------------------------------------------------------

fn expect_invalid_name(resp: DbResponse, context: &str) {
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "invalid_request", "{context}");
            assert_eq!(
                message, "db, repo and table names must be 1..=255 UTF-8 bytes",
                "{context}"
            );
            assert!(
                !message.contains("aaaa"),
                "{context}: the refusal must not echo the caller's name"
            );
        }
        other => panic!("{context}: expected the fixed invalid_request refusal, got {other:?}"),
    }
}

/// A 256-byte `db`, `repo` or `table` name is the fixed `invalid_request`
/// refusal that never echoes the input; a 255-byte table name passes the
/// bound (it then fails later with `unknown_table`, proving the boundary).
///
/// WITHOUT the fix the over-long name is echoed back in a resolution error
/// and reaches the catalogue/resolver instead of being refused up front.
#[tokio::test]
async fn get_record_refuses_over_long_names_without_echoing_them() {
    let (handler, _seeded) =
        build_handler(ByteBudget::unbounded(), QueryLimitsCap::UNLIMITED).await;
    let session = superuser_alice_session();
    let key = RecordId([9u8; 16]);

    let over_long = "a".repeat(256);
    let resp = send(
        &handler,
        &session,
        get_record(over_long.as_str(), "main", "items", key),
    )
    .await;
    expect_invalid_name(resp, "256-byte db name");

    let resp = send(
        &handler,
        &session,
        get_record("app", over_long.as_str(), "items", key),
    )
    .await;
    expect_invalid_name(resp, "256-byte repo name");

    let resp = send(
        &handler,
        &session,
        get_record("app", "main", over_long.as_str(), key),
    )
    .await;
    expect_invalid_name(resp, "256-byte table name");

    // Boundary: 255 bytes is accepted by the name bound and then refused by
    // the configured-table check.
    let at_limit = "a".repeat(255);
    let resp = send(
        &handler,
        &session,
        get_record("app", "main", at_limit.as_str(), key),
    )
    .await;
    expect_error(
        resp,
        "unknown_table",
        "255-byte table name passes the bound",
    );
}

// ---------------------------------------------------------------------------
// Cold-resolver cell retention on the batch-read and cursor-create paths
// (moved here verbatim from the former `unknown_table_cell_growth_tests`
// module, which the layout folder limit no longer allows as its own file).
// ---------------------------------------------------------------------------

// Regression for cold-resolver cell retention in `RepoCore::get_table_ctx`:
// the table cell used to be inserted BEFORE the configured-table check, so
// every request naming a NEVER-created table left an empty `OnceCell` in
// the repository's `tables` map forever — N distinct unknown names grew it
// without bound.
//
// The engine's own tests cannot observe this (`RepoInstance::table_cell_count`
// is `test-util`-gated and `shamir-engine`'s tests do not enable it), so the
// probe lives here, where the dev-dependency does.
//
// Each subject request must reach the configured-table refusal (the message
// carries the engine's "is not configured in this repository" text) and
// leave the cell map unchanged. WITHOUT the pre-check every probe inserts
// one cell, so the count grows by the number of distinct names.
//
// Fixture mirrors the point-read tests above (in-memory `ShamirDb`, owner
// alice, repo `main` with table `items`; requests are builder-produced and
// dispatched through the real handler; every await is bounded by a 120 s
// timeout because the Windows CI runner may be up to 10x slower). No temp
// dirs and no sleeps are involved: the repository is purely in-memory and
// lives for the whole test.

/// Distinct never-created names per subject (the task's minimum is 8).
const UNKNOWN_NAMES: usize = 8;

/// Handler over an in-memory `ShamirDb` with `app.main.items` owned by alice.
/// `items` is CONFIGURED but never opened, so its own cell is not part of the
/// baseline either.
async fn build_growth_handler() -> ShamirDbHandler {
    let shamir = tokio::time::timeout(CALL_TIMEOUT, ShamirDb::init_memory())
        .await
        .expect("init must complete within the 120s bound")
        .expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    tokio::time::timeout(CALL_TIMEOUT, shamir.create_db_as("app", owner.clone()))
        .await
        .expect("create_db must complete within the 120s bound")
        .expect("create_db");
    let cfg =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
    tokio::time::timeout(CALL_TIMEOUT, shamir.add_repo_as("app", cfg, owner))
        .await
        .expect("add repo must complete within the 120s bound")
        .expect("add repo");
    ShamirDbHandler::new(Arc::new(shamir)).with_cursor_limits(CursorLimitsCap::UNLIMITED)
}

/// Eight DISTINCT never-created table names.
fn unknown_names() -> impl Iterator<Item = String> {
    (0..UNKNOWN_NAMES).map(|i| format!("absent_table_{i}"))
}

/// Live cell-map size of the repository the requests resolve against.
fn table_cell_count(handler: &ShamirDbHandler) -> usize {
    handler
        .db()
        .get_db("app")
        .expect("app database")
        .get_repo("main")
        .expect("main repo")
        .table_cell_count()
}

fn batch_read_req(table: &str) -> DbRequest {
    let mut batch = Batch::new();
    batch.query("read", Query::with_repo("main", table));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        batch: batch.build(),
    }
}

fn create_cursor_req(table: &str) -> DbRequest {
    DbRequest::CreateCursor {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".to_string(),
        query: ReadQuery::with_repo("main", table),
        page_size: 1,
    }
}

/// The batch path refuses an unknown table with the canonical, name-hiding
/// `unknown_table` response.
fn expect_batch_unknown_table_refusal(resp: DbResponse, table: &str) {
    match resp {
        DbResponse::Error { code, message } => assert_eq!(
            (code.as_str(), message.as_str()),
            ("unknown_table", "table is unavailable"),
            "batch read of unknown table '{table}'"
        ),
        other => panic!("batch read of unknown table '{table}': got {other:?}"),
    }
}

/// Cursor creation uses the same canonical, name-hiding refusal and never
/// becomes a cursor.
fn expect_cursor_unknown_table_refusal(resp: DbResponse, table: &str) {
    match resp {
        DbResponse::Error { code, message } => assert_eq!(
            (code.as_str(), message.as_str()),
            ("unknown_table", "table is unavailable"),
            "cursor create of unknown table '{table}'"
        ),
        other => panic!("cursor create of unknown table '{table}': got {other:?}"),
    }
}

/// Directly probe the cold resolver (not `GetRecord`, `Execute`, or cursor
/// preflight): two distinct unknown names must not allocate catalogue cells.
#[tokio::test]
async fn cold_resolver_unknown_names_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let before = table_cell_count(&handler);
    let db = handler.db();

    for name in ["cold_absent_one", "cold_absent_two"] {
        assert!(
            db.get_table("app", "main", name).await.is_err(),
            "the cold resolver must refuse unknown table '{name}'"
        );
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "direct cold-resolver refusals must not insert catalogue cells"
    );
}

#[tokio::test]
async fn batch_reads_of_unknown_tables_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let session = superuser_alice_session();
    let before = table_cell_count(&handler);

    for name in unknown_names() {
        let resp = send(&handler, &session, batch_read_req(name.as_str())).await;
        expect_batch_unknown_table_refusal(resp, name.as_str());
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "a refused batch read must not insert a catalogue cell for its table"
    );
}

/// (b) The same eight names, each a refused cursor create, must leave the
/// cell map unchanged.
///
/// WITHOUT the fix these eight requests grow the cell map by eight.
#[tokio::test]
async fn cursor_creates_over_unknown_tables_do_not_grow_catalogue_cells() {
    let handler = build_growth_handler().await;
    let session = superuser_alice_session();
    let before = table_cell_count(&handler);

    for name in unknown_names() {
        let resp = send(&handler, &session, create_cursor_req(name.as_str())).await;
        expect_cursor_unknown_table_refusal(resp, name.as_str());
    }

    assert_eq!(
        table_cell_count(&handler),
        before,
        "a refused cursor create must not insert a catalogue cell for its table"
    );
}

/// PAIR under a SATURATED request-slot pool: absent and existing-but-closed
/// tables must both answer the fixed refusal, never `server_busy`; availability
/// is decided BEFORE the slot gate, so the absent name cannot reach the
/// exhausted slot while the closed one stops at the table ACL.
#[tokio::test]
async fn get_record_absent_and_closed_tables_are_never_server_busy_under_saturated_slot() {
    let (handler, seeded) = build_handler(ByteBudget::unbounded(), parked_limits()).await;
    let store = SessionStore::new();
    let session = store.insert(ALICE_SID, alice_session());
    chmod_items_table(&handler, 0o000).await;
    let held = session.try_acquire_request_slot(1).expect("the cap-1 slot");
    let closed = send(&handler, &session, get_record_req(seeded)).await;
    let absent = send(
        &handler,
        &session,
        get_record("app", "main", "absent_items", seeded),
    )
    .await;
    drop(held);
    for (resp, label) in [(closed, "closed"), (absent, "absent")] {
        let (code, message) = match resp {
            DbResponse::Error { code, message } => (code, message),
            other => panic!("{label} table: expected the fixed refusal, got {other:?}"),
        };
        assert_eq!(
            (code.as_str(), message.as_str()),
            ("unknown_table", "table is unavailable"),
            "{label} table under a saturated slot"
        );
    }
}
