//! R4b-4 — every end of an interactive transaction releases the Level-3
//! pessimistic record locks it still holds.
//!
//! `shamir_tx::TxContext` has no `Drop` that frees them (they live in the
//! per-table `MvccStore`), so `InteractiveTx::drop` spawns the release once the
//! LAST reference to the parked transaction goes away. Each test parks a real
//! waiter on a record lock and requires it to proceed after one end path:
//! owner rollback, the reaper, session close, or a request still in flight at
//! session close. On the parent commit (no `Drop` release) every waiter hangs
//! and the test fails.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::storage::types::RecordKey;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::{Batch, Isolation};
use shamir_query_builder::wire::admin::cancel_tx;
use shamir_query_builder::{doc, filter, write};
use shamir_query_types::admin::activity::CancelOutcome;
use shamir_query_types::batch::IsolationLevel;
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_sync::durability::tx_cancel::TxParkedState;
use shamir_test_diag::FlakeGuard;
use shamir_tx::{LockMode, MvccStore, RepoTxGate};
use shamir_types::types::record_id::RecordId;
use tokio::sync::Notify;
use tokio_util::task::AbortOnDropHandle;

use crate::db_handler::handler::ShamirDbHandler;
use crate::security::http_rest::NativeDbHandler;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const TIMEOUT: Duration = Duration::from_secs(120);
const DB: &str = "app";
const REPO: &str = "main";
const TABLE: &str = "items";

fn session(user: u8, sid: u8, superuser: bool) -> Session {
    let mut session = Session::new(
        [user; 16],
        "same-display-name".to_owned(),
        SessionPermissions::new(superuser, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0; 32],
        1_000,
    );
    session.session_id = [sid; 32];
    session
}

async fn open_app_db(owner: &Actor) -> Arc<ShamirDb> {
    let db = ShamirDb::init_memory().await.expect("in-memory database");
    db.create_db_as(DB, owner.clone()).await.expect("create db");
    db.add_repo_as(
        DB,
        RepoConfig::new(REPO, BoxRepoFactory::in_memory()).add_table(TableConfig::new(TABLE)),
        owner.clone(),
    )
    .await
    .expect("add repo");
    Arc::new(db)
}

async fn wire_request(
    handler: &ShamirDbHandler,
    session: &Session,
    request: DbRequest,
) -> DbResponse {
    let bytes = rmp_serde::to_vec_named(&request).expect("encode request");
    let raw = handler
        .handle(session, &bytes, &ConnectionServices::without_push(0))
        .await
        .expect("handle request");
    rmp_serde::from_slice(&raw).expect("decode reply")
}

fn execute(batch: shamir_query_builder::BatchRequest) -> DbRequest {
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        batch,
    }
}

fn pessimistic_begin() -> DbRequest {
    DbRequest::TxBegin {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        repo: REPO.into(),
        isolation: Some(IsolationLevel::Pessimistic),
    }
}

fn update_where(where_n: i64, set_n: i64) -> shamir_query_types::write::UpdateOp {
    write::Update::with_repo(REPO, TABLE)
        .where_(filter::eq("n", where_n))
        .set(doc! { "n" => set_n })
        .build()
        .expect("build fixture update")
}

fn stage_update(tx_handle: u64, where_n: i64, set_n: i64) -> DbRequest {
    let mut batch = Batch::new();
    batch.update("staged", update_where(where_n, set_n));
    DbRequest::TxExecute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: DB.into(),
        tx_handle,
        batch: batch.build(),
    }
}

fn blocked_update(n: i64) -> DbRequest {
    let mut batch = Batch::new();
    batch.transactional().isolation(Isolation::Pessimistic);
    batch.update("blocked", update_where(n, -n));
    execute(batch.build())
}

