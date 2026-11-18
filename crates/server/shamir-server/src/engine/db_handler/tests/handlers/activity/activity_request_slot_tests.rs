//! Private tests for the per-session in-flight request-slot cap
//! (R4b-1 follow-up): `Session::try_acquire_request_slot` under
//! `QueryLimitsCap::max_in_flight_requests_per_session`, enforced by
//! `ShamirDbHandler::try_track_activity_request` BEFORE execution and the
//! byte-budget wait on every tracked path (execute, TxBegin, TxExecute).
//!
//! Covered here, deterministically (no sleeps, no timing assumptions):
//! - concurrent cap: a start barrier races several workers against ONE
//!   stamped session; exactly `cap` are admitted while the rest are
//!   refused (asserted on the outcome SET, never the join order), and a
//!   second stamped session through the same handler/store is unaffected;
//! - the stateless handle-0 fallback skips the cap (stays admitted while
//!   a stamped session is saturated);
//! - RAII release: an early return and an unwinding panic both free the
//!   slot for the next admission of the SAME session;
//! - same-bearer replacement witness: a re-admitted generation with the
//!   same user + sid INHERITS the shared in-flight counter, so it cannot
//!   raise its concurrency while a permit is held;
//! - a real write from an exhausted session is refused with the fixed
//!   `server_busy` code/message BEFORE any side effect (row count proves
//!   the refused write never landed).
//!
//! Slot permits are owned RAII values on the session (`SessionRequestSlot`,
//! re-exported at `shamir_connect::server::SessionRequestSlot`); the
//! in-flight guard owns the permit until drop, so the transient-permit
//! drop test is simply "guard dropped → slot re-acquirable".

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Barrier};

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};

use crate::engine::db_handler::activity::InFlightRequest;

use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::query::Query;
use shamir_query_builder::write::insert;

use crate::db_handler::config::QueryLimitsCap;
use crate::db_handler::handler::{DbResponse, ShamirDbHandler};
use crate::version::CURRENT_QUERY_LANG_VERSION;

/// Small nonzero cap so the saturation is reachable with a handful of
/// guards (the config layer rejects 0, and the internal test convention
/// is nonzero values only).
const CAP: usize = 2;

/// A stamped session: inserted through a [`SessionStore`], which mints a
/// nonzero activity handle (the precondition for cap enforcement). The
/// session id (bearer token) is derived deterministically and non-secretly
/// from the user id (`[user[0]; 32]`) so distinct users NEVER share a sid —
/// a shared sid would make the second `insert` a same-sid REPLACEMENT of
/// the first user's live session instead of an independent one.
fn stamped_session(store: &SessionStore, name: &str, user: [u8; 16]) -> Arc<Session> {
    let session = Session::new(
        user,
        name.to_string(),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [user[0]; 32],
        1_000,
    );
    let stamped = store.insert(session.session_id, session);
    assert_ne!(
        stamped.activity_handle(),
        0,
        "store-inserted sessions must carry a minted (stamped) handle"
    );
    stamped
}

/// An un-stamped session: never touches a store, so its activity handle
/// is the documented handle-0 stateless sentinel.
fn stateless_session() -> Session {
    Session::new(
        [7u8; 16],
        "stateless".to_string(),
        SessionPermissions::new(false, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0u8; 32],
        1_000,
    )
}

async fn fixture_with_cap() -> (ShamirDbHandler, Arc<SessionStore>) {
    let db = ShamirDb::init_memory().await.expect("init memory database");
    let handler = ShamirDbHandler::new(Arc::new(db)).with_query_limits(QueryLimitsCap {
        max_in_flight_requests_per_session: CAP,
        ..QueryLimitsCap::UNLIMITED
    });
    let store = Arc::new(SessionStore::new());
    (handler, store)
}

async fn write_fixture() -> (ShamirDbHandler, Arc<Session>, Arc<SessionStore>) {
    let db = ShamirDb::init_memory().await.expect("init memory database");
    let owner = Actor::User(principal64([0xAB; 16]));
    db.create_db_as("app", owner.clone())
        .await
        .expect("create_db");
    db.add_repo_as(
        "app",
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("items")),
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
    let store = Arc::new(SessionStore::new());
    let alice = stamped_session(&store, "alice", [0xAB; 16]);
    let handler = ShamirDbHandler::new(Arc::new(db))
        .with_query_limits(QueryLimitsCap {
            max_in_flight_requests_per_session: 1,
            ..QueryLimitsCap::UNLIMITED
        })
        .with_session_store(Arc::clone(&store));
    (handler, alice, store)
}

fn insert_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.insert(
        "i1",
        insert("items").row(doc! { "id" => "k1", "v" => 1_i64 }),
    );
    b.build()
}

fn read_batch() -> shamir_query_builder::BatchRequest {
    let mut b = Batch::new();
    b.query("r1", Query::from("items"));
    b.build()
}

