//! Shape-statistics handler tests (R4b step 4, item 3).
//!
//! Coverage, all through the REAL `RequestHandler::handle` dispatch:
//!
//! - the `ActivityView` + `Action::Manage` permission gate of BOTH ops,
//!   its ordering (permission BEFORE the repository lookup — an unknown
//!   repository still answers `permission_denied`) and the typed refusal
//!   text (no caller-supplied name, no path);
//! - the end-to-end lifecycle: literal-only traffic folds to ONE shape, a
//!   reset empties rows + `(other)` + `tracked`, and the audit chain holds
//!   exactly ONE reset record carrying the repository name only;
//! - `limit` handling (`0` → `invalid_request`, default 50, clamp), the
//!   total deterministic page order (`total_nanos` desc, `executions`
//!   desc, `shape_hex` asc) and truncation;
//! - the stable `unknown_db` / `unknown_repo` refusals, and that a
//!   read-only replica serves both ops (no node-mode gate: `is_write()` is
//!   `false` for local in-memory telemetry).
//!
//! Fixture: in-memory `ShamirDb` + `ShamirDbHandler::new`, `app`/`main`
//! with a schema-less `items` table and OPEN data-path metadata. Every
//! await is bounded by [`CALL_TIMEOUT`].

use std::future::Future;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::wire::admin::{list_shape_stats, reset_shape_stats};
use shamir_query_builder::write::{doc, insert};
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq, RequestExecId,
};
use shamir_query_types::admin::shape_stats::{ListShapeStatsReq, ResetShapeStatsReq};
use shamir_query_types::admin::{ShapeStatsPage, ShapeStatsRow, ShapeStatsTotals};
use shamir_query_types::wire::{error_code, DbRequest, DbResponse};

use shamir_types::access::{ResourceMeta, ResourcePath};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::ShamirDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const DB: &str = "app";
const REPO: &str = "main";
const TABLE: &str = "items";
/// The two fields the fixture's seed document interns, so the traffic
/// below filters on REAL (interned) field names.
const FIELD_NOTE: &str = "note";
const FIELD_TAG: &str = "tag";

/// Hard per-call bound: every await is bounded, never an unbounded engine
/// await.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The exact fixed refusal text of the shape handlers (asserted verbatim:
/// a drift here is a wire-visible change).
const MANAGE_REQUIRED: &str =
    "shape statistics aggregate every user's workload; ActivityView manage permission is \
     required";

fn alice_session() -> Session {
    let mut s = Session::new(
        [0xAB; 16],
        "alice".into(),
        SessionPermissions::new(false, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0xA1; 32];
    s
}

/// Ordinary user that will be GRANTED `ActivityView` + `Manage` by the
/// fixture (never a superuser: the grant, not the admin bypass, is tested).
fn bob_session() -> Session {
    let mut s = Session::new(
        [0xB2; 16],
        "bob".into(),
        SessionPermissions::new(false, false, vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0xB2; 32];
    s
}

fn manager_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "manager".into(),
        SessionPermissions::new(true, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    s.session_id = [0xFE; 32];
    s
}

/// Existing server fixture convention: deep engine futures need a roomy
/// stack; one runner per test, no runtime-inside-runtime.
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = ()>,
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
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// In-memory `app`/`main`/`items`, owned by alice, with OPEN data-path
/// metadata so every fixture session may read and write the table.
async fn open_fixture() -> (Arc<ShamirDb>, Session) {
    let shamir = ShamirDb::init_memory().await.expect("init in-memory db");
    let alice = alice_session();
    let owner = Actor::User(principal64(alice.user_id));
    shamir
        .create_db_as(DB, owner.clone())
        .await
        .expect("create_db");
    shamir
        .add_repo_as(
            DB,
            RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE)),
            owner,
        )
        .await
        .expect("add repo");
    for path in [
        ResourcePath::database(DB),
        ResourcePath::store(DB, REPO),
        ResourcePath::table(DB, REPO, TABLE),
    ] {
        shamir
            .set_resource_meta(&path, &ResourceMeta::open())
            .await
            .expect("open data path");
    }
    (Arc::new(shamir), alice)
}

