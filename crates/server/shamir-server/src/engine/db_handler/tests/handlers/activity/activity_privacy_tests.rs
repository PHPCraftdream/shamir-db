//! Private deterministic privacy test for the R4b-1 activity view
//! (design §6 `activity_privacy_no_literals_or_credentials`, D3).
//!
//! Deterministic hold via the RI-15 byte-budget seam — no race polling,
//! no reliance on the listing's own (excluded) row: the budget is capped
//! at 2048 bytes and the test holds the FULL budget, then starts a
//! canary write batch whose 4096-byte upfront reservation cannot fit the
//! remaining 0. Registration happens BEFORE the budget acquire, so the
//! write stays visible as a `WaitingBudget` in-flight row while parked (one
//! explicit `Poll::Pending` proves the hold). The `ListActivity` page is
//! rendered with NO upfront budget reservation (R4b-1 follow-up): a
//! SECOND stamped session (an admin) lists `scope: All` while the budget
//! is fully saturated by the parked workload, still under the common
//! serialized byte/result limit (`max_result_size` 1024). BOTH the raw
//! response bytes AND the re-encoded page/rows are byte-scanned for the
//! canary literal, the 32-byte bearer sid, its 8-byte prefix and the
//! prefix's hex forms (contrast: the write REQUEST bytes do carry the
//! canary — the scan is sensitive). Dropping the permit lets the write
//! finish; the in-flight mirror returns to zero.

use std::future::{poll_fn, Future};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::dispatch::RequestHandler;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::{ShamirDb, SystemStoreConfig};

use shamir_query_builder::batch::Batch;
use shamir_query_types::admin::activity::{
    ActivityKind, ActivityRequestState, ActivityRow, ActivityScope, CancelOutcome, ListActivityReq,
};
use shamir_query_types::batch::BatchLimits;

use crate::byte_budget::ByteBudget;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::db_handler::DbRequest;
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// Distinctive literal embedded in the held write's query — never a
/// credential, safe to name here.
const CANARY: &str = "PRIVACY-CANARY-9f31c4d7e2b8";
/// The fixture session's bearer sid bytes (distinctive tag prefix).
const SID_TAG: u8 = 0x5A;

/// Hard per-call bound: every await is bounded, never an unbounded
/// engine await.
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

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
    let mut sid = [0u8; 32];
    sid[0] = SID_TAG;
    sid[31] = 0x77;
    s.session_id = sid;
    s
}

/// The SECOND stamped session: an admin, whose `scope: All` listing must
/// succeed while the byte budget is fully saturated by the parked
/// workload (the listing reserves no budget — R4b-1 follow-up).
fn bob_admin_session() -> Session {
    let mut s = Session::new(
        [0xCD; 16],
        "bob".into(),
        SessionPermissions::new(true, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        1_000_000,
    );
    let mut sid = [0u8; 32];
    sid[0] = SID_TAG;
    sid[31] = 0x88;
    s.session_id = sid;
    s
}

async fn fixture(root: &Path) -> (ShamirDbHandler, Arc<Session>, Arc<SessionStore>, ByteBudget) {
    let db = ShamirDb::init(SystemStoreConfig::Fjall(root.join("system")))
        .await
        .expect("init persistent database");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("repos/main")))
            .add_table(TableConfig::new("items")),
        owner.clone(),
    )
    .await
    .expect("add repo");
    let open = shamir_types::access::ResourceMeta::open();
    for path in [
        shamir_types::access::ResourcePath::database("app"),
        shamir_types::access::ResourcePath::store("app", "main"),
        shamir_types::access::ResourcePath::table("app", "main", "items"),
    ] {
        db.set_resource_meta(&path, &open)
            .await
            .expect("open resource");
    }

    // Distinctive bearer sid; the store stamps the minted activity handle.
    let store = Arc::new(SessionStore::new());
    let session = store.insert(alice_session().session_id, alice_session());

    let budget = ByteBudget::new(Some(2048));
    let handler = ShamirDbHandler::new(Arc::new(db))
        .with_byte_budget(budget.clone())
        .with_session_store(Arc::clone(&store));
    (handler, session, store, budget)
}

fn canary_insert(limits: BatchLimits) -> shamir_query_types::batch::BatchRequest {
    let mut b = Batch::new();
    b.limits(limits);
    b.insert(
        "i",
        shamir_query_builder::write::insert("items").rows([shamir_query_builder::doc! {
            "n" => 1,
            "note" => CANARY,
        }]),
    );
    b.build()
}