async fn storage(handler: &ShamirDbHandler) -> (Arc<RepoTxGate>, Arc<MvccStore>) {
    let db = handler.db().get_db(DB).expect("fixture db");
    let table = db.get_table(REPO, TABLE).await.expect("open table");
    let repo = db.get_repo(REPO).expect("fixture repo");
    let gate = repo.tx_gate().await.expect("gate");
    (gate, table.mvcc_store_for_test().expect("MVCC table"))
}

async fn seed_row(handler: &ShamirDbHandler, admin: &Session, n: i64) -> RecordKey {
    let mut batch = Batch::new();
    batch.id("ins");
    batch.insert("i", write::insert(TABLE).rows([doc! { "n" => n }]));
    let response = wire_request(handler, admin, execute(batch.build())).await;
    let DbResponse::Batch { response } = response else {
        panic!("seed row: {response:?}");
    };
    let id: RecordId = response.results["i"].records[0]
        .get_value_str("_id")
        .expect("inserted id")
        .parse()
        .expect("RecordId");
    RecordKey::from_slice(id.as_bytes())
}

async fn hold(mvcc: &MvccStore, gate: &RepoTxGate, keys: &[RecordKey]) -> u64 {
    let older = gate.fresh_tx_id().raw();
    for key in keys {
        mvcc.lock_key(
            key.clone(),
            older,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
            LockMode::Exclusive,
        )
        .await
        .expect("hold the seeded key");
    }
    older
}

async fn begin_locked(handler: &ShamirDbHandler, owner: &Session, set_n: i64) -> u64 {
    let handle = match wire_request(handler, owner, pessimistic_begin()).await {
        DbResponse::TxOpened { tx_handle, .. } => tx_handle,
        other => panic!("pessimistic begin: {other:?}"),
    };
    let staged = wire_request(handler, owner, stage_update(handle, 1, set_n)).await;
    assert!(
        matches!(&staged, DbResponse::TxBatch { .. }),
        "the staged pessimistic update must accumulate: {staged:?}"
    );
    handle
}

fn spawn_waiter(
    handler: &Arc<ShamirDbHandler>,
    waiter: Session,
    n: i64,
) -> AbortOnDropHandle<DbResponse> {
    let handler = Arc::clone(handler);
    AbortOnDropHandle::new(tokio::spawn(async move {
        wire_request(&handler, &waiter, blocked_update(n)).await
    }))
}

async fn expect_waiter_proceeds(pending: AbortOnDropHandle<DbResponse>) {
    let response = tokio::time::timeout(TIMEOUT, pending)
        .await
        .expect("the waiter parked on the transaction's record lock must wake")
        .expect("waiter task joined");
    let DbResponse::Batch { response } = response else {
        panic!("the unblocked update must complete normally: {response:?}");
    };
    assert!(
        response.results.contains_key("blocked"),
        "the unblocked update must return its results: {response:?}"
    );
}

/// Wait, without sleeping, until every record lock of the finished transaction
/// is gone. The release runs as a DETACHED task (`spawn_abort_release`), so a
/// state read taken straight after the drop can precede it on a current-thread
/// runtime and merely race it on a multi-thread one: the lock count is an event
/// to wait for here, never a value to read once.
async fn wait_for_lock_release(mvcc: &MvccStore, baseline: usize) {
    let released = tokio::time::timeout(TIMEOUT, async {
        loop {
            if mvcc.locks_len() == baseline {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    assert!(released, "every record lock is released");
}

async fn assert_quiet(
    handler: &ShamirDbHandler,
    mvcc: &MvccStore,
    gate: &RepoTxGate,
    baseline: usize,
) {
    wait_for_lock_release(mvcc, baseline).await;
    assert!(
        handler.tx_registry().is_empty(),
        "no parked transaction survives"
    );
    assert!(
        gate.active_snapshots_empty(),
        "the snapshot pin is released"
    );
}

/// Owner `TxRollback`: the parked transaction leaves the registry and its
/// record lock must be freed, so the waiter proceeds. Kills the mutant that
/// keeps the parent behaviour (drop frees no Level-3 lock).
#[tokio::test]
async fn owner_rollback_releases_record_locks() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard = FlakeGuard::with_watchdog("owner_rollback_releases_record_locks", TIMEOUT);
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let _key = seed_row(&handler, &admin, 1).await;
            let baseline = mvcc.locks_len();
            let writer = session(1, 1, true);
            let handle = begin_locked(&handler, &writer, 5).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the staged pessimistic write holds the row's record lock"
            );
            guard.mark("interactive tx holds the row lock");
            let pending = spawn_waiter(&handler, session(2, 2, true), 1);
            tokio::task::yield_now().await;
            let rolled = wire_request(
                &handler,
                &writer,
                DbRequest::TxRollback {
                    db: DB.into(),
                    tx_handle: handle,
                },
            )
            .await;
            assert!(
                matches!(&rolled, DbResponse::TxRolledBack { tx_handle } if *tx_handle == handle),
                "rollback: {rolled:?}"
            );
            guard.mark("owner rolled back");
            expect_waiter_proceeds(pending).await;
            assert_quiet(&handler, &mvcc, &gate, baseline).await;
            guard.mark("waiter proceeded");
        }),
    )
    .await
    .expect("rollback scenario within timeout");
}