/// Grant `ActivityView` + `Manage` to exactly one ordinary user, through the
/// same setting key the authorization path reads (`ShamirDb::
/// set_resource_meta` has no `ActivityView` arm). Owner-only rwx
/// (`ResourceMeta::owned_enforced`) is the single-principal `Manage` rule.
async fn grant_activity_manage(shamir: &ShamirDb, session: &Session) {
    let meta = ResourceMeta::owned_enforced(Actor::User(principal64(session.user_id)));
    let mut map = new_map();
    map.insert(
        "key".to_string(),
        QueryValue::Str("activity_view_meta".to_string()),
    );
    let mut rec = QueryValue::Map(map);
    meta.inject_into(&mut rec);
    tokio::time::timeout(
        CALL_TIMEOUT,
        shamir
            .system_store()
            .save_setting("activity_view_meta", &rec),
    )
    .await
    .expect("save activity view meta within the per-call timeout")
    .expect("grant ActivityView Manage");
}

async fn send(handler: &ShamirDbHandler, session: &Session, req: &DbRequest) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(req).expect("encode request");
    let conn = ConnectionServices::without_push(0);
    let raw = tokio::time::timeout(CALL_TIMEOUT, handler.handle(session, &bytes, &conn))
        .await
        .expect("handle within the per-call timeout")
        .expect("handle request");
    rmp_serde::from_slice(&raw).expect("decode DbResponse")
}

fn list_req(db: &str, repo: &str, limit: Option<u32>) -> DbRequest {
    let mut b = Batch::new();
    b.op(
        "list_shape_stats",
        list_shape_stats(ListShapeStatsReq {
            repo: repo.to_owned(),
            limit,
        }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.to_owned(),
        batch: b.build(),
    }
}

fn reset_req(db: &str, repo: &str) -> DbRequest {
    let mut b = Batch::new();
    b.op(
        "reset_shape_stats",
        reset_shape_stats(ResetShapeStatsReq {
            repo: repo.to_owned(),
        }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: db.to_owned(),
        batch: b.build(),
    }
}

/// One insert that interns BOTH fixture fields (the reads below filter on
/// real field names, so an unknown-field collapse cannot mask a bug).
fn seed_req() -> DbRequest {
    let mut b = Batch::new();
    b.insert(
        "seed",
        insert(TABLE).row(
            doc()
                .set(FIELD_NOTE, "seed")
                .expect("note field")
                .set(FIELD_TAG, "seedtag")
                .expect("tag field"),
        ),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.to_owned(),
        batch: b.build(),
    }
}

/// Reads that differ ONLY in one string literal: they must fold to ONE shape.
fn literal_reads(values: &[&str]) -> DbRequest {
    let mut b = Batch::new();
    for (index, value) in values.iter().enumerate() {
        b.query(
            format!("r{index}"),
            Query::from(TABLE).where_eq(FIELD_NOTE, *value),
        );
    }
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.to_owned(),
        batch: b.build(),
    }
}

/// First traffic batch: two literal-only variants of ONE shape (2
/// executions), a different-field read, a filter-less read, and one insert.
fn traffic_first() -> DbRequest {
    let mut b = Batch::new();
    b.query("a1", Query::from(TABLE).where_eq(FIELD_NOTE, "aaa"));
    b.query("a2", Query::from(TABLE).where_eq(FIELD_NOTE, "bbb"));
    b.query("t1", Query::from(TABLE).where_eq(FIELD_TAG, "ttt"));
    b.query("all", Query::from(TABLE));
    b.insert(
        "i1",
        insert(TABLE).row(doc().set(FIELD_NOTE, "another").expect("note field")),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.to_owned(),
        batch: b.build(),
    }
}

/// Second traffic batch: one more execution of the literal-only shape and
/// one more of the filter-less shape. Final counts: 3 / 1 / 2 / 1.
fn traffic_second() -> DbRequest {
    let mut b = Batch::new();
    b.query("a3", Query::from(TABLE).where_eq(FIELD_NOTE, "ccc"));
    b.query("all2", Query::from(TABLE));
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.to_owned(),
        batch: b.build(),
    }
}

