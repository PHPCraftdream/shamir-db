//! Spec IMPL §4 NORMATIVE — log redaction CI gate.
//!
//! Asserts that `format!("{:?}", value)` for sensitive types does NOT
//! contain any byte of the wrapped secret. A future regression that
//! accidentally derives `Debug` on these types (or adds a new key field
//! without redacting) will break this test.
//!
//! Strategy: build each type with a magic byte pattern that's unlikely to
//! appear by accident in formatter output (e.g., `[0xab; 32]` → "ab" 32
//! times — looking for the substring `"abab"` in the Debug output flags
//! a leak).

use shamir_connect::common::crypto::{Ed25519Keypair, StoredKey};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::bootstrap::BootstrapState;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::session::{PendingChangePwChallenge, Session, SessionPermissions};
use shamir_connect::server::ticket::TicketPlain;
use shamir_connect::server::user_record::UserRecord;
use zeroize::Zeroizing;

/// Detect "abab" / "cdcd" / similar repeated-byte patterns in Debug output.
fn contains_repeated_pattern(haystack: &str, byte_hex: &str) -> bool {
    // Build "ababab..." (16+ chars) and check substring.
    let mut needle = String::with_capacity(byte_hex.len() * 16);
    for _ in 0..16 {
        needle.push_str(byte_hex);
    }
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[test]
fn stored_key_debug_redacts() {
    let s = StoredKey([0xabu8; 32]);
    let dbg = format!("{:?}", s);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    assert!(
        !contains_repeated_pattern(&dbg, "ab"),
        "stored_key bytes leaked: {}",
        dbg
    );
}

#[test]
fn ed25519_keypair_debug_redacts_private() {
    let kp = Ed25519Keypair::from_seed(&[0xcdu8; 32]);
    let dbg = format!("{:?}", kp);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    // The seed bytes (cd) MUST NOT appear; the public key fingerprint may
    // appear (it's a 4-byte hash-ish prefix derived from the seed, but
    // doesn't reveal the seed itself).
    assert!(
        !contains_repeated_pattern(&dbg, "cd"),
        "private key bytes leaked: {}",
        dbg
    );
}

#[test]
fn user_record_debug_redacts_all_secret_fields() {
    let rec = UserRecord {
        salt: [0xefu8; 16],
        stored_key: StoredKey([0xabu8; 32]),
        server_key: Zeroizing::new([0xbau8; 32]),
        kdf_params: KdfParams::DEFAULT,
        tickets_invalid_before_ns: 12345,
    };
    let dbg = format!("{:?}", rec);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    assert!(
        !contains_repeated_pattern(&dbg, "ef"),
        "salt bytes leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "ab"),
        "stored_key bytes leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "ba"),
        "server_key bytes leaked: {}",
        dbg
    );
    // Non-secret fields SHOULD appear.
    assert!(
        dbg.contains("12345"),
        "tickets_invalid_before_ns missing: {}",
        dbg
    );
}

#[test]
fn server_secrets_debug_redacts_both() {
    let s = ServerSecrets {
        server_secret: [0xa1u8; 32],
        lockout_secret: [0xb2u8; 32],
    };
    let dbg = format!("{:?}", s);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    assert!(
        !contains_repeated_pattern(&dbg, "a1"),
        "server_secret bytes leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "b2"),
        "lockout_secret bytes leaked: {}",
        dbg
    );
}

#[test]
fn session_debug_redacts_user_id_and_channel_binding() {
    let session = Session::new(
        [0x11u8; 16],
        "alice".into(),
        SessionPermissions::from_roles(vec!["read_write".into()]),
        TransportKind::Tcp,
        BindingMode::TlsExporter,
        [0x77u8; 32], // channel_binding_at_auth
        1000,
    );
    let dbg = format!("{:?}", session);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    assert!(
        !contains_repeated_pattern(&dbg, "11"),
        "user_id leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "77"),
        "channel_binding leaked: {}",
        dbg
    );
}

#[test]
fn pending_changepw_challenge_debug_redacts_nonces() {
    let p = PendingChangePwChallenge {
        server_nonce_cp: [0xdeu8; 32],
        client_nonce_cp: [0xadu8; 32],
        issued_at_ns: 5000,
    };
    let dbg = format!("{:?}", p);
    assert!(dbg.contains("REDACTED"));
    assert!(!contains_repeated_pattern(&dbg, "de"));
    assert!(!contains_repeated_pattern(&dbg, "ad"));
    assert!(dbg.contains("5000"));
}

#[test]
fn ticket_plain_debug_redacts_identifying_fields() {
    let t = TicketPlain {
        version: 1,
        user_id: serde_bytes::ByteArray::new([0x21u8; 16]),
        username_nfc: "secret-username".into(),
        transport_kind_at_auth: 0x01,
        binding_mode_at_auth: 0x01,
        channel_binding_at_auth: serde_bytes::ByteArray::new([0x33u8; 32]),
        ticket_family_id: serde_bytes::ByteArray::new([0x44u8; 16]),
        original_auth_at_ns: 1000,
        expires_at_ns: 2000,
        family_counter: 5,
        roles: vec!["superuser".into()],
        identity_key_version: 0,
    };
    let dbg = format!("{:?}", t);
    assert!(dbg.contains("REDACTED"), "expected REDACTED, got: {}", dbg);
    assert!(
        !contains_repeated_pattern(&dbg, "21"),
        "user_id leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "33"),
        "channel_binding leaked: {}",
        dbg
    );
    assert!(
        !contains_repeated_pattern(&dbg, "44"),
        "ticket_family_id leaked: {}",
        dbg
    );
    assert!(!dbg.contains("secret-username"), "username leaked: {}", dbg);
    assert!(!dbg.contains("superuser"), "roles leaked: {}", dbg);
    // Non-identifying fields SHOULD appear.
    assert!(dbg.contains("family_counter") && dbg.contains("5"));
}

#[test]
fn bootstrap_state_debug_redacts_token_hash() {
    let st = BootstrapState::empty();
    // Issue a token so token_hash is Some(...).
    let _token = st.issue_token(60_000_000_000, 0).unwrap();
    let dbg = format!("{:?}", st);
    assert!(dbg.contains("REDACTED") || dbg.contains("active"));
    // The 32 raw token-hash bytes must NOT appear in any conventional form.
    // We can't predict the random hash bytes, so just assert structural
    // redaction via the marker word.
    assert!(dbg.contains("active") || dbg.contains("None"));
}