/// The reaper's removal (`reap_expired`) must free the reaped transaction's
/// record locks. Kills the mutant that releases only on the explicit paths.
#[tokio::test]
async fn reaper_releases_record_locks() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard = FlakeGuard::with_watchdog("reaper_releases_record_locks", TIMEOUT);
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let _key = seed_row(&handler, &admin, 1).await;
            let baseline = mvcc.locks_len();
            let writer = session(1, 1, true);
            let _handle = begin_locked(&handler, &writer, 5).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the staged write holds the lock"
            );
            guard.mark("interactive tx holds the row lock");
            let pending = spawn_waiter(&handler, session(2, 2, true), 1);
            tokio::task::yield_now().await;
            assert_eq!(
                handler.tx_registry().reap_expired(Duration::ZERO),
                1,
                "a zero idle TTL expires the idle transaction"
            );
            guard.mark("reaper removed the transaction");
            expect_waiter_proceeds(pending).await;
            assert_quiet(&handler, &mvcc, &gate, baseline).await;
            guard.mark("waiter proceeded");
        }),
    )
    .await
    .expect("reaper scenario within timeout");
}

/// Session close (`cleanup_session`) must free the record locks of the
/// transaction it removes. Kills the mutant that releases only on rollback.
#[tokio::test]
async fn session_close_releases_record_locks() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard = FlakeGuard::with_watchdog("session_close_releases_record_locks", TIMEOUT);
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let _key = seed_row(&handler, &admin, 1).await;
            let baseline = mvcc.locks_len();
            let writer = session(1, 1, true);
            let _handle = begin_locked(&handler, &writer, 5).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the staged write holds the lock"
            );
            guard.mark("interactive tx holds the row lock");
            let pending = spawn_waiter(&handler, session(2, 2, true), 1);
            tokio::task::yield_now().await;
            NativeDbHandler::cleanup_session(&*handler, &writer.session_id);
            assert!(
                handler.tx_registry().is_empty(),
                "session close removes the handle"
            );
            guard.mark("session closed");
            expect_waiter_proceeds(pending).await;
            assert_quiet(&handler, &mvcc, &gate, baseline).await;
            guard.mark("waiter proceeded");
        }),
    )
    .await
    .expect("session-close scenario within timeout");
}