fn expect_page(resp: DbResponse) -> ShapeStatsPage {
    match resp {
        DbResponse::ShapeStatsPage { page } => page,
        // Never print the non-matching response: it may echo request
        // material and this suite is also a privacy witness.
        _ => panic!("expected ShapeStatsPage"),
    }
}

/// The documented total order: `total_nanos` desc, `executions` desc,
/// `shape_hex` asc.
fn ordered(rows: &[ShapeStatsRow]) -> bool {
    rows.windows(2).all(|pair| {
        let (left, right) = (&pair[0], &pair[1]);
        left.total_nanos > right.total_nanos
            || (left.total_nanos == right.total_nanos
                && (left.executions > right.executions
                    || (left.executions == right.executions && left.shape_hex < right.shape_hex)))
    })
}

fn is_lower_hex(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Default)]
struct CapturedAudit(Mutex<Vec<AuditEntry>>);

impl AuditAppender for CapturedAudit {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.0.lock().expect("audit mutex").push(entry.clone());
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

fn assert_refusal_not_path_like(message: &str) {
    for forbidden in ['/', '\\', ':'] {
        assert!(
            !message.contains(forbidden),
            "a refusal must not carry a path-like or raw string"
        );
    }
}

/// Permission denial comes FIRST, for BOTH ops: an unauthorized caller is
/// refused identically for an existing and a nonexistent repository/database
/// (no existence oracle), the refusal is the typed `permission_denied` with
/// the fixed text, a denied reset performs no wipe, and a session holding
/// `ActivityView:Manage` (a non-superuser, granted through the same
/// `activity_view_meta` setting the authorization path reads) is allowed for
/// both ops.
///
/// Catches: dropping the `authorize_access` call from either handler;
/// moving the permission check after `resolve_repo` (the unknown-repo case
/// would answer `unknown_repo`); widening the gate to a superuser test; a
/// non-typed/echoing refusal; and a denied reset reaching the registry.
#[test]
fn permission_denial_shapes() {
    on_big_stack(|| async {
        let (shamir, alice) = open_fixture().await;
        let bob = bob_session();
        let handler = ShamirDbHandler::new(Arc::clone(&shamir));

        // Seed one real shape first, so "the denied reset wiped nothing" is
        // observable below.
        assert!(matches!(
            send(&handler, &alice, &seed_req()).await,
            DbResponse::Batch { .. }
        ));

        // (a) Ordinary session: both ops on a REAL repo, on an unknown repo
        // and on an unknown database are all the same typed refusal.
        for req in [
            list_req(DB, REPO, None),
            reset_req(DB, REPO),
            list_req(DB, "no-such-repo", None),
            reset_req(DB, "no-such-repo"),
            list_req("no-such-db", "no-such-repo", None),
            reset_req("no-such-db", "no-such-repo"),
        ] {
            match send(&handler, &alice, &req).await {
                DbResponse::Error { code, message } => {
                    assert_eq!(code, "permission_denied");
                    assert_eq!(message, MANAGE_REQUIRED);
                    assert_refusal_not_path_like(&message);
                    for name in ["no-such-repo", "no-such-db", REPO, TABLE] {
                        assert!(
                            !message.contains(name),
                            "the refusal must not echo a caller-supplied name"
                        );
                    }
                }
                _ => panic!("an ordinary session must be refused for both shape ops"),
            }
        }

        // (b) The denied reset performed no wipe: the seeded shape is still
        // tracked once bob (holding the grant) lists.
        grant_activity_manage(&shamir, &bob).await;
        let before = expect_page(send(&handler, &bob, &list_req(DB, REPO, None)).await);
        assert_eq!(
            before.rows.len(),
            1,
            "the seeded shape must still be tracked"
        );
        assert_eq!(before.tracked, 1);

        // (c) A non-superuser holding ActivityView:Manage is allowed for
        // BOTH ops.
        assert!(matches!(
            send(&handler, &bob, &reset_req(DB, REPO)).await,
            DbResponse::ShapeStatsReset
        ));
        let after = expect_page(send(&handler, &bob, &list_req(DB, REPO, None)).await);
        assert!(after.rows.is_empty());
        assert_eq!(after.tracked, 0);
    });
}

/// End-to-end lifecycle through the real handlers: literal-only traffic
/// folds to exactly ONE row (3 executions, 0 errors, 32 lowercase hex), a
/// second listing is byte-identical (deterministic order), the reset empties
/// rows/`tracked`/`other`, and the audit chain holds exactly ONE reset record
/// carrying the repository name and NOTHING else.
///
/// The setup runs on an audit-LESS handler, which is also the proof that the
/// reset skips the audit append silently when `self.audit` is `None`
/// (otherwise two records would be observed).
///
/// Catches: a shape row per literal instead of per folded shape; a missing
/// `truncated`/`tracked`/`other` reset; a reset that leaves rows behind;
/// deletion of the `enqueue_shape_reset_audit` call (zero records); a reset
/// audited unconditionally even without a writer; and any counter/row/user
/// value added to the audit payload.
#[test]
fn shape_stats_lifecycle_and_single_reset_audit_record() {
    on_big_stack(|| async {
        let (shamir, alice) = open_fixture().await;
        let manager = manager_session();

        let setup = ShamirDbHandler::new(Arc::clone(&shamir));
        assert!(matches!(
            send(&setup, &alice, &seed_req()).await,
            DbResponse::Batch { .. }
        ));
        assert!(matches!(
            send(&setup, &manager, &reset_req(DB, REPO)).await,
            DbResponse::ShapeStatsReset
        ));

        let captured = Arc::new(CapturedAudit::default());
        let writer = Arc::new(AuditChainWriter::new(
            AuditChain::new([0x42; 32]),
            captured.clone(),
        ));
        let handler = ShamirDbHandler::new(Arc::clone(&shamir)).with_audit_writer(writer);

        // Three reads differing only in a literal: ONE shape, 3 executions.
        assert!(matches!(
            send(
                &handler,
                &alice,
                &literal_reads(&["alpha", "beta", "gamma"])
            )
            .await,
            DbResponse::Batch { .. }
        ));

        let page = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert_eq!(page.rows.len(), 1, "literal-only variants share one shape");
        let row = &page.rows[0];
        assert_eq!(row.executions, 3);
        assert_eq!(row.errors, 0);
        assert!(is_lower_hex(&row.shape_hex), "32 lowercase hex characters");
        assert!(page.tracked >= 1);
        assert!(!page.truncated);
        assert_eq!(
            page.other,
            ShapeStatsTotals {
                executions: 0,
                total_nanos: 0,
                errors: 0,
            }
        );

        // No data op ran in between: the page must be identical (total,
        // deterministic order — never hash iteration order).
        let again = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert_eq!(again, page);

        // Reset empties rows, `tracked` and `(other)`.
        assert!(matches!(
            send(&handler, &manager, &reset_req(DB, REPO)).await,
            DbResponse::ShapeStatsReset
        ));
        let after = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert!(after.rows.is_empty(), "a reset leaves no rows");
        assert_eq!(after.tracked, 0);
        assert_eq!(
            after.other,
            ShapeStatsTotals {
                executions: 0,
                total_nanos: 0,
                errors: 0,
            }
        );

        // Exactly ONE reset record; repository name only.
        handler.cancel_refusal_audit().drain().await;
        let entries = captured.0.lock().expect("audit mutex");
        assert_eq!(entries.len(), 1, "exactly one reset audit record");
        let entry = &entries[0];
        assert_eq!(entry.event, "reset_shape_stats");
        assert_eq!(entry.result, "ok");
        assert_eq!(entry.user, manager.username);
        assert_eq!(&entry.session_id_prefix[..], &manager.session_id[..8]);
        let details: serde_json::Value =
            rmp_serde::from_slice(&entry.details_canonical_msgpack).expect("decode details");
        let object = details.as_object().expect("details object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["repo"],
            "the record carries the repository name only"
        );
        assert_eq!(details["repo"].as_str(), Some(REPO));
        let encoded = &entry.details_canonical_msgpack;
        assert!(
            !encoded
                .windows(alice.username.len())
                .any(|w| w == alice.username.as_bytes()),
            "the reset record must not name a user"
        );
    });
}

/// `limit` handling and the total page order: `0` is `invalid_request`
/// (before any lookup), the default renders every row, a limit below the row
/// count truncates and renders exactly the head of the full order, and the
/// counters are the ones the controlled traffic produced. Unknown repository
/// and database keep the stable typed codes; a read-only replica serves both
/// ops (no node-mode gate).
///
/// Catches: a missing `limit: 0` refusal; `truncated` never set; a page
/// sorted by hash iteration (or by `shape_hex` first); a limit that is
/// applied before the sort; an invented error code for an unknown
/// repo/db; and a follower-write refusal for local telemetry.
#[test]
fn shape_stats_limits_order_and_typed_refusals() {
    on_big_stack(|| async {
        let (shamir, alice) = open_fixture().await;
        let manager = manager_session();
        let handler = ShamirDbHandler::new(Arc::clone(&shamir));

        // Setup: intern the fields, then start from an empty registry.
        assert!(matches!(
            send(&handler, &alice, &seed_req()).await,
            DbResponse::Batch { .. }
        ));
        assert!(matches!(
            send(&handler, &manager, &reset_req(DB, REPO)).await,
            DbResponse::ShapeStatsReset
        ));

        assert!(matches!(
            send(&handler, &alice, &traffic_first()).await,
            DbResponse::Batch { .. }
        ));
        assert!(matches!(
            send(&handler, &alice, &traffic_second()).await,
            DbResponse::Batch { .. }
        ));

        // Default limit (None = 50) renders everything here.
        let full = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert_eq!(full.tracked, 4);
        assert_eq!(full.rows.len(), 4);
        assert!(!full.truncated);
        assert!(ordered(&full.rows), "documented total order");
        assert!(full.rows.iter().all(|row| is_lower_hex(&row.shape_hex)));

        // Execution counts are exactly the ones the traffic produced.
        let mut counts: Vec<u64> = full.rows.iter().map(|row| row.executions).collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![1, 1, 2, 3]);

        // A limit under the row count truncates and renders the head of the
        // SAME total order (no data op in between).
        let capped = expect_page(send(&handler, &manager, &list_req(DB, REPO, Some(3))).await);
        assert!(capped.truncated);
        assert_eq!(capped.rows.len(), 3);
        assert_eq!(capped.tracked, 4);
        assert_eq!(capped.rows, full.rows[..3].to_vec());

        // A limit equal to the row count is not a truncation.
        let exact = expect_page(send(&handler, &manager, &list_req(DB, REPO, Some(4))).await);
        assert!(!exact.truncated);
        assert_eq!(exact.rows, full.rows);

        // `limit: 0` is a typed form refusal.
        assert!(matches!(
            send(&handler, &manager, &list_req(DB, REPO, Some(0))).await,
            DbResponse::Error { ref code, .. } if code == "invalid_request"
        ));

        // Unknown repository / database keep the existing stable codes.
        for (req, expected) in [
            (list_req(DB, "no-such-repo", None), "unknown_repo"),
            (reset_req(DB, "no-such-repo"), "unknown_repo"),
            (list_req("no-such-db", REPO, None), "unknown_db"),
            (reset_req("no-such-db", REPO), "unknown_db"),
        ] {
            match send(&handler, &manager, &req).await {
                DbResponse::Error { code, message } => {
                    assert_eq!(code, expected);
                    assert_refusal_not_path_like(&message);
                }
                _ => panic!("expected the stable typed unknown-db/unknown-repo refusal"),
            }
        }

        // No node-mode gate: the registry is local in-memory telemetry and
        // `is_write()` is false for both ops.
        let follower = ShamirDbHandler::new(Arc::clone(&shamir)).with_node_mode(NodeMode::ReadOnly);
        assert!(matches!(
            send(&follower, &manager, &list_req(DB, REPO, None)).await,
            DbResponse::ShapeStatsPage { .. }
        ));
        assert!(matches!(
            send(&follower, &manager, &reset_req(DB, REPO)).await,
            DbResponse::ShapeStatsReset
        ));
    });
}

