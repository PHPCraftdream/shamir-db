//! Tests for crypto primitive wrappers.
//!
//! Where possible, vectors are pulled from upstream RFCs to confirm we wire
//! the libraries correctly (rather than testing the libraries themselves).

use crate::common::crypto::{
    aes256gcm_decrypt, aes256gcm_encrypt, argon2id, constant_time_eq, ed25519_verify_strict,
    hkdf_sha256, hmac_sha256, random_array, random_bytes, sha256, Ed25519Keypair,
};
use crate::common::kdf_params::KdfParams;

#[test]
fn sha256_empty_string_rfc6234() {
    // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    let h = sha256(b"");
    let expected =
        hex::decode("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855").unwrap();
    assert_eq!(&h[..], &expected[..]);
}

#[test]
fn sha256_abc_rfc6234() {
    // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
    let h = sha256(b"abc");
    let expected =
        hex::decode("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad").unwrap();
    assert_eq!(&h[..], &expected[..]);
}

#[test]
fn hmac_sha256_rfc4231_test_case_1() {
    // RFC 4231 §4.2 Test Case 1
    // Key  = 0x0b * 20
    // Data = "Hi There"
    // Expected = b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
    let key = [0x0b; 20];
    let tag = hmac_sha256(&key, b"Hi There");
    let expected =
        hex::decode("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7").unwrap();
    assert_eq!(&tag[..], &expected[..]);
}

#[test]
fn hmac_sha256_rfc4231_test_case_2() {
    // RFC 4231 §4.3 Test Case 2 — JKE
    // Key  = "Jefe"
    // Data = "what do ya want for nothing?"
    // Expected = 5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843
    let tag = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let expected =
        hex::decode("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843").unwrap();
    assert_eq!(&tag[..], &expected[..]);
}

#[test]
fn hkdf_sha256_rfc5869_test_case_1() {
    // RFC 5869 §A.1 Test Case 1
    // IKM    = 0x0b * 22
    // salt   = 0x000102030405060708090a0b0c
    // info   = 0xf0f1f2f3f4f5f6f7f8f9
    // L      = 42
    // OKM    = 3cb25f25faacd57a90434f64d0362f2a
    //          2d2d0a90cf1a5a4c5db02d56ecc4c5bf
    //          34007208d5b887185865
    let ikm = [0x0b; 22];
    let salt = hex::decode("000102030405060708090a0b0c").unwrap();
    let info = hex::decode("f0f1f2f3f4f5f6f7f8f9").unwrap();
    let mut okm = [0u8; 42];
    hkdf_sha256(&ikm, &salt, &info, &mut okm).unwrap();

    let expected = hex::decode(
        "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865",
    )
    .unwrap();
    assert_eq!(&okm[..], &expected[..]);
}

#[test]
fn hkdf_sha256_zero_length_salt_per_rfc5869_section_2_2() {
    // Empty salt is valid; HKDF treats it as HashLen of zeros.
    let mut okm = [0u8; 32];
    hkdf_sha256(b"some ikm", &[], b"info", &mut okm).unwrap();
    // Just sanity: output is non-zero and deterministic.
    assert!(okm.iter().any(|&b| b != 0));

    let mut okm2 = [0u8; 32];
    hkdf_sha256(b"some ikm", &[], b"info", &mut okm2).unwrap();
    assert_eq!(okm, okm2);
}

#[test]
fn argon2id_default_params_runs() {
    // Smoke test — actual derive with default params (~2s on CI is OK for one test).
    // We don't pin the output bytes here because Argon2id may have subtle library
    // implementation choices; spec test vectors will live in test-vectors/ later.
    let salt = [0x55u8; 16];
    let result = argon2id(b"hello world!1", &salt, &KdfParams::DEFAULT);
    assert!(result.is_ok(), "argon2id default must succeed");
    let salted = result.unwrap();
    assert_eq!(salted.len(), 32);
    assert!(salted.iter().any(|&b| b != 0));
}

#[test]
fn argon2id_rejects_bad_version() {
    let mut bad = KdfParams::DEFAULT;
    bad.argon2_version = 0x10;
    let result = argon2id(b"x", &[0u8; 16], &bad);
    assert!(result.is_err());
}

