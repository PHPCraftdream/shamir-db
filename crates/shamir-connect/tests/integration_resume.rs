//! Integration tests for resumption flow (SESSION_RESUMPTION §5).

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::BindingMode;
use shamir_connect::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, ConsumedCounterStore,
    InMemoryConsumedCounters, ResumeConfig, ResumeRequest,
};
use shamir_connect::server::session::SessionStore;

const TICKET_TTL: u64 = ns::HOUR;

fn fixed_config() -> ResumeConfig {
    ResumeConfig {
        ticket_key: [0xa1u8; 32],
        ticket_key_previous: None,
        allow_browser_ticket_upgrade: true,
        disable_plain_ticket_upgrade: false,
    }
}

#[test]
fn full_resume_round_trip() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0); // tickets_invalid_before_ns = 0
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, ticket_exp) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8().into(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();
    assert!(ticket_exp > now);

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };

    let ok = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now).unwrap();
    assert_eq!(store.len(), 1);
    assert_eq!(counters.len(), 1);
    let ticket_v2 = ok.resumption_ticket.expect("expected new ticket issued");

    // Step 2: same ticket cannot be replayed.
    let req2 = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req2, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));

    // Step 3: NEW ticket from step 1 (counter+1) does work.
    let req3 = ResumeRequest {
        ticket_wire_bytes: &ticket_v2,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok2 = process_resume(&req3, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now).unwrap();
    assert_eq!(store.len(), 2);
}

#[test]
fn rejects_expired_ticket() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        ns::SECOND, // 1s TTL
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };

    // 2 seconds later — expired.
    let later = now + 2 * ns::SECOND;
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, later);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_when_user_kicked_via_tickets_invalid_before() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now, // original_auth_at_ns = now
        TICKET_TTL,
    )
    .unwrap();

    // Admin then kicked the user — tickets_invalid_before_ns = now (== original_auth_at_ns).
    // Per spec §5.4 step 9: STRICT > → reject.
    users.insert(user_id, now);

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn ticket_one_ns_after_kick_succeeds() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now + 1, // ticket strictly after kick boundary
        TICKET_TTL,
    )
    .unwrap();
    users.insert(user_id, now);

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now + 2);
    assert!(ok.is_ok());
}

#[test]
fn rejects_anti_downgrade_tls_to_browser() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(), // ticket issued in TLS exporter context
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Resume in TlsNoExport (browser) — DOWNGRADE.
    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsNoExport,
        channel_binding_now: [0u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn allows_browser_to_native_upgrade_by_default() {
    let cfg = fixed_config(); // allow_browser_ticket_upgrade = true
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsNoExport.as_u8(),
        BindingMode::TlsNoExport.as_u8(), // browser-issued ticket
        [0u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter, // upgrade to native
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(result.is_ok());
}

#[test]
fn strict_mode_rejects_browser_to_native() {
    let mut cfg = fixed_config();
    cfg.allow_browser_ticket_upgrade = false;

    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsNoExport.as_u8(),
        BindingMode::TlsNoExport.as_u8(),
        [0u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_when_user_unknown() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map(); // empty — user not found
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_when_ticket_key_changed_no_overlap() {
    let mut cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Server emergency-rotated ticket_key without overlap.
    cfg.ticket_key = [0xcdu8; 32];
    cfg.ticket_key_previous = None;

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn ticket_works_under_previous_key_during_overlap() {
    let mut cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let old_key = cfg.ticket_key;
    let (ticket_bytes, _) = issue_initial_ticket(
        &old_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Server rotated current → previous = old; current = new.
    cfg.ticket_key = [0xcdu8; 32];
    cfg.ticket_key_previous = Some(old_key);

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(ok.is_ok());
}

#[test]
fn rejects_aad_tampered_ticket() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (mut ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Flip a byte inside ciphertext.
    ticket_bytes[20] ^= 0xff;

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(&req, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn multi_device_family_isolation() {
    // Laptop (family A) refresh DOES NOT invalidate phone (family B) ticket.
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();

    // Two independent initial tickets — server issues fresh family_id each time.
    let (ticket_a, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    let (ticket_b, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Laptop uses ticket_a → counters[(user, family_a)] = 1
    let req_a = ResumeRequest {
        ticket_wire_bytes: &ticket_a,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok_a = process_resume(&req_a, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now).unwrap();

    // Laptop refreshes via the new ticket — family_a counter advances.
    let new_a_bytes = ok_a.resumption_ticket.unwrap();
    let req_a2 = ResumeRequest {
        ticket_wire_bytes: &new_a_bytes,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok_a2 = process_resume(&req_a2, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now).unwrap();

    // Phone uses ticket_b — must STILL succeed (different family_id).
    let req_b = ResumeRequest {
        ticket_wire_bytes: &ticket_b,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok_b = process_resume(&req_b, &cfg, &counters, &users, &store, 24 * ns::HOUR, TICKET_TTL, now);
    assert!(ok_b.is_ok(), "phone ticket must survive laptop refresh");
}

#[test]
fn counter_store_gc_evicts_stale_entries() {
    let counters = InMemoryConsumedCounters::new();
    let uid = [1u8; 16];
    let fam = [2u8; 16];
    counters.try_advance(&uid, &fam, 1);
    assert_eq!(counters.len(), 1);

    let now_far_future = UnixNanos::now().as_u64() + 48 * ns::HOUR;
    counters.gc(now_far_future);
    assert_eq!(counters.len(), 0);
}