// ---------------------------------------------------------------------------
// R4b step 4, item 3 follow-up — the reset/cancel race.
//
// `reset_shape_stats` claims the commit slot (`CancelHandle::try_commit`)
// BEFORE the wipe and answers the typed `cancelled` error when the claim is
// lost; the dispatch arm answers the same error when `in_flight.finish()`
// loses afterwards. Both orderings are pinned through the test-only pause
// seam (`with_shape_reset_pause`), never by a sleep: the parked handler runs
// on its OWN current-thread runtime, because the fixture runtime is
// single-threaded and a handler parked inside it would deadlock the very
// request that has to cancel it.
// ---------------------------------------------------------------------------

/// Static `BatchOp::activity_name()` of the reset op — the `op_kind` the
/// activity view carries for the parked reset row.
const RESET_OP_KIND: &str = "admin:reset_shape_stats";

/// One handshake endpoint for the pause seam: the installed closure signals
/// that it has parked, then blocks until it is explicitly released.
struct ParkedPause {
    entered: Option<mpsc::Receiver<()>>,
    release: mpsc::Sender<()>,
}

impl ParkedPause {
    /// Bounded wait for the handler to reach the pause seam.
    async fn wait_entered(&mut self) {
        let entered = self.entered.take().expect("the seam is entered once");
        tokio::time::timeout(
            CALL_TIMEOUT,
            tokio::task::spawn_blocking(move || entered.recv().expect("pause handshake")),
        )
        .await
        .expect("the reset must reach the pause seam within the per-call timeout")
        .expect("pause handshake thread joined");
    }

