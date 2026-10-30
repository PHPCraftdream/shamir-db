//! Regression tests for the resume flow:
//! - RV-CONNECT-ECE3E157: counter persistence failure must be distinguishable
//!   from replay (distinct `Error` variant, not `AuthFailed`).

use crate::common::error::Error;
use crate::common::time::ns;
use crate::common::types::limits;
use crate::common::types::BindingMode;
use crate::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, AdvanceOutcome, ConsumedCounterStore,
    InMemoryConsumedCounters, InMemoryUserStateMap, ResumeConfig, ResumeRequest, ResumeUserState,
};
use crate::server::rotation::ServerIdentityState;
use crate::server::session::SessionStore;
use crate::server::ticket::{
    decrypt_ticket, encrypt_ticket, encrypt_ticket_with_cipher, TicketPlain, TicketWire,
};

#[test]
fn resume_entropy_failure_does_not_consume_counter_or_publish_session() {
    let cfg = config();
    let users = lookup_map();
    let store = SessionStore::new();
    let counters = InMemoryConsumedCounters::new();
    let identity = ServerIdentityState::fresh();
    let now = ns::HOUR * 1_000;
    let (ticket, _) = issue_initial_ticket(
        cfg.ticket_key(),
        USER,
        "u".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77; 32],
        0,
        now,
        ns::HOUR,
    )
    .unwrap();
    let req = request(&ticket, [0xab; 32]);
    let result = crate::common::crypto::with_entropy_failure_after(1, || {
        process_resume(
            &req,
            &cfg,
            &counters,
            &users,
            &store,
            &identity,
            24 * ns::HOUR,
            1000,
            now,
        )
    });
    assert!(matches!(
        result,
        Err(Error::Environment(
            crate::common::error::EnvironmentError::Entropy
        ))
    ));
    assert!(counters.is_empty());
    assert_eq!(store.len(), 0);
    assert!(process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1000,
        now
    )
    .is_ok());
}

#[test]
fn warmed_rotation_ciphers_accept_both_keys() {
    let cfg = ResumeConfig::new([1; 32], Some([2; 32]), true, false);
    let plain = TicketPlain {
        version: 2,
        user_id: [1; 16].into(),
        username_nfc: "alice".into(),
        transport_kind_at_auth: 1,
        binding_mode_at_auth: 1,
        channel_binding_at_auth: [2; 32].into(),
        ticket_family_id: [3; 16].into(),
        original_auth_at_ns: 1,
        expires_at_ns: 2,
        family_counter: 0,
        identity_key_version: 0,
    };
    let (current, previous) = cfg.ciphers();
    let current_wire = encrypt_ticket(&[1; 32], &plain).unwrap();
    let previous_wire = encrypt_ticket(&[2; 32], &plain).unwrap();
    assert_eq!(
        crate::server::ticket::decrypt_ticket_with_ciphers(current, previous, &current_wire)
            .unwrap(),
        plain
    );
    let (current, previous) = cfg.ciphers();
    assert_eq!(
        crate::server::ticket::decrypt_ticket_with_ciphers(current, previous, &previous_wire)
            .unwrap(),
        plain
    );
    assert_eq!(cfg.ticket_key(), &[1; 32]);
    assert_eq!(cfg.ticket_key_previous(), Some(&[2; 32]));
}

#[test]
fn ticket_wire_bounds_and_supported_version() {
    for len in [0, u16::MAX as usize] {
        let wire = TicketWire {
            version: 2,
            nonce: [1; 12],
            ciphertext: vec![2; len],
            tag: [3; 16],
        };
        assert_eq!(
            TicketWire::from_bytes(&wire.to_bytes().unwrap()).unwrap(),
            wire
        );
    }
    let oversized = TicketWire {
        version: 2,
        nonce: [1; 12],
        ciphertext: vec![2; 65536],
        tag: [3; 16],
    };
    assert!(oversized.to_bytes().is_err());
    let mut plain = TicketPlain {
        version: 2,
        user_id: [1; 16].into(),
        username_nfc: "alice".into(),
        transport_kind_at_auth: 1,
        binding_mode_at_auth: 1,
        channel_binding_at_auth: [2; 32].into(),
        ticket_family_id: [3; 16].into(),
        original_auth_at_ns: 1,
        expires_at_ns: 2,
        family_counter: 0,
        identity_key_version: 0,
    };
    let key = [4; 32];
    let wire = encrypt_ticket(&key, &plain).unwrap();
    assert_eq!(decrypt_ticket(&key, None, &wire).unwrap(), plain);
    plain.version = 3;
    assert!(encrypt_ticket(&key, &plain).is_err());
    assert!(encrypt_ticket_with_cipher(
        &crate::common::crypto::aes256gcm_cipher(&key).unwrap(),
        &plain
    )
    .is_err());
    plain.version = 2;
    plain.username_nfc = "x".repeat(65536);
    assert!(encrypt_ticket(&key, &plain).is_err());
}

