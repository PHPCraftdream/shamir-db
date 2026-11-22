//! P2 `DbRequest::GetRecord` handler tests (admin-shell PR3, exact-key
//! point read in `repo_admin/operator_state.rs`).
//!
//! Covers: exact-key lookup vs the payload `_id` string, wrong typed id
//! → typed `not_found`, ACL denial BEFORE any table lookup (Database
//! Read denied; Database-allowed-but-Table-Read denied), the repository
//! read-quarantine (`resync_required`, same fixture as
//! `cursor_quarantine_tests`), cursor-registry isolation (a `data get`
//! creates and consumes no cursor — with a positive control), the
//! `max_result_size_bytes` rejection, the per-session in-flight slot
//! positive control (store-minted handle), the fixed 16-byte wire key
//! decode (unit AND handler level, incl. a 17-byte malformed frame),
//! and missing db/repo/table resolution under a SUPERUSER control
//! (so the ACL's fail-closed fallback cannot mask genuine misses).
//! Requests are built ONLY through `shamir_query_builder`
//! (`wire::admin::get_record`, `batch::Batch`, `write::insert`);
//! responses are decoded through the wire types.
//!
//! Fixture mirrors `cursor_quarantine_tests.rs` (its helpers are
//! module-private and the layout cap forbids widening shared fixtures
//! beyond this topic folder). Every await is bounded by a 120 s timeout
//! (the Windows CI runner may be up to 10x slower).

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
use shamir_query_builder::cursor::{create_cursor, fetch_next};
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_builder::wire::admin::get_record;
use shamir_query_builder::write::insert;
use shamir_query_types::admin::{ReplDirection, ReplMode, ResourceRef};
use shamir_query_types::read::QueryRecord;
use shamir_query_types::wire::{CursorId, DbRequest, DbResponse};

use shamir_types::types::record_id::RecordId;

use crate::db_handler::config::{CursorLimitsCap, QueryLimitsCap};
use crate::db_handler::handler::ShamirDbHandler;

const ALICE_SID: [u8; 32] = [0xAA; 32];
const BOB_SID: [u8; 32] = [0xBB; 32];
/// Generous per-call bound (hang detector; slow Windows runner).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

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

fn bob_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    );
    s.session_id = BOB_SID;
    s
}

/// Superuser-class alice: `session_actor` maps `is_superuser` to
/// `Actor::Admin`, which short-circuits `authorize_access` — used ONLY
/// where the test's subject is name RESOLUTION, not ACL: a non-admin
/// actor would let the ACL's fail-closed `resource_meta` fallback hide a
/// genuinely missing name behind `access_denied`.
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

/// Handler over an in-memory `ShamirDb` with `app/{main,side}.items`,
/// owned by alice (principal `[0xAB; 16]` — same identity the alice
/// session carries, so alice is the owner). `main` is seeded with one
/// row `{ id: "k000", v: 0 }`; `side` stays empty for the
/// non-replicated-repo assertions. The enforced 0o700 default keeps
/// bob out of everything until a test chmod's levels open.
async fn build_handler(query_limits: QueryLimitsCap) -> ShamirDbHandler {
    let shamir = ShamirDb::init_memory().await.expect("init shamir");
    let owner = Actor::User(principal64([0xAB; 16]));
    shamir
        .create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    for repo in ["main", "side"] {
        let cfg =
            RepoConfig::new(repo, BoxRepoFactory::in_memory()).add_table(TableConfig::new("items"));
        shamir
            .add_repo_as("app", cfg, owner.clone())
            .await
            .expect("add repo");
    }

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "seed_k000",
        insert("items").row(doc! { "id" => "k000", "v" => 0_i64 }),
    );
    shamir
        .execute_as(owner, "app", &b.build())
        .await
        .expect("seed row");

    ShamirDbHandler::new(Arc::new(shamir))
        .with_cursor_limits(CursorLimitsCap::UNLIMITED)
        .with_query_limits(query_limits)
}