    /// Let the parked handler continue.
    fn unpark(&self) {
        self.release.send(()).expect("release the parked reset");
    }
}

/// Install a pause that parks only at the requested side of the commit
/// claim (`false` = before `try_commit`, `true` = after it).
fn parking_pause(at_pause_for: bool) -> (Arc<dyn Fn(bool) + Send + Sync>, ParkedPause) {
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    // `mpsc::Sender`/`Receiver` are `Send` but not `Sync`; the seam wants a
    // `Send + Sync` closure, so each endpoint sits behind its own mutex.
    let entered_tx = Mutex::new(entered_tx);
    let release_rx = Mutex::new(release_rx);
    let pause = Arc::new(move |after_claim: bool| {
        if after_claim != at_pause_for {
            return;
        }
        entered_tx
            .lock()
            .expect("pause signal mutex")
            .send(())
            .expect("signal the pause seam");
        release_rx
            .lock()
            .expect("pause release mutex")
            .recv()
            .expect("wait for the release");
    });
    (
        pause,
        ParkedPause {
            entered: Some(entered_rx),
            release: release_tx,
        },
    )
}

/// One `ListActivity` page over the `Requests` registry for every user.
fn activity_req() -> DbRequest {
    let mut b = Batch::new();
    b.op(
        "list_activity",
        shamir_query_builder::wire::admin::list_activity(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::Requests,
            limit: Some(256),
            continuation: None,
        }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.to_owned(),
        batch: b.build(),
    }
}