/// A `TxExecute` still holding the context when the session closes: the
/// registry entry disappears but the request owns an `Arc`, so the release can
/// only happen when that last reference drops. Kills the mutant that wires the
/// release into the removal path only.
#[tokio::test]
async fn request_in_flight_when_the_session_closes_still_releases() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard = FlakeGuard::with_watchdog(
                "request_in_flight_when_the_session_closes_still_releases",
                TIMEOUT,
            );
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let first = seed_row(&handler, &admin, 1).await;
            let _second = seed_row(&handler, &admin, 2).await;
            let baseline = mvcc.locks_len();
            let older = hold(&mvcc, &gate, std::slice::from_ref(&first)).await;
            let writer = session(1, 1, true);
            let handle = match wire_request(&handler, &writer, pessimistic_begin()).await {
                DbResponse::TxOpened { tx_handle, .. } => tx_handle,
                other => panic!("pessimistic begin: {other:?}"),
            };
            let staged = wire_request(&handler, &writer, stage_update(handle, 2, 20)).await;
            assert!(
                matches!(&staged, DbResponse::TxBatch { .. }),
                "staged: {staged:?}"
            );
            assert!(
                mvcc.locks_len() > baseline,
                "the staged write holds row 2's lock"
            );
            let pending = spawn_waiter(&handler, session(2, 2, true), 2);
            tokio::task::yield_now().await;
            let parked = match handler.tx_registry().peek(handle) {
                TxParkedState::Open(tx) => tx,
                other => panic!(
                    "the transaction must be parked: {}",
                    matches!(other, TxParkedState::Gone)
                ),
            };
            let in_flight = {
                let handler = Arc::clone(&handler);
                let writer_owned = session(1, 1, true);
                AbortOnDropHandle::new(tokio::spawn(async move {
                    wire_request(&handler, &writer_owned, stage_update(handle, 1, 10)).await
                }))
            };
            tokio::time::timeout(TIMEOUT, async {
                loop {
                    if parked.ctx().try_lock().is_err() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the in-flight TxExecute must hold the transaction's context lock");
            // The probe clone must go: the in-flight request is meant to be the
            // last owner of the transaction when the session closes.
            drop(parked);
            guard.mark("a request is in flight inside the transaction");
            NativeDbHandler::cleanup_session(&*handler, &writer.session_id);
            assert!(
                handler.tx_registry().is_empty(),
                "session close removes the handle"
            );
            assert!(
                !pending.is_finished(),
                "the waiter cannot proceed while the in-flight request still owns the transaction"
            );
            guard.mark("request holds the transaction past session close");
            mvcc.release_locks(older, std::slice::from_ref(&first))
                .await;
            let finished = tokio::time::timeout(TIMEOUT, in_flight)
                .await
                .expect("the in-flight request must finish after its blocker releases")
                .expect("in-flight task joined");
            assert!(
                matches!(&finished, DbResponse::TxBatch { .. }),
                "in-flight request: {finished:?}"
            );
            guard.mark("the in-flight request returned and dropped the last reference");
            expect_waiter_proceeds(pending).await;
            assert_quiet(&handler, &mvcc, &gate, baseline).await;
            guard.mark("waiter proceeded");
        }),
    )
    .await
    .expect("in-flight scenario within timeout");
}