/// Send a wire request through the REAL `RequestHandler::handle`,
/// bounded by `CALL_TIMEOUT`.
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

/// Insert one row through the ENGINE's real `execute_as` path and return
/// its STORAGE `RecordId` — the typed `QueryRecord::Inserted` /
/// `InsertedRecord::id` the write result carries, never any payload
/// field. The insert deliberately does NOT go through the handler's
/// wire path: the result-size fixtures configure an 8-byte
/// `max_result_size_bytes` cap on the handler, which would reject the
/// insert's OWN batch response with `result_too_large` before any
/// point read could ever be sent. Seeding happens below the fixture's
/// budget so only the measured request is under the cap.
async fn insert_row_and_get_storage_id(handler: &ShamirDbHandler, payload_id: &str) -> RecordId {
    let mut b = Batch::new();
    b.insert(
        "row",
        insert("items").row(doc! { "id" => payload_id, "v" => 1_i64 }),
    );
    let response = handler
        .db()
        .execute_as(Actor::System, "app", &b.build())
        .await
        .expect("engine insert must succeed");
    let result = response.results.get("row").expect("insert alias returned");
    match result.records.first() {
        Some(QueryRecord::Inserted(record)) => {
            record.id.expect("insert must carry the storage RecordId")
        }
        other => panic!("expected an Inserted write-result row, got {other:?}"),
    }
}

fn expect_error(resp: DbResponse, code: &str, context: &str) {
    match resp {
        DbResponse::Error { code: got, .. } => assert_eq!(got, code, "{context}"),
        other => panic!("{context}: expected error code '{code}', got {other:?}"),
    }
}

fn get_record_req(key: RecordId) -> DbRequest {
    get_record("app", "main", "items", key)
}

/// Open `app.main.items` to `mode` at ALL three ancestor levels
/// (database, store, table) — `authorize_access`'s ancestor walk
/// requires `Read` on every level (mirrors
/// `cursor/handler/common.rs::open_app_main_items`).
async fn open_all_levels(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "db",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Database {
                database: "app".into(),
            },
            mode,
        )
        .hmac("get-record-test")
        .build()
        .expect("build chmod db"),
    );
    batch.chmod(
        "store",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Store {
                store: ["app".into(), "main".into()],
            },
            mode,
        )
        .hmac("get-record-test")
        .build()
        .expect("build chmod store"),
    );
    batch.chmod(
        "table",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Table {
                table: ["app".into(), "main".into(), "items".into()],
            },
            mode,
        )
        .hmac("get-record-test")
        .build()
        .expect("build chmod table"),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &batch.build())
        .await
        .expect("admin chmod must succeed");
}

/// Open ONLY the database and store ancestors to `mode`, leaving the
/// table at its enforced 0o700 default — the
/// "Database-read-but-Table-read-denied" ACL state.
async fn open_db_and_store_only(handler: &ShamirDbHandler, mode: u16) {
    let mut batch = Batch::new();
    batch.chmod(
        "db",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Database {
                database: "app".into(),
            },
            mode,
        )
        .hmac("get-record-test")
        .build()
        .expect("build chmod db"),
    );
    batch.chmod(
        "store",
        shamir_query_builder::ddl::chmod(
            ResourceRef::Store {
                store: ["app".into(), "main".into()],
            },
            mode,
        )
        .hmac("get-record-test")
        .build()
        .expect("build chmod store"),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &batch.build())
        .await
        .expect("admin chmod must succeed");
}