/// Discover the parked reset's server-minted id from the activity view:
/// bounded retries with `yield_now` (never a sleep), each call bounded by
/// [`CALL_TIMEOUT`].
async fn parked_reset_id(handler: &ShamirDbHandler, manager: &Session) -> RequestExecId {
    const MAX_ROUNDS: usize = 4_096;
    for _ in 0..MAX_ROUNDS {
        match send(handler, manager, &activity_req()).await {
            DbResponse::ActivityPage { page } => {
                for row in page.rows {
                    if let ActivityRow::Request {
                        request_exec_id,
                        op_kind,
                        ..
                    } = row
                    {
                        if op_kind == RESET_OP_KIND {
                            return request_exec_id;
                        }
                    }
                }
            }
            // Never print the response: it may echo request material.
            _ => panic!("expected an ActivityPage while discovering the parked reset"),
        }
        tokio::task::yield_now().await;
    }
    panic!("the parked reset must render as a `{RESET_OP_KIND}` activity row");
}

/// Run one reset on its OWN current-thread runtime (the fixture runtime is
/// single-threaded, so a parked handler inside it would deadlock) and hand
/// back the thread plus the channel its reply arrives on.
fn spawn_parked_reset(
    handler: &ShamirDbHandler,
) -> (std::thread::JoinHandle<()>, mpsc::Receiver<DbResponse>) {
    let handler = handler.clone();
    let (reply_tx, reply_rx) = mpsc::channel::<DbResponse>();
    let parked = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            let manager = manager_session();
            let response = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("parked reset runtime")
                .block_on(send(&handler, &manager, &reset_req(DB, REPO)));
            reply_tx.send(response).expect("deliver the reset reply");
        })
        .expect("spawn the parked reset thread");
    (parked, reply_rx)
}