const USER: [u8; 16] = [0x5au8; 16];

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

fn request<'a>(ticket: &'a [u8], nonce: [u8; 32]) -> ResumeRequest<'a> {
    ResumeRequest {
        ticket_wire_bytes: ticket,
        client_nonce: nonce,
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    }
}

/// Counter stub that always reports a persistence failure (RV-CONNECT-ECE3E157
/// fault-injection seam): `try_advance` never accepts, mirroring a failing
/// durable backend (get/insert/persist error).
struct FailingCounterStore;

impl ConsumedCounterStore for FailingCounterStore {
    fn try_advance(
        &self,
        _user_id: &[u8; 16],
        _family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        _new_counter: u64,
    ) -> AdvanceOutcome {
        AdvanceOutcome::StorageFailed
    }
}

/// RV-CONNECT-ECE3E157: with a failing persistence path, `process_resume`
/// must return the DISTINCT `Error::StorageFailed` (server-side storage
/// error), not the replay rejection `Error::AuthFailed` — a persistence
/// outage must not be reported to the caller as a replayed ticket.
#[test]
fn rv_counters_ece3e157_persistence_failure_is_not_replay_rejection() {
    let cfg = config();
    let users = lookup_map();
    let store = SessionStore::new();
    let identity = ServerIdentityState::fresh();
    let now = ns::HOUR * 1_000;

    let (ticket, _) = issue_initial_ticket(
        cfg.ticket_key(),
        USER,
        "u".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0,
        now,
        ns::HOUR,
    )
    .unwrap();

    let req = request(&ticket, [0xabu8; 32]);
    let err = process_resume(
        &req,
        &cfg,
        &FailingCounterStore,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1_000,
        now,
    )
    .expect_err("storage failure must reject the resume");
    assert!(
        matches!(err, Error::StorageFailed),
        "persistence failure must map to StorageFailed, not AuthFailed (got {err:?})"
    );
    assert_eq!(
        store.len(),
        0,
        "no session may be created on storage failure"
    );
}

/// RV-CONNECT-B11F48CA: ticket keys are immutable after construction — the
/// public `OnceLock` cipher caches are warmed on first use, so a previously
/// mutable key field could be swapped after warming and leave ciphers bound
/// to the OLD key serving resumes. Keys are now private; the read-only
/// accessors always agree with the warmed caches.
#[test]
fn rv_connect_b11f48ca_resume_keys_immutable() {
    let cfg = ResumeConfig::new([0xa1u8; 32], Some([0xb2u8; 32]), true, false);

    // Warm the permanent cipher caches first (the hazard window).
    let (current, previous) = cfg.ciphers();
    assert!(!std::ptr::eq(current, previous.unwrap()));

    // The accessor view matches the keys the caches were scheduled from.
    assert_eq!(cfg.ticket_key(), &[0xa1u8; 32]);
    assert_eq!(cfg.ticket_key_previous(), Some(&[0xb2u8; 32]));

    // Functional anchor: a ticket minted under `ticket_key()` is accepted by
    // `process_resume`, proving the caches and the accessor agree.
    let users = lookup_map();
    let store = SessionStore::new();
    let identity = ServerIdentityState::fresh();
    let now = ns::HOUR * 1_000;
    let (ticket, _) = issue_initial_ticket(
        cfg.ticket_key(),
        USER,
        "u".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0,
        now,
        ns::HOUR,
    )
    .unwrap();
    let req = request(&ticket, [0xabu8; 32]);
    let outcome = process_resume(
        &req,
        &cfg,
        &InMemoryConsumedCounters::default(),
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        1_000,
        now,
    )
    .expect("resume with the immutable key must succeed");
    assert_eq!(outcome.username, "u");
}
