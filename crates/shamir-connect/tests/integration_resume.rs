//! Integration tests for resumption flow (SESSION_RESUMPTION §5).

use shamir_connect::common::time::{ns, UnixNanos};
use shamir_connect::common::types::BindingMode;
use shamir_connect::server::resume::{
    issue_initial_ticket, new_user_state_map, process_resume, ConsumedCounterStore,
    InMemoryConsumedCounters, ResumeConfig, ResumeRequest, ResumeUserState,
};
use shamir_connect::server::rotation::ServerIdentityState;
use shamir_connect::server::session::SessionStore;
use shamir_connect::server::ticket::TicketWire;

const TICKET_TTL: u64 = ns::HOUR;

/// Test helper: a minimal `ResumeUserState` carrying only a
/// `tickets_invalid_before_ns` epoch (username/roles/superuser left at
/// harmless defaults). Used at the mechanical call sites that only care
/// about the epoch check.
fn state(tib: u64) -> ResumeUserState {
    ResumeUserState {
        username: "u".into(),
        roles: vec![],
        superuser: false,
        tickets_invalid_before_ns: tib,
    }
}

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
    users.insert(user_id, state(0)); // tickets_invalid_before_ns = 0
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, ticket_exp) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0, // identity_key_version
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

    let ok = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();
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
    let result = process_resume(
        &req2,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));

    // Step 3: NEW ticket from step 1 (counter+1) does work.
    let req3 = ResumeRequest {
        ticket_wire_bytes: &ticket_v2,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok2 = process_resume(
        &req3,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();
    assert_eq!(store.len(), 2);
}

#[test]
fn rejects_expired_ticket() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        later,
    );
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
        0,
        now, // original_auth_at_ns = now
        TICKET_TTL,
    )
    .unwrap();

    // Admin then kicked the user — tickets_invalid_before_ns = now (== original_auth_at_ns).
    // Per spec §5.4 step 9: STRICT > → reject.
    users.insert(user_id, state(now));

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
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
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
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
        0,
        now + 1, // ticket strictly after kick boundary
        TICKET_TTL,
    )
    .unwrap();
    users.insert(user_id, state(now));

    let req = ResumeRequest {
        ticket_wire_bytes: &ticket_bytes,
        client_nonce: [0xabu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now + 2,
    );
    assert!(ok.is_ok());
}

#[test]
fn rejects_anti_downgrade_tls_to_browser() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(), // ticket issued in TLS exporter context
        [0x77u8; 32],
        0, // identity_key_version
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn allows_browser_to_native_upgrade_by_default() {
    let cfg = fixed_config(); // allow_browser_ticket_upgrade = true
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsNoExport.as_u8(),
        BindingMode::TlsNoExport.as_u8(), // browser-issued ticket
        [0u8; 32],
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(result.is_ok());
}