/// Await the parked reset's reply (bounded) and join its thread.
async fn join_parked_reset(
    parked: std::thread::JoinHandle<()>,
    reply_rx: mpsc::Receiver<DbResponse>,
) -> DbResponse {
    let reply = tokio::time::timeout(
        CALL_TIMEOUT,
        tokio::task::spawn_blocking(move || reply_rx.recv().expect("the parked reset replies")),
    )
    .await
    .expect("the parked reset must reply within the per-call timeout")
    .expect("reset reply thread joined");
    parked.join().expect("parked reset thread joined");
    reply
}

/// A cancellation that wins BEFORE the commit claim leaves the registry
/// untouched and answers the typed `cancelled`: the reset parks at the
/// pre-claim seam, the operator cancels the REAL in-flight request (its id
/// discovered through the activity view), and the released handler must not
/// wipe anything.
///
/// Catches: claiming the commit AFTER the wipe (a cancelled reset would
/// erase the statistics); answering `ShapeStatsReset` after the claim was
/// lost; and a wipe that runs even though the claim/`finish()` lost.
#[test]
fn cancelled_reset_does_not_wipe() {
    on_big_stack(|| async {
        let (shamir, alice) = open_fixture().await;
        let manager = manager_session();

        // Seed real shapes with a plain handler; the registry lives in the
        // engine, so every handler over this fixture shares it.
        let seed = ShamirDbHandler::new(Arc::clone(&shamir));
        assert!(matches!(
            send(&seed, &alice, &seed_req()).await,
            DbResponse::Batch { .. }
        ));

        let (pause, mut parked_pause) = parking_pause(false);
        let handler = ShamirDbHandler::new(Arc::clone(&shamir)).with_shape_reset_pause(Some(pause));

        // Baseline as the superuser manager, before the race touches
        // anything.
        let baseline = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert!(
            !baseline.rows.is_empty(),
            "the seeded shape must be tracked before the race"
        );
        let baseline_executions: u64 = baseline.rows.iter().map(|row| row.executions).sum();

        let (parked, reply_rx) = spawn_parked_reset(&handler);
        parked_pause.wait_entered().await;

        let id = parked_reset_id(&handler, &manager).await;
        let cancelled = tokio::time::timeout(CALL_TIMEOUT, handler.cancel_request(&manager, id))
            .await
            .expect("cancel within the per-call timeout");
        match cancelled {
            DbResponse::CancelOutcome { outcome } => assert_eq!(outcome, CancelOutcome::Cancelled),
            _ => panic!("a pre-claim cancel must win"),
        }

        parked_pause.unpark();
        match join_parked_reset(parked, reply_rx).await {
            DbResponse::Error { code, .. } => assert_eq!(code, error_code::CANCELLED),
            _ => panic!("a cancelled reset must answer the typed `cancelled` error"),
        }

        let after = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert_eq!(
            after.rows, baseline.rows,
            "a cancelled reset must wipe nothing"
        );
        assert_eq!(
            after.rows.iter().map(|row| row.executions).sum::<u64>(),
            baseline_executions,
            "every execution count survives a cancelled reset"
        );
        assert_eq!(after.tracked, baseline.tracked);
    });
}

