//! Integration tests for admin commands (spec §12).

use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::admin::{
    create_user, kick_session, unlock_user, update_user, CreateUserInput, InMemoryAuditSink,
    InMemoryUserDirectory, UserDirectory,
};
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_connect::server::user_record::UserRecord;

fn fast_kdf() -> KdfParams {
    KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

fn make_admin_session(user_id: [u8; 16]) -> Session {
    Session::new(
        user_id,
        "admin".into(),
        SessionPermissions::from_roles(vec!["superuser".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

fn make_normal_session(user_id: [u8; 16], username: &str) -> Session {
    Session::new(
        user_id,
        username.into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0u8; 32],
        UnixNanos::now().as_u64(),
    )
}

fn fixture_user_record() -> UserRecord {
    UserRecord {
        salt: [0x55u8; 16],
        stored_key: StoredKey([0xaau8; 32]),
        server_key: zeroize::Zeroizing::new([0xbbu8; 32]),
        kdf_params: fast_kdf(),
        tickets_invalid_before_ns: 0,
    }
}

#[test]
fn create_user_round_trip() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin_session = make_admin_session([0u8; 16]);

    let input = CreateUserInput {
        username: "alice".into(),
        salt: [0x11u8; 16],
        stored_key: [0x22u8; 32],
        server_key: [0x33u8; 32],
        kdf_params: fast_kdf(),
        roles: vec!["read_write".into()],
    };

    let uid = create_user(&admin_session, input, &fast_kdf(), &dir, &audit).unwrap();
    assert_ne!(uid, [0u8; 16]);

    // Subsequent lookup works.
    assert!(dir.lookup_by_name("alice").is_some());
    assert_eq!(dir.user_id("alice"), Some(uid));

    let events = audit.snapshot();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "user_created");
    assert_eq!(events[0].actor, "admin");
}

#[test]
fn create_user_rejects_non_superuser() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let normal = make_normal_session([1u8; 16], "alice");

    let input = CreateUserInput {
        username: "bob".into(),
        salt: [0u8; 16],
        stored_key: [0u8; 32],
        server_key: [0u8; 32],
        kdf_params: fast_kdf(),
        roles: vec![],
    };

    let result = create_user(&normal, input, &fast_kdf(), &dir, &audit);
    assert!(result.is_err());
}

#[test]
fn create_user_rejects_duplicate_username() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);

    dir.preinsert("alice".into(), [9u8; 16], fixture_user_record());

    let input = CreateUserInput {
        username: "alice".into(),
        salt: [0u8; 16],
        stored_key: [0u8; 32],
        server_key: [0u8; 32],
        kdf_params: fast_kdf(),
        roles: vec![],
    };

    let result = create_user(&admin, input, &fast_kdf(), &dir, &audit);
    assert!(result.is_err());
}

#[test]
fn create_user_rejects_kdf_below_floor() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);

    let weak = KdfParams {
        memory_kb: 1024, // way below floor
        time: 1,
        parallelism: 1,
        argon2_version: 0x13,
    };

    let input = CreateUserInput {
        username: "alice".into(),
        salt: [0u8; 16],
        stored_key: [0u8; 32],
        server_key: [0u8; 32],
        kdf_params: weak,
        roles: vec![],
    };

    let result = create_user(&admin, input, &weak, &dir, &audit);
    assert!(result.is_err());
}

#[test]
fn create_user_rejects_kdf_mismatch_with_current() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);

    let mut other = fast_kdf();
    other.time += 1;

    let input = CreateUserInput {
        username: "alice".into(),
        salt: [0u8; 16],
        stored_key: [0u8; 32],
        server_key: [0u8; 32],
        kdf_params: other,
        roles: vec![],
    };

    let result = create_user(&admin, input, &fast_kdf(), &dir, &audit);
    assert!(result.is_err());
}