/// Cancel (which releases synchronously) followed by the drop, and a rollback
/// that loses the race with the reaper, release nothing twice. Kills the
/// mutant that frees a key twice or frees a later holder.
#[tokio::test]
async fn a_repeated_release_is_harmless() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard = FlakeGuard::with_watchdog("a_repeated_release_is_harmless", TIMEOUT);
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let _key = seed_row(&handler, &admin, 1).await;
            let baseline = mvcc.locks_len();
            let writer = session(1, 1, true);
            let handle = begin_locked(&handler, &writer, 5).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the staged write holds the lock"
            );
            let mut batch = Batch::new();
            batch.op("cancel", cancel_tx(handle));
            match wire_request(&handler, &writer, execute(batch.build())).await {
                DbResponse::CancelOutcome { outcome } => {
                    assert_eq!(
                        outcome,
                        CancelOutcome::Cancelled,
                        "cancel the idle transaction"
                    )
                }
                other => panic!("cancel_tx: {other:?}"),
            }
            assert_eq!(
                mvcc.locks_len(),
                baseline,
                "the cancel releases the record lock"
            );
            if let TxParkedState::Open(tx) = handler.tx_registry().peek(handle) {
                drop(tx);
            }
            assert!(matches!(
                handler.tx_registry().peek(handle),
                TxParkedState::Gone
            ));
            assert_eq!(
                mvcc.locks_len(),
                baseline,
                "the drop releases nothing twice"
            );
            let late = wire_request(
                &handler,
                &writer,
                DbRequest::TxRollback {
                    db: DB.into(),
                    tx_handle: handle,
                },
            )
            .await;
            assert!(
                matches!(&late, DbResponse::Error { code, .. } if code == "tx_not_found"),
                "a rollback after the removal is refused: {late:?}"
            );
            assert_eq!(
                mvcc.locks_len(),
                baseline,
                "the refused rollback releases nothing"
            );

            let second = begin_locked(&handler, &writer, 6).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the second transaction holds the lock"
            );
            assert_eq!(handler.tx_registry().reap_expired(Duration::ZERO), 1);
            let raced = wire_request(
                &handler,
                &writer,
                DbRequest::TxRollback {
                    db: DB.into(),
                    tx_handle: second,
                },
            )
            .await;
            assert!(
                matches!(&raced, DbResponse::Error { code, .. } if code == "tx_not_found"),
                "a rollback that lost the race with the reaper is refused: {raced:?}"
            );
            assert_quiet(&handler, &mvcc, &gate, baseline).await;
            guard.mark("neither repeated release freed anything");
        }),
    )
    .await
    .expect("repeated-release scenario within timeout");
}

/// The release is keyed by the transaction's own id, so a late release never
/// frees a record that a LATER transaction has since acquired. Kills the
/// mutant that clears every holder on the transaction's keys.
#[tokio::test]
async fn a_late_release_never_frees_a_reacquired_lock() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let guard =
                FlakeGuard::with_watchdog("a_late_release_never_frees_a_reacquired_lock", TIMEOUT);
            let owner = Actor::User(principal64([7; 16]));
            let db = open_app_db(&owner).await;
            let handler = Arc::new(ShamirDbHandler::new(Arc::clone(&db)));
            let (gate, mvcc) = storage(&handler).await;
            let admin = session(9, 9, true);
            let key = seed_row(&handler, &admin, 1).await;
            let baseline = mvcc.locks_len();
            let writer = session(1, 1, true);
            let handle = begin_locked(&handler, &writer, 5).await;
            assert!(
                mvcc.locks_len() > baseline,
                "the staged write holds the lock"
            );
            let parked = match handler.tx_registry().peek(handle) {
                TxParkedState::Open(tx) => tx,
                _ => panic!("the transaction must be parked"),
            };
            let parked_ctx = parked
                .ctx()
                .lock()
                .await
                .take()
                .expect("the parked context must still be there");
            let repo = handler
                .db()
                .get_db(DB)
                .expect("fixture db")
                .get_repo(REPO)
                .expect("fixture repo");
            shamir_db::engine::tx::release_interactive_tx_locks(&parked_ctx, repo.core()).await;
            assert_eq!(
                mvcc.locks_len(),
                baseline,
                "the transaction's own release frees the lock"
            );
            guard.mark("the first release freed the record");
            let newer = hold(&mvcc, &gate, std::slice::from_ref(&key)).await;
            assert_eq!(
                mvcc.locks_len(),
                baseline + 1,
                "a later transaction holds the record"
            );
            shamir_db::engine::tx::release_interactive_tx_locks(&parked_ctx, repo.core()).await;
            assert_eq!(
                mvcc.locks_len(),
                baseline + 1,
                "a late release must not free another transaction's lock"
            );
            guard.mark("the late release left the later holder alone");
            mvcc.release_locks(newer, &[key]).await;
            assert_eq!(mvcc.locks_len(), baseline);
            drop(parked);
        }),
    )
    .await
    .expect("late-release scenario within timeout");
}