/// Same persisted quarantine fixture as `cursor_quarantine_tests`:
/// create a Pull profile + ACTIVE subscription covering `repo`, then
/// flip it to `resync_required` through the real path
/// (`ShamirDb::mark_subscription_resync_required`). The refresh hook
/// rebuilds the read-quarantine cache automatically.
async fn quarantine(handler: &ShamirDbHandler, sub: &str, profile: &str, repo: &str) {
    let mut b = Batch::new();
    b.id(2);
    b.op(
        "profile",
        replication_profile(profile).stream(
            repl_scope("app").repo(repo).build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    b.op("sub", subscription(sub, "tcp://peer:9000", "pub", profile));
    handler
        .db()
        .execute_as(Actor::System, "app", &b.build())
        .await
        .expect("admin replication batch must succeed");
    handler
        .db()
        .mark_subscription_resync_required(sub, 7, 3)
        .await
        .expect("mark resync_required must succeed");
}

// ---------------------------------------------------------------------------
// Exact-key lookup vs payload `_id`.
// ---------------------------------------------------------------------------

/// The point read resolves the STORAGE record id (the write result's
/// `_id`), not any payload field: the row whose payload `id` string is
/// "k001" is found only under its real storage id, and the returned
/// record carries the seeded payload fields.
#[tokio::test]
async fn get_record_exact_storage_key_returns_the_row() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let storage_id = insert_row_and_get_storage_id(&handler, "k001").await;
    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    match resp {
        DbResponse::Record { record } => {
            assert_eq!(
                record.get_value_i64("v"),
                Some(1),
                "the seeded row's payload must come back"
            );
        }
        other => panic!("expected Record, got {other:?}"),
    }
}

/// The typed key is the storage id, NOT the payload's valid base58
/// `id` string: that independent value points at no record → typed
/// `not_found`.
#[tokio::test]
async fn get_record_payload_id_string_is_not_the_lookup_key() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // An INDEPENDENT, well-formed typed id, base58-encoded: a plausible
    // payload id STRING (stored as the row's `id` field below), never a
    // storage key. The earlier literal "k000" was invalid — Base58 has
    // no '0' digit, so the probe could not parse at all.
    let payload_string = RecordId::new().to_string();
    let mut b = Batch::new();
    b.insert(
        "payload",
        insert("items").row(doc! { "id" => payload_string.as_str(), "v" => 9_i64 }),
    );
    handler
        .db()
        .execute_as(Actor::System, "app", &b.build())
        .await
        .expect("seed the payload row");

    let as_key: RecordId = payload_string
        .parse()
        .expect("base58 payload id string parses");
    let resp = send(&handler, &session, get_record_req(as_key)).await;
    expect_error(resp, "not_found", "payload id string as typed key");
}

/// A well-formed but unknown typed id is a clean typed `not_found`,
/// never a panic or a scan.
#[tokio::test]
async fn get_record_wrong_typed_id_is_typed_not_found() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let missing = RecordId([7u8; 16]);
    let resp = send(&handler, &session, get_record_req(missing)).await;
    expect_error(resp, "not_found", "unknown well-formed id");
}

/// Superuser control for genuine resolution misses (the ACL test suite
/// separately quantifies database-Read denied vs table-Read denied).
/// The point-read handler must still resolve real database/repository/table
/// misses to their typed codes once the session has permission to ask.
#[tokio::test]
async fn get_record_missing_parents_and_table_are_typed() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = superuser_alice_session();

    let key = RecordId([1u8; 16]);
    let resp = send(
        &handler,
        &session,
        get_record("app", "main", "no_such_table", key),
    )
    .await;
    expect_error(resp, "unknown_table", "missing table, parents exist");

    let resp = send(
        &handler,
        &session,
        get_record("app", "no_such_repo", "items", key),
    )
    .await;
    expect_error(resp, "unknown_repo", "missing repo");

    let resp = send(
        &handler,
        &session,
        get_record("no_such_db", "main", "items", key),
    )
    .await;
    expect_error(resp, "unknown_db", "missing db");
}

// ---------------------------------------------------------------------------
// Denial BEFORE the table lookup.
// ---------------------------------------------------------------------------