#[test]
fn argon2id_deterministic_for_same_inputs() {
    // Use a small parameter set to keep the test fast.
    let small = KdfParams {
        memory_kb: 19_456, // OWASP minimum
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let salt = [0xaau8; 16];
    let a = argon2id(b"password", &salt, &small).unwrap();
    let b = argon2id(b"password", &salt, &small).unwrap();
    assert_eq!(a.as_ref(), b.as_ref());
}

#[test]
fn ed25519_sign_and_verify_round_trip() {
    let kp = Ed25519Keypair::generate();
    let pub_key = kp.public_bytes();
    let msg = b"some message to sign";
    let sig = kp.sign(msg);

    assert!(ed25519_verify_strict(&pub_key, msg, &sig));
    assert_eq!(sig.len(), 64);
    assert_eq!(pub_key.len(), 32);
}

#[test]
fn ed25519_verify_rejects_tampered_message() {
    let kp = Ed25519Keypair::generate();
    let pub_key = kp.public_bytes();
    let sig = kp.sign(b"original");
    assert!(!ed25519_verify_strict(&pub_key, b"tampered", &sig));
}

#[test]
fn ed25519_verify_rejects_tampered_signature() {
    let kp = Ed25519Keypair::generate();
    let pub_key = kp.public_bytes();
    let mut sig = kp.sign(b"msg");
    sig[0] ^= 0xff;
    assert!(!ed25519_verify_strict(&pub_key, b"msg", &sig));
}

#[test]
fn ed25519_verify_rejects_wrong_public_key() {
    let kp1 = Ed25519Keypair::generate();
    let kp2 = Ed25519Keypair::generate();
    let sig = kp1.sign(b"msg");
    assert!(!ed25519_verify_strict(&kp2.public_bytes(), b"msg", &sig));
}

#[test]
fn ed25519_from_seed_deterministic() {
    let seed = [0x42u8; 32];
    let kp1 = Ed25519Keypair::from_seed(&seed);
    let kp2 = Ed25519Keypair::from_seed(&seed);
    assert_eq!(kp1.public_bytes(), kp2.public_bytes());

    let msg = b"reproducible";
    // Ed25519 is deterministic per RFC 8032 — same key + same message → same sig.
    assert_eq!(kp1.sign(msg), kp2.sign(msg));
}

#[test]
fn aes256gcm_round_trip() {
    let key = [0x77u8; 32];
    let nonce = [0x11u8; 12];
    let plaintext = b"hello shamir-connect";
    let aad = b"aad-context";

    let ct = aes256gcm_encrypt(&key, &nonce, plaintext, aad).unwrap();
    assert_eq!(ct.len(), plaintext.len() + 16); // tag is 16 bytes

    let pt = aes256gcm_decrypt(&key, &nonce, &ct, aad).unwrap();
    assert_eq!(&pt[..], plaintext);
}

#[test]
fn aes256gcm_rejects_tampered_ciphertext() {
    let key = [0x77u8; 32];
    let nonce = [0x11u8; 12];
    let mut ct = aes256gcm_encrypt(&key, &nonce, b"hello", b"aad").unwrap();
    ct[0] ^= 0xff;
    assert!(aes256gcm_decrypt(&key, &nonce, &ct, b"aad").is_err());
}

#[test]
fn aes256gcm_rejects_tampered_aad() {
    let key = [0x77u8; 32];
    let nonce = [0x11u8; 12];
    let ct = aes256gcm_encrypt(&key, &nonce, b"hello", b"aad-original").unwrap();
    assert!(aes256gcm_decrypt(&key, &nonce, &ct, b"aad-tampered").is_err());
}

#[test]
fn aes256gcm_rejects_wrong_key() {
    let nonce = [0x11u8; 12];
    let ct = aes256gcm_encrypt(&[0x77u8; 32], &nonce, b"hello", b"aad").unwrap();
    assert!(aes256gcm_decrypt(&[0x88u8; 32], &nonce, &ct, b"aad").is_err());
}

#[test]
fn aes256gcm_rejects_wrong_nonce() {
    let key = [0x77u8; 32];
    let ct = aes256gcm_encrypt(&key, &[0x11u8; 12], b"hello", b"aad").unwrap();
    assert!(aes256gcm_decrypt(&key, &[0x22u8; 12], &ct, b"aad").is_err());
}

#[test]
fn constant_time_eq_returns_true_for_equal_bytes() {
    assert!(constant_time_eq(&[1, 2, 3, 4], &[1, 2, 3, 4]));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn constant_time_eq_returns_false_for_different_bytes() {
    assert!(!constant_time_eq(&[1, 2, 3, 4], &[1, 2, 3, 5]));
}

#[test]
fn constant_time_eq_returns_false_for_different_length() {
    assert!(!constant_time_eq(&[1, 2, 3], &[1, 2, 3, 4]));
}

#[test]
fn random_bytes_distinct_each_call() {
    let a = random_array::<32>();
    let b = random_array::<32>();
    assert_ne!(a, b, "two CSPRNG calls produced same bytes — broken RNG");
    assert!(a.iter().any(|&v| v != 0), "all-zero CSPRNG output");
}

#[test]
fn random_bytes_into_slice() {
    let mut buf = [0u8; 64];
    random_bytes(&mut buf);
    assert!(buf.iter().any(|&v| v != 0));
}