/// A cancellation that arrives AFTER the commit claim loses: the parked
/// reset answers `AlreadyCommitting`, completes normally, and the registry
/// is empty. The mirror image of [`cancelled_reset_does_not_wipe`].
///
/// Catches: claiming the commit only at the END of the wipe (the cancel
/// would win instead of `AlreadyCommitting`); a reset that answers
/// `cancelled` even though it claimed first; and a wipe that leaves rows,
/// `tracked` or `(other)` behind.
#[test]
fn wiped_reset_refuses_a_late_cancel() {
    on_big_stack(|| async {
        let (shamir, alice) = open_fixture().await;
        let manager = manager_session();

        let seed = ShamirDbHandler::new(Arc::clone(&shamir));
        assert!(matches!(
            send(&seed, &alice, &seed_req()).await,
            DbResponse::Batch { .. }
        ));

        let (pause, mut parked_pause) = parking_pause(true);
        let handler = ShamirDbHandler::new(Arc::clone(&shamir)).with_shape_reset_pause(Some(pause));

        let baseline = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert!(
            !baseline.rows.is_empty(),
            "the seeded shape must be tracked before the race"
        );

        let (parked, reply_rx) = spawn_parked_reset(&handler);
        parked_pause.wait_entered().await;

        let id = parked_reset_id(&handler, &manager).await;
        let refused = tokio::time::timeout(CALL_TIMEOUT, handler.cancel_request(&manager, id))
            .await
            .expect("cancel within the per-call timeout");
        match refused {
            DbResponse::CancelOutcome { outcome } => {
                assert_eq!(outcome, CancelOutcome::AlreadyCommitting)
            }
            _ => panic!("a post-claim cancel must lose the commit race"),
        }

        parked_pause.unpark();
        match join_parked_reset(parked, reply_rx).await {
            DbResponse::ShapeStatsReset => {}
            _ => panic!("a claimed reset must complete and wipe"),
        }

        let after = expect_page(send(&handler, &manager, &list_req(DB, REPO, None)).await);
        assert_ne!(after.rows, baseline.rows, "the claimed reset must wipe");
        assert!(after.rows.is_empty(), "a completed reset leaves no rows");
        assert_eq!(after.tracked, 0);
        assert_eq!(
            after.other,
            ShapeStatsTotals {
                executions: 0,
                total_nanos: 0,
                errors: 0,
            }
        );
    });
}