#[test]
fn kick_session_kills_target_sessions_and_bumps_invalidation() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);
    let alice_uid = [0xaau8; 16];
    dir.preinsert("alice".into(), alice_uid, fixture_user_record());

    let store = SessionStore::new();
    store.insert([0xa1u8; 32], make_normal_session(alice_uid, "alice"));
    store.insert([0xa2u8; 32], make_normal_session(alice_uid, "alice"));
    store.insert(
        [0xb1u8; 32],
        make_normal_session([0xbbu8; 16], "bob"),
    );

    let now = UnixNanos::now().as_u64();
    let result = kick_session(&admin, "alice", now, &dir, &store, &audit).unwrap();
    assert_eq!(result.killed_count, 2);
    assert_eq!(store.len(), 1); // bob remains

    let record = dir.lookup_by_name("alice").unwrap();
    assert_eq!(record.tickets_invalid_before_ns, now);

    let events = audit.snapshot();
    assert!(events.iter().any(|e| e.event == "kick_session"));
}

#[test]
fn kick_session_rejects_non_superuser() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let store = SessionStore::new();
    let normal = make_normal_session([1u8; 16], "alice");

    let result = kick_session(
        &normal,
        "alice",
        UnixNanos::now().as_u64(),
        &dir,
        &store,
        &audit,
    );
    assert!(result.is_err());
}

#[test]
fn update_user_with_new_roles_kills_sessions_and_bumps() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);
    let alice_uid = [0xaau8; 16];
    dir.preinsert("alice".into(), alice_uid, fixture_user_record());

    let store = SessionStore::new();
    store.insert([0xa1u8; 32], make_normal_session(alice_uid, "alice"));

    let now = UnixNanos::now().as_u64();
    let result =
        update_user(&admin, "alice", Some(vec!["read_only".into()]), now, &dir, &store, &audit)
            .unwrap();
    assert!(result.changes_applied);
    assert_eq!(store.len(), 0);

    let events = audit.snapshot();
    assert!(events.iter().any(|e| e.event == "roles_changed"));
}

#[test]
fn update_user_noop_does_not_kill_sessions_or_bump() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);
    let alice_uid = [0xaau8; 16];
    dir.preinsert("alice".into(), alice_uid, fixture_user_record());

    let store = SessionStore::new();
    store.insert([0xa1u8; 32], make_normal_session(alice_uid, "alice"));

    let result = update_user(
        &admin,
        "alice",
        None, // no roles → noop
        UnixNanos::now().as_u64(),
        &dir,
        &store,
        &audit,
    )
    .unwrap();

    assert!(!result.changes_applied);
    assert_eq!(store.len(), 1, "noop must NOT kill sessions");
    let record = dir.lookup_by_name("alice").unwrap();
    assert_eq!(record.tickets_invalid_before_ns, 0);

    let events = audit.snapshot();
    assert!(events.iter().any(|e| e.event == "update_user_noop"));
}

#[test]
fn update_user_rejects_non_superuser() {
    let dir = InMemoryUserDirectory::new();
    let audit = InMemoryAuditSink::new();
    let store = SessionStore::new();
    let normal = make_normal_session([1u8; 16], "alice");

    let result = update_user(
        &normal,
        "alice",
        Some(vec!["read_only".into()]),
        UnixNanos::now().as_u64(),
        &dir,
        &store,
        &audit,
    );
    assert!(result.is_err());
}

#[test]
fn unlock_user_clears_state_and_audits() {
    let audit = InMemoryAuditSink::new();
    let admin = make_admin_session([0u8; 16]);

    let mut cleared = String::new();
    unlock_user(&admin, "alice", |u| cleared = u.to_string(), &audit).unwrap();
    assert_eq!(cleared, "alice");

    let events = audit.snapshot();
    assert!(events.iter().any(|e| e.event == "lockout_released"));
}

#[test]
fn unlock_user_rejects_non_superuser() {
    let audit = InMemoryAuditSink::new();
    let normal = make_normal_session([1u8; 16], "alice");

    let result = unlock_user(&normal, "alice", |_| {}, &audit);
    assert!(result.is_err());
}