#[test]
fn strict_mode_rejects_browser_to_native() {
    let mut cfg = fixed_config();
    cfg.allow_browser_ticket_upgrade = false;

    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsNoExport.as_u8(),
        BindingMode::TlsNoExport.as_u8(),
        [0u8; 32],
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
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
        0, // identity_key_version
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn rejects_when_ticket_key_changed_no_overlap() {
    let mut cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0, // identity_key_version
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn ticket_works_under_previous_key_during_overlap() {
    let mut cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
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
        0, // identity_key_version
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
    let ok = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(ok.is_ok());
}

#[test]
fn rejects_aad_tampered_ticket() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (mut ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "alice".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0, // identity_key_version
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
    let result = process_resume(
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(matches!(result, Err(shamir_connect::Error::AuthFailed)));
}

#[test]
fn multi_device_family_isolation() {
    // Laptop (family A) refresh DOES NOT invalidate phone (family B) ticket.
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0x11u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
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
        0, // identity_key_version
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
        0, // identity_key_version
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
    let ok_a = process_resume(
        &req_a,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();

    // Laptop refreshes via the new ticket — family_a counter advances.
    let new_a_bytes = ok_a.resumption_ticket.unwrap();
    let req_a2 = ResumeRequest {
        ticket_wire_bytes: &new_a_bytes,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let _ok_a2 = process_resume(
        &req_a2,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();

    // Phone uses ticket_b — must STILL succeed (different family_id).
    let req_b = ResumeRequest {
        ticket_wire_bytes: &ticket_b,
        client_nonce: [0xadu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok_b = process_resume(
        &req_b,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
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

/// Task #558 (replaces the old `resumed_admin_session_retains_roles_per_diagram_02`,
/// which encoded the EXACT bug this task closes — "resume trusts the ticket's
/// `roles` snapshot"). The ticket no longer carries ANY authorization data
/// (roles/superuser), so a resumed session's permissions can ONLY come from the
/// directory lookup. This test proves the grant direction: seed the directory
/// with `superuser: true`, resume, and assert the session reflects the
/// directory — not anything the ticket carried.
#[test]
fn resumed_session_permissions_come_from_directory_lookup_not_ticket() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa0u8; 16];
    let users = new_user_state_map();
    let admin_state = ResumeUserState {
        username: "admin".into(),
        roles: vec!["read_write".to_string()],
        superuser: true,
        tickets_invalid_before_ns: 0,
    };
    users.insert(user_id, admin_state.clone());
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    // No `roles` argument — the ticket structurally cannot carry authorization.
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "admin".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0, // identity_key_version
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
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();

    // The session's permissions MUST match the LOOKUP map's state, proving the
    // session was built from the directory, not from the ticket (which, by
    // construction, has no roles/superuser field at all).
    let session = store.lookup(&ok.session_id).expect("session created");
    let perms = &session.permissions;
    assert!(
        perms.is_superuser,
        "resumed session must reflect the directory's superuser flag (task #558)"
    );
    assert_eq!(
        perms.roles, admin_state.roles,
        "resumed session roles must match the directory lookup, not the ticket"
    );
    // Username must come from the directory (`state.username`), not the
    // ticket's `username_nfc` (they happen to agree here, but see
    // `revoked_superuser_resolves_to_non_admin_without_epoch_bump` and the
    // rename-guard rationale in design doc §5).
    assert_eq!(session.username, admin_state.username);
}

/// Task #558 (replaces the old `refresh_ticket_carries_roles_forward_per_diagram_02`,
/// which asserted a refresh ticket carried roles forward). The refresh ticket
/// now carries NO authorization data, so a second resume — using the refreshed
/// ticket — still resolves its permissions from a FRESH directory lookup, not
/// from anything baked into the refreshed ticket.
#[test]
fn refresh_ticket_carries_no_authorization_session_reads_directory_each_resume() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa1u8; 16];
    let users = new_user_state_map();
    let admin_state = ResumeUserState {
        username: "admin".into(),
        roles: vec!["read_write".to_string()],
        superuser: true,
        tickets_invalid_before_ns: 0,
    };
    users.insert(user_id, admin_state.clone());
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    let (ticket_v1, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "admin".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0, // identity_key_version
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
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();
    let ticket_v2 = ok.resumption_ticket.expect("refresh ticket issued");

    // Resume with the refreshed ticket — the second session's permissions must
    // STILL come from the directory lookup (the refreshed ticket carries no
    // roles/superuser to influence them).
    let req2 = ResumeRequest {
        ticket_wire_bytes: &ticket_v2,
        client_nonce: [0xacu8; 32],
        binding_mode_now: BindingMode::TlsExporter,
        channel_binding_now: [0x77u8; 32],
    };
    let ok2 = process_resume(
        &req2,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();
    let session2 = store.lookup(&ok2.session_id).expect("session created");
    assert!(
        session2.permissions.is_superuser,
        "second resume must read superuser from the directory, not the refreshed ticket"
    );
    assert_eq!(
        session2.permissions.roles, admin_state.roles,
        "second resume roles must match the directory lookup"
    );
}

/// Task #558 red test #1: a v1 ticket (pre-cutover wire shape) MUST be
/// rejected after the version bump. The version gate at `process_resume`
/// step 2 (`wire.version != 2`) fails closed, forcing a full SCRAM re-auth.
/// This is the ENTIRE migration mechanism — no dual-version window.
#[test]
fn rejects_v1_ticket_post_cutover() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa2u8; 16];
    let users = new_user_state_map();
    users.insert(user_id, state(0));
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();

    // Hand-craft a minimal v1 envelope: version byte = 1, a 12-byte nonce,
    // ct_len = 0, empty ciphertext, and a 16-byte tag. The version gate fires
    // BEFORE decryption, so the ciphertext/tag contents are irrelevant — only
    // the leading version byte matters here.
    let v1_wire = TicketWire {
        version: 1,
        nonce: [0u8; 12],
        ciphertext: Vec::new(),
        tag: [0u8; 16],
    };
    let v1_bytes = v1_wire.to_bytes();
    assert_eq!(v1_bytes[0], 1, "sanity: first byte is the v1 version");

    let req = ResumeRequest {
        ticket_wire_bytes: &v1_bytes,
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
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    );
    assert!(
        matches!(result, Err(shamir_connect::Error::AuthFailed)),
        "a v1 ticket must be rejected post-cutover (version gate)"
    );
    assert_eq!(store.len(), 0, "no session should be created");
}

/// Task #558 red test #2 — the DIRECT regression test for the bug this task
/// closes. Simulate an account that WAS a superuser when its ticket was minted
/// but has since been revoked. Crucially, `tickets_invalid_before_ns` is left
/// at 0 (NOT bumped past `original_auth_at_ns`), so the existing epoch check
/// alone would NOT catch this — only the directory re-lookup does. The resumed
/// session MUST resolve to `is_superuser == false`, proving the lookup-based
/// path closes the grant-revocation gap independently of the epoch mechanism.
#[test]
fn revoked_superuser_resolves_to_non_admin_without_epoch_bump() {
    let cfg = fixed_config();
    let counters = InMemoryConsumedCounters::new();
    let user_id = [0xa3u8; 16];
    let users = new_user_state_map();
    // The directory's CURRENT state: superuser revoked. `tickets_invalid_before_ns`
    // stays 0 — deliberately NOT bumped past the ticket's `original_auth_at_ns`,
    // so the §5.4 step 9 epoch check does NOT fire.
    users.insert(
        user_id,
        ResumeUserState {
            username: "was-admin".into(),
            roles: vec!["read_write".to_string()],
            superuser: false,
            tickets_invalid_before_ns: 0,
        },
    );
    let store = SessionStore::new();

    let now = UnixNanos::now().as_u64();
    // The ticket (v2) carries no roles/superuser at all; even if it could, the
    // directory now says superuser == false.
    let (ticket_bytes, _) = issue_initial_ticket(
        &cfg.ticket_key,
        user_id,
        "was-admin".into(),
        BindingMode::TlsExporter.as_u8(),
        BindingMode::TlsExporter.as_u8(),
        [0x77u8; 32],
        0,   // identity_key_version
        now, // original_auth_at_ns == now; epoch is 0, so 0 < now is NOT a reject
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
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &ServerIdentityState::fresh(),
        24 * ns::HOUR,
        TICKET_TTL,
        now,
    )
    .unwrap();

    let session = store.lookup(&ok.session_id).expect("session created");
    assert!(
        !session.permissions.is_superuser,
        "a revoked superuser must NOT retain admin powers on resume — the \
         session must reflect the directory's current superuser=false, even \
         though no epoch bump ran (task #558 closes this gap)"
    );
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
    users.insert(user_id, state(0));
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
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        TICKET_TTL,
        now,
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
    users.insert(user_id, state(0));
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
        &req,
        &cfg,
        &counters,
        &users,
        &store,
        &identity,
        24 * ns::HOUR,
        TICKET_TTL,
        now,
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
    users.insert(user_id, state(0));
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
        identity_key_version: u64,
    }

    let mirror = VecMirror {
        version: 2,
        user_id: vec![0x01u8; 16],
        username_nfc: "alice".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: vec![0x77u8; 32],
        ticket_family_id: vec![0x11u8; 16],
        original_auth_at_ns: 1_000_000,
        expires_at_ns: 2_000_000,
        family_counter: 1,
        identity_key_version: 0,
    };

    use shamir_connect::server::ticket::TicketPlain;
    let real = TicketPlain {
        version: 2,
        user_id: serde_bytes::ByteArray::new([0x01u8; 16]),
        username_nfc: "alice".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: serde_bytes::ByteArray::new([0x77u8; 32]),
        ticket_family_id: serde_bytes::ByteArray::new([0x11u8; 16]),
        original_auth_at_ns: 1_000_000,
        expires_at_ns: 2_000_000,
        family_counter: 1,
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