fn list_req() -> DbRequest {
    let mut b = Batch::new();
    b.limits(BatchLimits {
        // 1024 bytes: the common serialized byte/result limit still bounds
        // the page (enough for the alice and bob session rows plus the
        // held write's Request row). The page makes NO upfront budget
        // reservation, so it is admitted even with the budget fully
        // saturated.
        max_result_size: 1024,
        ..BatchLimits::default()
    });
    b.op(
        "list_activity",
        shamir_query_builder::wire::admin::list_activity(ListActivityReq {
            scope: ActivityScope::All,
            kind: ActivityKind::All,
            limit: Some(64),
            continuation: None,
        }),
    );
    DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: b.build(),
    }
}

/// Encode the request up front (bytes copied into the future) and drive
/// the public `handle` wire entry; returns (decoded, RAW response bytes).
fn call_raw<'a>(
    handler: &'a ShamirDbHandler,
    session: &'a Session,
    req: &DbRequest,
) -> Pin<Box<dyn Future<Output = (DbResponse, Vec<u8>)> + 'a>> {
    let bytes = rmp_serde::to_vec_named(req).expect("encode req");
    Box::pin(async move {
        let raw = handler
            .handle(session, &bytes, &ConnectionServices::without_push(0))
            .await
            .expect("handle request");
        let decoded: DbResponse = rmp_serde::from_slice(&raw).expect("decode DbResponse");
        (decoded, raw)
    })
}

fn forbidden_needles(session: &Session) -> Vec<Vec<u8>> {
    let sid = session.session_id;
    vec![
        CANARY.as_bytes().to_vec(),
        sid.to_vec(),
        sid[..8].to_vec(),
        hex::encode(&sid[..8]).into_bytes(),
        hex::encode_upper(&sid[..8]).into_bytes(),
    ]
}

fn assert_absent(label: &str, bytes: &[u8], needles: &[Vec<u8>]) {
    for (i, needle) in needles.iter().enumerate() {
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle.as_slice()),
            "{label}: activity bytes contain forbidden sequence #{i}"
        );
    }
}

// Existing server fixture convention: deep engine futures need a roomy stack.
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

#[test]
fn activity_privacy_no_literals_or_credentials() {
    let dir = tempfile::tempdir().expect("create persistent fixture directory");
    let root = dir.path().to_path_buf();
    on_big_stack(move || async move {
        let (handler, session, store, budget) = tokio::time::timeout(CALL_TIMEOUT, fixture(&root))
            .await
            .expect("initialize fixture");

        // Hold the FULL 2048-byte cap: the canary write's reservation
        // cannot fit zero remaining bytes, and NOTHING budget-reserving
        // can proceed until the permit drops.
        let hog = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(2048))
            .await
            .expect("reserve the whole byte budget");

        // Start the canary write; its 4096-byte reservation cannot fit the
        // remaining 0 (budget fully held), so it parks AT the budget
        // acquire — AFTER in-flight registration, i.e. deterministically
        // visible as WaitingBudget.
        let write_req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: canary_insert(BatchLimits {
                max_result_size: 4096,
                ..BatchLimits::default()
            }),
        };
        let write_bytes = rmp_serde::to_vec_named(&write_req).expect("encode write");
        // Contrast control, taken BEFORE the bytes move into the future:
        // the WRITE REQUEST bytes do carry the canary — the scans below
        // are sensitive to exactly this sequence.
        assert!(
            write_bytes
                .windows(CANARY.len())
                .any(|w| w == CANARY.as_bytes()),
            "internal: canary control sequence missing from the write request"
        );
        let handler_ref = &handler;
        let session_ref = &*session;
        let mut write = Box::pin(async move {
            let raw = handler_ref
                .handle(
                    session_ref,
                    &write_bytes,
                    &ConnectionServices::without_push(0),
                )
                .await
                .expect("handle write");
            let decoded: DbResponse = rmp_serde::from_slice(&raw).expect("decode write response");
            decoded
        });
        // One explicit poll proves the write is parked (held), not done.
        poll_fn(|cx| {
            assert!(matches!(write.as_mut().poll(cx), Poll::Pending));
            Poll::Ready(())
        })
        .await;
        assert_eq!(
            handler.activity_requests.in_flight(),
            1,
            "the held write must be the single in-flight entry"
        );

        // The listing is issued by the SECOND stamped session (an admin,
        // `scope: All`) over its own `ConnectionServices` and makes NO
        // upfront budget reservation, so it proceeds while the budget is
        // FULLY saturated by the held write. Raw response bytes are
        // retained as the privacy witness. Bounded like every other
        // await in this test.
        let admin = store.insert(bob_admin_session().session_id, bob_admin_session());
        assert_ne!(admin.activity_handle(), 0, "admin session is stamped");
        let (listing, listing_raw) =
            tokio::time::timeout(CALL_TIMEOUT, call_raw(&handler, &admin, &list_req()))
                .await
                .expect("listing completes within the per-call timeout");
        let page = match listing {
            DbResponse::ActivityPage { page } => page,
            // Never print the non-matching response: it may echo request
            // material (literals) and this is a privacy test.
            _ => panic!("expected ActivityPage"),
        };

        // Positive live marker: exactly the held write is visible,
        // correlated to the minted session handle, in state WaitingBudget.
        let request_rows: Vec<&ActivityRow> = page
            .rows
            .iter()
            .filter(|r| matches!(r, ActivityRow::Request { .. }))
            .collect();
        assert_eq!(
            request_rows.len(),
            1,
            "exactly the held write must appear as a Request row"
        );
        if let ActivityRow::Request {
            request_exec_id: _,
            session_handle,
            user,
            op_kind,
            state,
            ..
        } = request_rows[0]
        {
            assert_eq!(*session_handle, Some(session.activity_handle()));
            assert_eq!(user, "alice");
            assert_eq!(op_kind, "write", "op kind is the static discriminant name");
            assert!(matches!(state, ActivityRequestState::WaitingBudget));
        }

        // Privacy byte scan: BOTH the raw wire response bytes AND the
        // re-encoded page/rows carry neither the canary literal nor
        // bearer material.
        let needles = forbidden_needles(&session);
        assert_absent("raw listing response", &listing_raw, &needles);
        let encoded = rmp_serde::to_vec_named(&page).expect("encode page");
        assert_absent("encoded page", &encoded, &needles);
        for row in &page.rows {
            let row_bytes = rmp_serde::to_vec_named(row).expect("encode row");
            assert_absent("encoded row", &row_bytes, &needles);
        }
        // Release the permit: the write proceeds and completes; the RAII
        // guard removes the in-flight entry and the mirror returns to zero.
        // The await is bounded — a stuck write fails the timeout instead
        // of hanging the test.
        drop(hog);
        let write_done = tokio::time::timeout(CALL_TIMEOUT, write)
            .await
            .expect("held write completes within the per-call timeout");
        match write_done {
            DbResponse::Batch { .. } => {}
            // Response bodies can echo query literals — never print them.
            _ => panic!("held write must complete as a Batch after permit release"),
        }
        assert_eq!(
            handler.activity_requests.in_flight(),
            0,
            "in-flight mirror must return to zero after completion"
        );
    });
}