/// Existing server fixture convention: deep engine futures need a roomy
/// stack; one runtime per test, every await bounded by construction here
/// (none of the awaits below can block on external I/O or a held lock).
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

/// Concurrent workers racing one session (cap per test); all released on
/// a barrier so the outcomes genuinely race instead of sequencing.
const RACING_WORKERS: usize = 8;

#[test]
fn concurrent_admission_admits_exactly_cap_and_other_session_unaffected() {
    on_big_stack(|| async {
        let (handler, store) = fixture_with_cap().await;
        let alice = stamped_session(&store, "alice", [1u8; 16]);
        let bob = stamped_session(&store, "bob", [2u8; 16]);

        // All RACING_WORKERS start on the barrier and race the SAME real
        // stamped session; each HOLDS its guard (if admitted) and hands it
        // back as `Option<InFlightRequest>`, so nothing releases before
        // every outcome has been observed. The claim is the invariant the
        // cap must enforce: exactly `cap` successes, the rest refusals —
        // never a join order or a particular worker's result.
        let outcomes: Vec<Option<InFlightRequest>> = std::thread::scope(|scope| {
            let handler = &handler;
            let alice = &*alice;
            let start = Arc::new(Barrier::new(RACING_WORKERS));
            let handles: Vec<_> = (0..RACING_WORKERS)
                .map(|_| {
                    let start = Arc::clone(&start);
                    scope.spawn(move || {
                        start.wait();
                        handler
                            .try_track_activity_request(alice, "app", "read", None)
                            .ok()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("racing worker must not panic"))
                .collect()
        });
        let admitted = outcomes.iter().filter(|o| o.is_some()).count();
        assert_eq!(
            admitted, CAP,
            "exactly the cap may be admitted concurrently, however they race"
        );
        assert_eq!(
            outcomes.iter().filter(|o| o.is_none()).count(),
            RACING_WORKERS - CAP,
            "every remaining racing worker must be refused"
        );
        assert_eq!(handler.activity_requests.in_flight(), CAP);

        // A second stamped session (distinct user AND sid) through the SAME
        // handler is unaffected while all of alice's slots are still held.
        let bob_guard = handler
            .try_track_activity_request(&bob, "app", "read", None)
            .expect("bob must be unaffected by alice's saturation");
        assert_eq!(handler.activity_requests.in_flight(), CAP + 1);

        // Release EVERYTHING; alice's full cap is re-acquirable.
        drop(outcomes);
        drop(bob_guard);
        let g1 = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("alice slot re-acquirable after the racing guards release");
        let g2 = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("alice slot 2 re-acquirable after the racing guards release");
        drop((g1, g2));
        assert_eq!(
            handler.activity_requests.in_flight(),
            0,
            "every guard drop releases its entry and its slot"
        );
    });
}

/// Same-bearer replacement witness (R4b-1 follow-up): a re-admitted
/// generation with the SAME user AND sid (a resume/re-login) must inherit
/// the predecessor's shared in-flight counter (`admit` swaps it in before
/// publication), so a replacement can never raise its effective
/// concurrency while an old-generation permit is still held.
#[test]
fn same_bearer_replacement_gets_no_additional_slot_while_permit_is_held() {
    on_big_stack(|| async {
        let (handler, store) = fixture_with_cap().await;
        let user = [5u8; 16];
        let alice = stamped_session(&store, "alice", user);

        // Hold the FULL cap on the first generation so the replacement
        // genuinely races a saturated shared counter.
        let held_a = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("first-generation slot A");
        let held_b = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("first-generation slot B");
        assert_eq!(
            handler.activity_requests.in_flight(),
            CAP,
            "the cap is fully held by the first generation"
        );

        // The same user + same sid is re-admitted through the store: the
        // replacement generation of the same bearer identity.
        let replacement = stamped_session(&store, "alice", user);

        // The replacement INHERITS the first generation's minted activity
        // handle (occupied same-sid re-admission preserves the minted
        // handle) — the cap must still reject it while the full cap is held,
        // because it inherits the SAME shared counter.
        assert_eq!(
            replacement.activity_handle(),
            alice.activity_handle(),
            "the replacement preserves the predecessor's minted handle"
        );
        assert!(
            handler
                .try_track_activity_request(&replacement, "app", "read", None)
                .is_err(),
            "the replacement must be refused while the full cap is held"
        );
        assert_eq!(
            handler.activity_requests.in_flight(),
            CAP,
            "the refusal left the shared counter untouched"
        );

        // Release ONE old-generation permit; the replacement is admitted
        // against the SAME inherited counter (it cannot raise its
        // concurrency above the cap).
        drop(held_a);
        let reacquired = handler
            .try_track_activity_request(&replacement, "app", "read", None)
            .expect("the released slot admits the replacement generation");
        assert_eq!(
            handler.activity_requests.in_flight(),
            CAP,
            "the replacement shares the predecessor's counter: total stays at cap"
        );
        drop((held_b, reacquired));
        assert_eq!(
            handler.activity_requests.in_flight(),
            0,
            "both generations' guards release their entries"
        );
    });
}

#[test]
fn early_return_and_panic_release_the_slot_for_the_same_session() {
    on_big_stack(|| async {
        let (handler, store) = fixture_with_cap().await;
        let alice = stamped_session(&store, "alice", [3u8; 16]);
        // Early-return release: a guard dropped on a non-panic path frees
        // the slot for the next admission.
        fn early_return(handler: &ShamirDbHandler, s: &Session) -> bool {
            let _guard = handler
                .try_track_activity_request(s, "app", "read", None)
                .expect("slot under cap");
            true // early return; `_guard` drops here
        }
        assert!(early_return(&handler, &alice));
        let held = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("slot released by the early return");

        // Panic release: an unwinding request cannot leak its slot.
        let handler_ref = &handler;
        let alice_ref = &*alice;
        let result = std::panic::catch_unwind(AssertUnwindSafe(move || {
            let _guard = handler_ref
                .try_track_activity_request(alice_ref, "app", "read", None)
                .expect("slot under cap");
            assert_eq!(handler_ref.activity_requests.in_flight(), 2);
            panic!("simulated request panic holding a slot");
        }));
        assert!(result.is_err(), "the panic must propagate for this test");
        let recovered = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("slot must be released by the unwinding guard");
        assert_eq!(handler.activity_requests.in_flight(), 2);

        drop((held, recovered));
        assert_eq!(handler.activity_requests.in_flight(), 0);
    });
}

#[test]
fn stateless_handle_zero_session_skips_the_cap() {
    on_big_stack(|| async {
        let (handler, store) = fixture_with_cap().await;
        let alice = stamped_session(&store, "alice", [4u8; 16]);

        let _g1 = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("alice slot 1");
        let _g2 = handler
            .try_track_activity_request(&alice, "app", "read", None)
            .expect("alice slot 2");
        assert!(
            handler
                .try_track_activity_request(&alice, "app", "read", None)
                .is_err(),
            "alice saturated"
        );

        // The stateless (handle-0) fallback skips the cap entirely.
        let stateless = stateless_session();
        let guard = handler
            .try_track_activity_request(&stateless, "app", "read", None)
            .expect("stateless sessions bypass the per-session cap");
        assert_eq!(handler.activity_requests.in_flight(), 3);
        drop(guard);
    });
}

#[test]
fn real_write_is_refused_before_side_effect_when_session_is_saturated() {
    on_big_stack(|| async {
        let (handler, alice, _store) = write_fixture().await;
        let conn = ConnectionServices::without_push(0);

        // Saturate alice's single slot.
        let held = handler
            .try_track_activity_request(&alice, "app", "write", None)
            .expect("alice slot");

        // The real write is refused BEFORE execution (the slot gate sits
        // ahead of the budget wait and the engine call).
        let refused = handler
            .execute(
                &alice,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                insert_batch(),
                &conn,
            )
            .await;
        match refused {
            DbResponse::Error { code, message } => {
                assert_eq!(code, "server_busy", "expected the fixed busy code");
                assert_eq!(message, "server_busy", "expected the fixed busy text");
            }
            _ => panic!("a saturated session's write must be refused"),
        }

        // The refusal left NO side effect: exactly the guard's own entry
        // is in flight, and no row landed.
        assert_eq!(
            handler.activity_requests.in_flight(),
            1,
            "only the held guard remains in flight"
        );

        // Release the slot; BEFORE the admitted retry, prove the table is
        // empty: the refused write must not have staged any row (a later
        // overwrite could otherwise mask an earlier side effect).
        drop(held);
        let empty = handler
            .execute(
                &alice,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                read_batch(),
                &conn,
            )
            .await;
        match empty {
            DbResponse::Batch { response } => {
                assert_eq!(
                    response.results["r1"].records.len(),
                    0,
                    "the refused write must not have landed any row"
                );
            }
            _ => panic!("expected a Batch read response"),
        }

        // The same write is now admitted and lands EXACTLY ONE row.
        let admitted = handler
            .execute(
                &alice,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                insert_batch(),
                &conn,
            )
            .await;
        assert!(
            matches!(admitted, DbResponse::Batch { .. }),
            "the write must be admitted once the slot is free"
        );
        let counted = handler
            .execute(
                &alice,
                CURRENT_QUERY_LANG_VERSION,
                "app",
                read_batch(),
                &conn,
            )
            .await;
        match counted {
            DbResponse::Batch { response } => {
                assert_eq!(
                    response.results["r1"].records.len(),
                    1,
                    "the refused write must not have staged any row"
                );
            }
            _ => panic!("expected a Batch read response"),
        }
        assert_eq!(
            handler.activity_requests.in_flight(),
            0,
            "completed requests release their entries"
        );
    });
}
