//! Resume-path admission cap tests (RV-CONNECT-8AD8B1C8): a valid resume
//! goes through the SAME capped `SessionStore::admit` as login — it can
//! neither bypass `MAX_SESSIONS_PER_USER` nor evade the all-active
//! rejection.

use crate::common::error::Error;
use crate::common::time::ns;
use crate::common::types::limits;
use crate::common::types::BindingMode;
use crate::server::admission::{Admission, AdmitReject, ACTIVE_WINDOW_NS};
use crate::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, InMemoryConsumedCounters,
    InMemoryUserStateMap, ResumeConfig, ResumeRequest, ResumeUserState,
};
use crate::server::rotation::ServerIdentityState;
use crate::server::session::{Session, SessionPermissions, SessionStore};
use crate::server::ticket::{decrypt_ticket, TicketWire};

const USER: [u8; 16] = [0x5au8; 16];
const T: u64 = ns::HOUR * 1_000;

fn config() -> ResumeConfig {
    ResumeConfig::new([0xa1u8; 32], None, true, false)
}

fn lookup_map() -> InMemoryUserStateMap {
    let users = new_user_state_map();
    users.insert(
        USER,
        ResumeUserState {
            username: "u".into(),
            roles: vec![],
            superuser: false,
            replicator: false,
            tickets_invalid_before_ns: 0,
        },
    );
    users
}

fn request<'a>(ticket: &'a [u8]) -> ResumeRequest<'a> {
    ResumeRequest {
        ticket_wire_bytes: ticket,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    }
}

fn sess(last_activity_ns: u64) -> Session {
    Session::new(
        USER,
        "u".into(),
        SessionPermissions::from_roles(vec![]),
        crate::common::types::TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        last_activity_ns,
    )
}

fn seed(store: &SessionStore, i: u8, last_activity_ns: u64) {
    match store.admit([i; limits::SESSION_ID_BYTES], sess(last_activity_ns), T, 16) {
        Admission::Admitted { .. } => {}
        Admission::Rejected(r) => panic!("seeding must succeed: {r:?}"),
    }
}

/// Issue an initial ticket and derive (family_id, counter) from the issued
/// ticket itself (decrypting it with the config key — `issue_initial_ticket`
/// generates the family id internally and returns only wire bytes + expiry).
fn issue_and_decode(
    cfg: &ResumeConfig,
    identity: &ServerIdentityState,
) -> (Vec<u8>, [u8; 16], u64) {
    let (ticket, _) = issue_initial_ticket(
        &cfg.ticket_key,
        USER,
        "u".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        identity.current_version(),
        T - ns::HOUR,
        24 * ns::HOUR,
    )
    .unwrap();
    let plain = decrypt_ticket(
        &cfg.ticket_key,
        None,
        &TicketWire::from_bytes(&ticket).expect("parse issued ticket"),
    )
    .expect("decrypt issued ticket");
    let family_id: [u8; 16] = *plain.ticket_family_id.as_ref();
    let counter = plain.family_counter; // issue_initial_ticket ⇒ counter = 1
    (ticket, family_id, counter)
}

/// RV-CONNECT-8AD8B1C8 — resume cannot bypass the cap: with 16 ACTIVE
/// sessions (`last = T - 1 s`), `process_resume` at T returns
/// `Err(AdmissionRejected { reason: AllActive, .. })` whose wire form
/// collapses to a generic `AuthFailed`; count stays 16 and no session is
/// added.
///
/// ALSO (binding §4): the cap-rejected resume CONSUMES its anti-replay
/// ticket counter — the durable advance happens BEFORE admission and is
/// never rolled back; `peek(USER, family) == Some(1)` afterwards.
///
/// Fails if ONLY the resume-admission fix is reverted (old `process_resume`
/// called the UNCAPPED `SessionStore::insert`): the resume succeeds, a
/// 17th session is stored, and no `AdmissionRejected` error exists.
/// Fails if ONLY the no-rollback consumption decision is reverted
/// (advancement rolled back on admission rejection): `peek` returns `None`.
#[test]
fn rv_connect_8ad8b1c8_resume_rejected_when_all_sessions_active() {
    let cfg = config();
    let users = lookup_map();
    let store = SessionStore::new();
    let identity = ServerIdentityState::fresh();
    // ONE shared counter store, also passed to process_resume, so the
    // consumption decision can be observed via `peek`.
    let counters = InMemoryConsumedCounters::default();

    for i in 0..16u8 {
        seed(&store, i, T - 1_000_000_000); // active
    }
    assert_eq!(store.count_for_user(&USER), 16);

    let (ticket, family_id, ticket_counter) = issue_and_decode(&cfg, &identity);
    assert_eq!(ticket_counter, 1, "issue_initial_ticket gives counter = 1");

    let err = process_resume(
        &request(&ticket),
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1_000,
        T,
    )
    .expect_err("all sessions active: resume must be rejected");
    assert!(
        matches!(
            err,
            Error::AdmissionRejected {
                reason: AdmitReject::AllActive,
                ..
            }
        ),
        "expected AdmissionRejected(AllActive), got {err:?}"
    );
    assert!(
        matches!(err.to_wire(), Error::AuthFailed),
        "wire form must collapse to generic authentication_failed"
    );
    assert_eq!(store.count_for_user(&USER), 16, "count unchanged");
    assert_eq!(store.len(), 16, "no session added");
    // Binding §4: a cap-rejected resume consumes its ticket — no rollback.
    assert_eq!(
        counters.peek(&USER, &family_id),
        Some(ticket_counter),
        "rejected resume must consume its ticket counter"
    );
}

/// RV-CONNECT-8AD8B1C8 — with an IDLE session available (age ≥ 60 s), a
/// resume is ADMITTED and evicts exactly the idle LRU session; the count
/// stays 16 and `ResumeOk` carries the evicted sid. The anti-replay counter
/// also advanced to the consumed ticket counter.
///
/// Fails if ONLY the resume-admission fix is reverted: the resume inserts
/// without eviction, the count becomes 17 and `evicted` is empty.
#[test]
fn rv_connect_8ad8b1c8_resume_evicts_idle_lru_session() {
    let cfg = config();
    let users = lookup_map();
    let store = SessionStore::new();
    let identity = ServerIdentityState::fresh();
    let counters = InMemoryConsumedCounters::default();

    // 15 active + 1 idle (exactly 60 s old ⇒ idle).
    for i in 1..16u8 {
        seed(&store, i, T - 1_000_000_000);
    }
    seed(&store, 0, T - ACTIVE_WINDOW_NS);
    assert_eq!(store.count_for_user(&USER), 16);

    let (ticket, family_id, ticket_counter) = issue_and_decode(&cfg, &identity);

    let ok = process_resume(
        &request(&ticket),
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1_000,
        T,
    )
    .expect("idle session available: resume must be admitted");
    assert_eq!(ok.evicted.len(), 1, "exactly the idle session evicted");
    assert_eq!(ok.evicted[0], [0u8; limits::SESSION_ID_BYTES]);
    assert_eq!(store.count_for_user(&USER), 16);
    assert!(store.lookup(&ok.evicted[0]).is_none());
    assert!(store.lookup(&ok.session_id).is_some());
    // The ticket was consumed (advanced to the ticket's own counter; the
    // refresh ticket's counter = ticket+1 lives only in the issued ticket).
    assert_eq!(
        counters.peek(&USER, &family_id),
        Some(ticket_counter),
        "admitted resume consumes its ticket counter"
    );
}