#[test]
fn budget_parked_target_keeps_cancelled_error_at_tiny_result_cap() {
    let dir = tempfile::tempdir().expect("create persistent fixture directory");
    let root = dir.path().to_path_buf();
    on_big_stack(move || async move {
        let (handler, session, store, budget) = tokio::time::timeout(CALL_TIMEOUT, fixture(&root))
            .await
            .expect("initialize fixture");
        let hog = tokio::time::timeout(CALL_TIMEOUT, budget.acquire(2048))
            .await
            .expect("reserve the whole byte budget");
        let target_req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: canary_insert(BatchLimits {
                max_result_size: 1,
                ..BatchLimits::default()
            }),
        };
        let mut target = call_raw(&handler, &session, &target_req);
        poll_fn(|cx| {
            assert!(target.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        let admin = store.insert(bob_admin_session().session_id, bob_admin_session());
        let (listing, _) =
            tokio::time::timeout(CALL_TIMEOUT, call_raw(&handler, &admin, &list_req()))
                .await
                .expect("list requests while the budget is held");
        let DbResponse::ActivityPage { page } = listing else {
            panic!("expected ActivityPage");
        };
        let id = page
            .rows
            .iter()
            .find_map(|row| match row {
                ActivityRow::Request {
                    request_exec_id,
                    state: ActivityRequestState::WaitingBudget,
                    session_handle: Some(handle),
                    ..
                } if *handle == session.activity_handle() => Some(*request_exec_id),
                _ => None,
            })
            .expect("target is parked on the byte budget");
        let mut batch = Batch::new();
        batch.limits(BatchLimits {
            max_result_size: 1,
            ..BatchLimits::default()
        });
        batch.op(
            "cancel_request",
            shamir_query_builder::wire::admin::cancel_request(id),
        );
        let cancel_req = DbRequest::Execute {
            query_version: CURRENT_QUERY_LANG_VERSION,
            db: "app".into(),
            batch: batch.build(),
        };
        let ((reply, raw), (outcome, _)) = tokio::time::timeout(CALL_TIMEOUT, async {
            tokio::join!(target, call_raw(&handler, &admin, &cancel_req))
        })
        .await
        .expect("cancel wakes the target before budget is released");
        assert!(matches!(
            outcome,
            DbResponse::CancelOutcome {
                outcome: CancelOutcome::Cancelled,
            }
        ));
        assert!(matches!(
            reply,
            DbResponse::Error { code, message }
                if code == "cancelled" && message == "request cancelled"
        ));
        assert!(
            raw.len() > 1,
            "the cancellation envelope exceeds the target cap"
        );
        assert_eq!(handler.activity_requests.in_flight(), 0);
        drop(hog);
    });
}
