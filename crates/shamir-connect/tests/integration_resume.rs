//! Integration tests for resumption flow (SESSION_RESUMPTION §5).

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::BindingMode;
use shamir_connect::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, ConsumedCounterStore,
    InMemoryConsumedCounters, ResumeConfig, ResumeRequest,
};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;

const TICKET_TTL: u64 = ns::HOUR;

fn fixed_config() -> ResumeConfig {
    ResumeConfig::new(
        [0xa1u8; 32], // ticket_key
        None,         // ticket_key_previous
        true,         // allow_browser_ticket_upgrade
        false,        // disable_plain_ticket_upgrade
    )
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
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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

    let ok = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now).unwrap();
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
    let result = process_resume(&req2, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));

    // Step 3: NEW ticket from step 1 (counter+1) does work.
    let req3 = ResumeRequest {
        ticket_wire_bytes: &ticket_v2,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok2 = process_resume(&req3, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now).unwrap();
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
        vec![],
        0,
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, later);
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
        vec![],
        0,
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![],
        0,
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
    let ok = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now + 2);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![],
        0,
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![],
        0,
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let ok = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let result = process_resume(&req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
        vec![], // roles snapshot (empty for these tests; see admin-resume test for non-empty)
        0,      // identity_key_version
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
    let ok_a = process_resume(&req_a, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now).unwrap();

    // Laptop refreshes via the new ticket — family_a counter advances.
    let new_a_bytes = ok_a.resumption_ticket.unwrap();
    let req_a2 = ResumeRequest {
        ticket_wire_bytes: &new_a_bytes,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok_a2 = process_resume(&req_a2, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now).unwrap();

    // Phone uses ticket_b — must STILL succeed (different family_id).
    let req_b = ResumeRequest {
        ticket_wire_bytes: &ticket_b,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok_b = process_resume(&req_b, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now);
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

/// Diagram 02 step 12 + SESSION_RESUMPTION §2.1: the resumed [`Session`] MUST
/// be constructed with `permissions = ticket_plain.roles`. A `superuser`
/// session resumed via ticket MUST retain `is_superuser == true` so admin
/// commands continue to work.
#[test]
fn resumed_admin_session_retains_roles_per_diagram_02() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa0u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let admin_roles = vec!["superuser".to_string(), "read_write".to_string()];
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "admin".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        admin_roles.clone(),
        0,
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
    let ok = process_resume(
        &req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now,
    )
    .unwrap();

    // Pull the freshly-created session and assert role/superuser preserved.
    let session = store.lookup(&ok.session_id).expect("session created");
    let perms = &session.permissions;
    assert!(
        perms.is_superuser,
        "admin resumed via ticket must keep superuser flag (diagram 02 step 12)"
    );
    assert_eq!(perms.roles, admin_roles, "roles vector must round-trip");
}

/// Diagram 02 step 13 + SESSION_RESUMPTION §2.1: a refresh-ticket issued
/// during resume MUST carry the same roles forward so subsequent resumes
/// continue to authorize the user as admin.
#[test]
fn refresh_ticket_carries_roles_forward_per_diagram_02() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa1u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let admin_roles = vec!["superuser".to_string()];
    let (ticket_v1, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "admin".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        admin_roles.clone(),
        0,
        now,
        TICKET_TTL,
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_v1,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok = process_resume(
        &req, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now,
    )
    .unwrap();
    let ticket_v2 = ok.resumption_ticket.expect("refresh ticket issued");

    // Resume with the refreshed ticket — admin still admin.
    let req2 = ResumeRequest {
        ticket_wire_bytes: &ticket_v2,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok2 = process_resume(
        &req2, &cfg, &counters, &users, &store, &ServerIdentityState::fresh(), 24 * ns::HOUR, TICKET_TTL, now,
    )
    .unwrap();
    let session2 = store.lookup(&ok2.session_id).expect("session created");
    assert!(session2.permissions.is_superuser);
}

/// Spec §5.7 NORMATIVE / diagram 12 footer: a ticket issued under the
/// previous keypair MUST be rejected during the rotation overlap window. The
/// orphan client is forced through full SCRAM and picks up the
/// `rotation_in_progress` payload (diagram 05 Part B).
#[test]
fn pre_rotation_ticket_rejected_during_overlap_per_diagram_12() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xb0u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    // Identity v0 — issue a ticket under it.
    let identity = ServerIdentityState::fresh();
    assert_eq!(identity.current_version(), 0);
    let now = UnixNanos::now().as_u64();
    let (ticket_v0, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        vec![],
        identity.current_version(), // 0
        now,
        TICKET_TTL,
    )
    .unwrap();

    // Rotate → now we're in overlap, current_version becomes 1.
    let outcome = identity.rotate(now).unwrap();
    assert_eq!(outcome.new_version, 1);
    assert_eq!(identity.current_version(), 1);
    assert!(identity.rotation_in_progress(now));

    // Resume with the v0 ticket — MUST be rejected (force re-auth).
    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_v0,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(
        &req, &cfg, &counters, &users, &store, &identity, 24 * ns::HOUR, TICKET_TTL, now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
    assert_eq!(store.len(), 0, "no session should be created");
}

/// Diagram 12 + spec §5.7: tickets issued AFTER rotation (under
/// `current_version`) MUST be accepted normally even while overlap is still
/// active.
#[test]
fn post_rotation_ticket_accepted_during_overlap() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xb1u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let identity = ServerIdentityState::fresh();
    let now = UnixNanos::now().as_u64();
    identity.rotate(now).unwrap();
    assert!(identity.rotation_in_progress(now));

    // Brand-new ticket issued under post-rotation version (1).
    let (ticket_v1, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        vec![],
        identity.current_version(), // 1
        now,
        TICKET_TTL,
    )
    .unwrap();

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_v1,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok = process_resume(
        &req, &cfg, &counters, &users, &store, &identity, 24 * ns::HOUR, TICKET_TTL, now,
    );
    assert!(ok.is_ok(), "post-rotation ticket must work during overlap");
}

/// Diagram 12: after the overlap window finalizes (current_version stays at
/// the post-rotation value), pre-rotation tickets remain rejected (their
/// version is now strictly less than current).
#[test]
fn pre_rotation_ticket_rejected_after_overlap_finalize() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xb2u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, 0);
    let store = SessionStore::new();

    let identity = ServerIdentityState::fresh();
    let now = UnixNanos::now().as_u64();
    let (ticket_v0, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        vec![],
        0,
        now,
        TICKET_TTL,
    )
    .unwrap();

    identity.rotate(now).unwrap();
    // Force overlap to finalize.
    let after_overlap = now + 8 * ns::DAY;
    assert!(identity.try_finalize(after_overlap));
    assert!(!identity.rotation_in_progress(after_overlap));
    assert_eq!(identity.current_version(), 1);

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_v0,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        TICKET_TTL,
        // Use after_overlap as "now" but back-date the chain-age check via
        // a now within original TTL — supply now, ticket still in TTL because
        // we used a 1h TTL and "now" param is fresh.
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

/// Optim #2: confirm wire-compatibility — `serde_bytes::ByteArray<N>` must
/// produce IDENTICAL msgpack bytes as `#[serde(with = "serde_bytes")] Vec<u8>`
/// for the same content. Otherwise tickets issued by old servers (or by
/// other-language clients) would fail to decrypt.
///
/// Strategy: define a Vec-typed mirror struct, encode both, compare bytes.
#[test]
fn ticket_plain_bytearray_wire_compat_with_vec_u8() {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct VecMirror {
        version: u8,
        #[serde(with = "serde_bytes")]
        user_id: Vec<u8>,
        username_nfc: String,
        transport_kind_at_auth: u8,
        binding_mode_at_auth: u8,
        #[serde(with = "serde_bytes")]
        channel_binding_at_auth: Vec<u8>,
        #[serde(with = "serde_bytes")]
        ticket_family_id: Vec<u8>,
        original_auth_at_ns: u64,
        expires_at_ns: u64,
        family_counter: u64,
        roles: Vec<String>,
        identity_key_version: u64,
    }

    let mirror = VecMirror {
        version: 1,
        user_id: vec![0x01u8; 16],
        username_nfc: "alice".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: vec![0x77u8; 32],
        ticket_family_id: vec![0x11u8; 16],
        original_auth_at_ns: 1_000_000,
        expires_at_ns: 2_000_000,
        family_counter: 1,
        roles: vec!["read_write".into()],
        identity_key_version: 0,
    };

    use shamir_connect::server::ticket::TicketPlain;
    let real = TicketPlain {
        version: 1,
        user_id: serde_bytes::ByteArray::new([0x01u8; 16]),
        username_nfc: "alice".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: serde_bytes::ByteArray::new([0x77u8; 32]),
        ticket_family_id: serde_bytes::ByteArray::new([0x11u8; 16]),
        original_auth_at_ns: 1_000_000,
        expires_at_ns: 2_000_000,
        family_counter: 1,
        roles: vec!["read_write".into()],
        identity_key_version: 0,
    };

    let mirror_bytes = rmp_serde::to_vec_named(&mirror).unwrap();
    let real_bytes = rmp_serde::to_vec_named(&real).unwrap();

    assert_eq!(
        mirror_bytes, real_bytes,
        "ByteArray<N> wire format must match #[serde(with = \"serde_bytes\")] Vec<u8>"
    );

    // Cross-deserialize: ByteArray bytes must decode into Vec mirror and vice versa.
    let decoded_mirror: VecMirror = rmp_serde::from_slice(&real_bytes).unwrap();
    assert_eq!(decoded_mirror, mirror);

    let decoded_real: TicketPlain = rmp_serde::from_slice(&mirror_bytes).unwrap();
    assert_eq!(decoded_real, real);
}