/// Database `Read` denied (enforced 0o700 default, bob is not the
/// owner): `access_denied` fires BEFORE any resolution — the SAME code
/// comes back for an existing table and for a table that does not
/// exist, so denial stays indistinguishable from not-found.
#[tokio::test]
async fn get_record_denied_when_database_read_denied() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let bob = bob_session();

    let key = RecordId([1u8; 16]);
    let resp = send(&handler, &bob, get_record_req(key)).await;
    expect_error(resp, "access_denied", "db Read denied, existing table");

    // Indistinguishability probe: identical denial on a table that does
    // not exist (proves the denial precedes any table lookup).
    let resp = send(
        &handler,
        &bob,
        get_record("app", "main", "no_such_table", key),
    )
    .await;
    expect_error(resp, "access_denied", "db Read denied, unknown table");
}

/// The TWO denial conditions are separate and separately proven:
///
/// - `get_record_denied_when_database_read_denied` (below): the FIRST
///   `authorize_access` call (`Action::Read` on the `Database`) refuses
///   — table and even repo/db names are never resolved.
/// - this test: the FIRST call succeeds (db + store chmod'd open) and
///   the SECOND call (`Action::Read` on the specific `Table`) refuses —
///   still before any lookup.
#[tokio::test]
async fn get_record_denied_when_table_read_denied() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    open_db_and_store_only(&handler, 0o777).await;
    let bob = bob_session();

    let key = RecordId([1u8; 16]);
    let resp = send(&handler, &bob, get_record_req(key)).await;
    expect_error(resp, "access_denied", "db allowed, table Read denied");
}

/// PARITY (pins the CURRENT documented behaviour; it does not change the
/// ACL policy): with Database Read granted and Table Read restricted, the
/// SAME principal can tell a CONFIGURED private table from an ABSENT name.
///
/// The two outcomes are deliberately paired because the deny-then-resolve
/// order makes them different: `authorize_cursor_read` authorizes the Table
/// NAME, and the ACL default is open for a name with no catalogue record,
/// so an absent name passes the Table check and name resolution then fails
/// with `unknown_table`. Pinning both here keeps a future re-order from
/// silently widening or narrowing the accepted disclosure.
#[tokio::test]
async fn get_record_denied_present_table_and_absent_name_are_distinguishable() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    open_db_and_store_only(&handler, 0o777).await;
    let bob = bob_session();
    let key = RecordId([1u8; 16]);

    // Present private table: Database+Store Read pass, Table Read denied.
    let resp = send(&handler, &bob, get_record_req(key)).await;
    expect_error(resp, "access_denied", "db allowed, table Read denied");

    // Absent name: no catalogue record, so the Table ACL default is open
    // and name resolution is what refuses.
    let resp = send(
        &handler,
        &bob,
        get_record("app", "main", "no_such_table", key),
    )
    .await;
    expect_error(resp, "unknown_table", "db allowed, absent table name");
}

/// Same request as `get_record_denied_when_table_read_denied` with all
/// three levels opened: bob's point read SUCCEEDS — the ACL state, not
/// the handler, was the refuser.
#[tokio::test]
async fn get_record_allowed_once_all_levels_grant_read() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    open_all_levels(&handler, 0o777).await;
    let bob = bob_session();

    let storage_id = insert_row_and_get_storage_id(&handler, "k002").await;
    let resp = send(&handler, &bob, get_record_req(storage_id)).await;
    assert!(
        matches!(resp, DbResponse::Record { .. }),
        "granted non-owner must get the record: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// Repository quarantine.
// ---------------------------------------------------------------------------

/// A quarantined repo refuses the point read with the SAME wire code the
/// cursor/batch path carries (`resync_required`), probed before any
/// lookup; a non-replicated sibling repo is unaffected.
#[tokio::test]
async fn get_record_quarantined_repo_is_resync_required() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    // Insert while the repo is still healthy — the write path must not
    // be assumed quarantine-free; the test gates READS only.
    let storage_id = insert_row_and_get_storage_id(&handler, "k003").await;
    quarantine(&handler, "sub", "prof", "main").await;

    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    expect_error(resp, "resync_required", "quarantined repo");

    // Non-replicated `side` (empty table) is unaffected: typed
    // not_found, not resync_required.
    let resp = send(
        &handler,
        &session,
        get_record("app", "side", "items", RecordId([1u8; 16])),
    )
    .await;
    expect_error(resp, "not_found", "non-replicated repo unaffected");
}

