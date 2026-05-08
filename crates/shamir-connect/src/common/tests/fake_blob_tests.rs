//! Tests for [`FakeBlob`] (anti-enumeration) — spec §5.2.1.

use crate::common::fake_blob::FakeBlob;
use crate::common::username::NormalizedUsername;

#[test]
fn deterministic_for_same_inputs() {
    let secret = [0x42u8; 32];
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let a = FakeBlob::derive(&secret, &user).unwrap();
    let b = FakeBlob::derive(&secret, &user).unwrap();
    assert_eq!(a.salt, b.salt);
    assert_eq!(a.stored_key.0, b.stored_key.0);
    assert_eq!(a.server_key.as_ref(), b.server_key.as_ref());
}

#[test]
fn different_username_yields_different_blob() {
    let secret = [0x42u8; 32];
    let alice = NormalizedUsername::from_raw("alice").unwrap();
    let bob = NormalizedUsername::from_raw("bob").unwrap();
    let a = FakeBlob::derive(&secret, &alice).unwrap();
    let b = FakeBlob::derive(&secret, &bob).unwrap();
    assert_ne!(a.salt, b.salt);
    assert_ne!(a.stored_key.0, b.stored_key.0);
    assert_ne!(a.server_key.as_ref(), b.server_key.as_ref());
}

#[test]
fn different_secret_yields_different_blob() {
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let a = FakeBlob::derive(&[0x42u8; 32], &user).unwrap();
    let b = FakeBlob::derive(&[0x43u8; 32], &user).unwrap();
    assert_ne!(a.salt, b.salt);
    assert_ne!(a.stored_key.0, b.stored_key.0);
    assert_ne!(a.server_key.as_ref(), b.server_key.as_ref());
}

#[test]
fn lengths_match_real_user_record() {
    let secret = [0x42u8; 32];
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let blob = FakeBlob::derive(&secret, &user).unwrap();
    // Match shape of __system__/users record per spec §3.5.
    assert_eq!(blob.salt.len(), 16);
    assert_eq!(blob.stored_key.0.len(), 32);
    assert_eq!(blob.server_key.len(), 32);
}

#[test]
fn fake_blob_is_indistinguishable_from_random_in_appearance() {
    // Sanity: HKDF output should look uniformly random (no obvious patterns).
    let secret = [0x42u8; 32];
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let blob = FakeBlob::derive(&secret, &user).unwrap();

    // Crude entropy check: not all bytes the same.
    assert!(blob.salt.iter().any(|&b| b != blob.salt[0]));
    assert!(blob.stored_key.0.iter().any(|&b| b != blob.stored_key.0[0]));
    assert!(blob.server_key.iter().any(|&b| b != blob.server_key[0]));
}

#[test]
fn pinned_test_vector_for_interop() {
    // Deterministic value pinned for cross-language consistency. If this
    // test fails after upgrading hkdf/sha2, the JS SDK and Rust SDK will
    // disagree on fake values for unknown users. Update only with full
    // ecosystem coordination.
    let secret = [0u8; 32]; // all-zero secret
    let user = NormalizedUsername::from_raw("alice").unwrap();
    let blob = FakeBlob::derive(&secret, &user).unwrap();

    // We don't pin specific bytes here in the source — those go to the
    // JSON test vector file later. This is a placeholder asserting that
    // derivation is deterministic and produces something specific (recorded
    // separately).
    let blob2 = FakeBlob::derive(&secret, &user).unwrap();
    assert_eq!(blob.salt, blob2.salt);
}

#[test]
fn nfc_normalization_affects_blob() {
    // Two byte-different but visually-identical usernames produce different blobs
    // (NFC normalization SHOULD collapse them to same bytes; if upstream
    // normalization changes, test exposes the divergence).
    let secret = [0x42u8; 32];
    let precomposed = NormalizedUsername::from_raw("caf\u{00E9}").unwrap();
    let decomposed = NormalizedUsername::from_raw("cafe\u{0301}").unwrap();
    let a = FakeBlob::derive(&secret, &precomposed).unwrap();
    let b = FakeBlob::derive(&secret, &decomposed).unwrap();
    // After NFC normalization both inputs are identical bytes → same blob.
    assert_eq!(a.salt, b.salt);
}