/// The point read's `resync_required` refusal must NOT carry the stored
/// quarantine reason verbatim: `mark_subscription_resync_required(sub, 7, 3)`
/// stores `journal gap at 7, requested from 3`, that reason stays in the
/// engine/cache (and in the shared batch path), while `GetRecord` answers
/// with the fixed text.
///
/// WITHOUT the fixed text the wire message is the shared
/// `BatchError::ResyncRequired` Display, which includes the stored reason.
#[tokio::test]
async fn get_record_quarantine_refusal_does_not_carry_the_stored_reason() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let storage_id = insert_row_and_get_storage_id(&handler, "k007").await;
    quarantine(&handler, "sub", "prof", "main").await;

    let reason = handler
        .db()
        .read_quarantine_reason("app", "main")
        .expect("the quarantine cache must carry the reason the mark stored");
    assert!(
        reason.contains("journal gap"),
        "the stored reason is the journal-gap text, got {reason:?}"
    );

    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    match resp {
        DbResponse::Error { code, message } => {
            assert_eq!(code, "resync_required", "quarantined point read");
            assert_eq!(
                message, "repository is quarantined; resynchronize before reading",
                "the point read must carry the fixed refusal text"
            );
            assert!(
                !message.contains("journal gap"),
                "the stored quarantine reason must never be echoed verbatim, got {message:?}"
            );
        }
        other => panic!("expected the fixed resync_required refusal, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Cursor-registry isolation (no cursor created or consumed).
// ---------------------------------------------------------------------------

/// A successful `data get` must not create OR consume any cursor:
/// registry count is captured before/after around a positive control
/// (a real CreateCursor bumps the count to 1; the point read leaves it
/// at exactly that cursor).
#[tokio::test]
async fn get_record_never_creates_or_consumes_a_cursor() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let before = handler.cursor_registry().len();
    assert_eq!(before, 0, "fixture starts with an empty registry");

    // Positive control: the cursor path really is observable here.
    // The fixture starts with exactly ONE row, so page_size=1 leaves
    // has_more=true and the control cursor is actually registered.
    let storage_id = insert_row_and_get_storage_id(&handler, "k004").await;
    let resp = send(
        &handler,
        &session,
        create_cursor(
            "app",
            shamir_query_builder::Query::with_repo("main", "items").build(),
            1,
        ),
    )
    .await;
    let CursorId(control_id) = match resp {
        DbResponse::CursorPage {
            cursor_id,
            has_more,
            ..
        } => {
            assert!(has_more, "positive control: cursor has_more must be true");
            cursor_id
        }
        other => panic!("positive control: expected CursorPage, got {other:?}"),
    };
    assert_eq!(
        handler.cursor_registry().len(),
        1,
        "control cursor registered"
    );

    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    assert!(
        matches!(resp, DbResponse::Record { .. }),
        "get must succeed"
    );

    assert_eq!(
        handler.cursor_registry().len(),
        1,
        "a data get must not create or consume any cursor"
    );

    // The surviving cursor is still the CONTROL cursor — fetchable.
    let fetch = fetch_next(control_id, Some(1));
    let resp = send(&handler, &session, fetch).await;
    assert!(
        matches!(resp, DbResponse::CursorPage { .. }),
        "the control cursor must be untouched by the point read: {resp:?}"
    );
    // The fetch above returns the LAST row, so the control cursor is
    // exhausted and the REAL fetch path removes it from the registry
    // (`cursor_handlers/fetch.rs` removes an exhausted cursor). The
    // point read must have created no cursor of its own and must not
    // have touched the control cursor — the registry is empty exactly
    // because of the control cursor's own terminal fetch, and would
    // still be empty had the point read run twice.
    assert_eq!(
        handler.cursor_registry().len(),
        0,
        "terminal fetch consumed the control cursor; the point read created none"
    );
}

// ---------------------------------------------------------------------------
// Result-size control.
// ---------------------------------------------------------------------------

/// A `max_result_size_bytes` cap smaller than the encoded `DbResponse`
/// envelope rejects the point read with `result_too_large` — the
/// rejection shape the batch path uses, mirrored by `get_record`.
#[tokio::test]
async fn get_record_rejects_result_over_max_result_bytes() {
    let tiny = QueryLimitsCap {
        enforce_result_size: true,
        max_result_size_bytes: 8,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(tiny).await;
    let session = alice_session();

    let storage_id = insert_row_and_get_storage_id(&handler, "k005").await;
    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    expect_error(resp, "result_too_large", "response over the byte cap");
}

/// Same cap, but a request that is DENIED earlier (ACL) still returns
/// the denial — the byte cap never masks an earlier gate's answer.
#[tokio::test]
async fn get_record_byte_cap_does_not_mask_acl_denial() {
    let tiny = QueryLimitsCap {
        enforce_result_size: true,
        max_result_size_bytes: 8,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(tiny).await;
    let bob = bob_session();

    let resp = send(&handler, &bob, get_record_req(RecordId([1u8; 16]))).await;
    expect_error(resp, "access_denied", "cap active, ACL denial first");
}

// ---------------------------------------------------------------------------
// Fixed 16-byte wire key decode.
// ---------------------------------------------------------------------------

/// The wire key is a FIXED 16-byte record id (msgpack `bin`): a
/// builder-built `GetRecord` survives a full msgpack round trip with
/// its key bytes intact, and the `RecordId` type itself refuses
/// wrong-sized byte slices at construction (the "typed-key size cap" —
/// a malformed key is a typed refusal at decode time, never a scan).
#[test]
fn get_record_wire_key_is_fixed_16_bytes_and_round_trips() {
    let key = RecordId([0x5A; 16]);
    let req = get_record("app", "main", "items", key);
    let bytes = rmp_serde::to_vec_named(&req).expect("encode GetRecord");
    let decoded: DbRequest = rmp_serde::from_slice(&bytes).expect("decode GetRecord");
    match decoded {
        DbRequest::GetRecord {
            db,
            repo,
            table,
            key: got,
        } => {
            assert_eq!(
                (db.as_str(), repo.as_str(), table.as_str()),
                ("app", "main", "items")
            );
            assert_eq!(
                got.as_bytes(),
                &[0x5A; 16],
                "fixed 16-byte key must round-trip"
            );
        }
        other => panic!("expected GetRecord, got {other:?}"),
    }

    // Typed-key size cap: 15 and 17 bytes are refused at the type level.
    assert!(RecordId::try_from_bytes(&[0u8; 15]).is_none());
    assert!(RecordId::try_from_bytes(&[0u8; 17]).is_none());
    assert!(RecordId::try_from_bytes(&[0u8; 16]).is_some());
}

// ---------------------------------------------------------------------------
// Per-session in-flight slot (positive control).
// ---------------------------------------------------------------------------

/// POSITIVE control for the in-flight gate: a REAL store-minted session
/// (nonzero activity handle — the precondition for cap enforcement) with
/// its single slot already acquired externally gets the fixed-text
/// `server_busy` rejection BEFORE any engine work; releasing the slot
/// lets the SAME request succeed. Proves `get_record` actually acquires
/// the per-session slot (a gate that never fires would admit everything).
#[tokio::test]
async fn get_record_in_flight_cap_positive_control() {
    let capped = QueryLimitsCap {
        max_in_flight_requests_per_session: 1,
        ..QueryLimitsCap::UNLIMITED
    };
    let handler = build_handler(capped).await;
    let store = SessionStore::new();
    // Stamp through the store: store-inserted sessions carry a minted
    // nonzero handle (same precondition the slot tests assert).
    let session = store.insert(ALICE_SID, alice_session());
    assert_ne!(session.activity_handle(), 0, "stamped session");

    let storage_id = insert_row_and_get_storage_id(&handler, "k006").await;

    // Exhaust the single-slot cap OUTSIDE the handler, then prove the
    // point read is refused with the fixed `server_busy` text.
    let slot = session
        .try_acquire_request_slot(1)
        .expect("the cap-1 session admits exactly one external slot");
    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    expect_error(
        resp,
        "server_busy",
        "slot exhausted, refused before engine work",
    );
    drop(slot);

    // Released: the identical request now succeeds (no stale denial).
    let resp = send(&handler, &session, get_record_req(storage_id)).await;
    assert!(
        matches!(resp, DbResponse::Record { .. }),
        "after the slot is released the point read succeeds: {resp:?}"
    );
}

// ---------------------------------------------------------------------------
// Malformed wire key: decoder AND handler level.
// ---------------------------------------------------------------------------

/// A 17-byte key is refused by the ACTUAL msgpack decoder (`rmp_serde`
/// from_slice over `DbRequest`): the fixed-width `ByteArray<16>` codec
/// is a typed decode error, not a truncated/re-sized id.
#[test]
fn get_record_17_byte_key_is_a_decoder_error() {
    let request = get_record("app", "main", "items", RecordId([0x5A; 16]));
    let bytes = rmp_serde::to_vec_named(&request).expect("encode GetRecord");

    // Locate the key: bin8 header (0xC4, 0x10) followed by 16 x 0x5A.
    let start = bytes
        .windows(3)
        .position(|w| w == [0xC4, 0x10, 0x5A])
        .expect("bin8-encoded 16-byte key present");
    let mut malformed = bytes.clone();
    malformed[start + 1] = 0x11; // bin8 length 0x10 -> 0x11 (17)
    malformed.insert(start + 2 + 16, 0x5A); // one extra key byte

    let decoded: Result<DbRequest, _> = rmp_serde::from_slice(&malformed);
    assert!(
        decoded.is_err(),
        "a 17-byte key must fail the fixed-width decode, got {decoded:?}"
    );
}

/// The SAME malformed frame through the REAL `RequestHandler::handle`
/// never reaches the point-read handler and never panics: the envelope
/// decode fails first and the wire answer is the typed
/// `invalid_request` envelope (`handler.rs` decode boundary).
#[tokio::test]
async fn get_record_malformed_key_frame_is_invalid_request_at_the_handler() {
    let handler = build_handler(QueryLimitsCap::UNLIMITED).await;
    let session = alice_session();

    let request = get_record("app", "main", "items", RecordId([0x5A; 16]));
    let bytes = rmp_serde::to_vec_named(&request).expect("encode GetRecord");
    let start = bytes
        .windows(3)
        .position(|w| w == [0xC4, 0x10, 0x5A])
        .expect("bin8-encoded 16-byte key present");
    let mut malformed = bytes.clone();
    malformed[start + 1] = 0x11;
    malformed.insert(start + 2 + 16, 0x5A);
    // Decoder sanity first (cheap, same frame as the unit test above).
    assert!(rmp_serde::from_slice::<DbRequest>(&malformed).is_err());

    let conn = ConnectionServices::without_push(0);
    let resp_bytes = tokio::time::timeout(
        CALL_TIMEOUT,
        Box::pin(handler.handle(&session, &malformed, &conn)),
    )
    .await
    .expect("handle must complete within the 120s bound")
    .expect("malformed frames are typed responses, not transport errors");
    let decoded: DbResponse = rmp_serde::from_slice(&resp_bytes).expect("decode response");
    expect_error(decoded, "invalid_request", "17-byte key frame");
}
