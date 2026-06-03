//! Per-function `/crypto` tests — at least one correct-result assert (against a
//! published known-answer vector) and one error/edge case per registered fn.

use crate::crypto;
use crate::registry::{v_bool, ScalarRegistry};
use shamir_types::types::value::InnerValue;

fn reg() -> ScalarRegistry {
    let mut r = ScalarRegistry::new();
    crypto::register(&mut r);
    r
}

fn bin(b: &[u8]) -> InnerValue {
    InnerValue::Bin(b.to_vec())
}

fn out(v: InnerValue) -> Vec<u8> {
    match v {
        InnerValue::Bin(b) => b,
        other => panic!("expected Bin, got {other:?}"),
    }
}

#[test]
fn sha256_known_answer_and_type_error() {
    let r = reg();
    // SHA-256("") known-answer vector.
    let got = out(r.call("sha256", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(got.len(), 32);
    // error: wrong type (Str, not Bin).
    assert_eq!(
        r.call("sha256", &[InnerValue::Str("x".into())])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn sha512_known_answer_and_arity() {
    let r = reg();
    // SHA-512("") known-answer vector.
    let got = out(r.call("sha512", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
         47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
    );
    assert_eq!(got.len(), 64);
    // error: no args -> arity.
    assert_eq!(r.call("sha512", &[]).unwrap_err().code, "arity");
}

#[test]
fn sha3_256_known_answer_and_type_error() {
    let r = reg();
    // SHA3-256("") known-answer vector.
    let got = out(r.call("sha3_256", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a"
    );
    assert_eq!(got.len(), 32);
    // error: too many args -> arity.
    assert_eq!(
        r.call("sha3_256", &[bin(b"a"), bin(b"b")])
            .unwrap_err()
            .code,
        "arity"
    );
}

#[test]
fn blake3_known_answer_and_type_error() {
    let r = reg();
    // BLAKE3("") known-answer vector.
    let got = out(r.call("blake3", &[bin(b"")]).unwrap());
    assert_eq!(
        hex_lower(&got),
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
    );
    assert_eq!(got.len(), 32);
    // error: wrong type.
    assert_eq!(
        r.call("blake3", &[InnerValue::Int(3)]).unwrap_err().code,
        "type_mismatch"
    );
}

#[test]
fn hmac_sha256_known_answer_and_arity() {
    let r = reg();
    // RFC 4231 Test Case 2: key = "Jefe", data = "what do ya want for nothing?".
    let got = out(r
        .call(
            "hmac_sha256",
            &[bin(b"Jefe"), bin(b"what do ya want for nothing?")],
        )
        .unwrap());
    assert_eq!(
        hex_lower(&got),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(got.len(), 32);
    // error: missing message arg -> arity.
    assert_eq!(
        r.call("hmac_sha256", &[bin(b"key")]).unwrap_err().code,
        "arity"
    );
    // error: wrong type for key.
    assert_eq!(
        r.call("hmac_sha256", &[InnerValue::Int(1), bin(b"msg")])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

#[test]
fn ct_eq_equal_unequal_and_length_mismatch() {
    let r = reg();
    // Equal contents -> true.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abc")]).unwrap(),
        v_bool(true)
    );
    // Differing contents, same length -> false.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abd")]).unwrap(),
        v_bool(false)
    );
    // Length mismatch -> false (definite inequality).
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), bin(b"abcd")]).unwrap(),
        v_bool(false)
    );
    // error: wrong type.
    assert_eq!(
        r.call("ct_eq", &[bin(b"abc"), InnerValue::Bool(true)])
            .unwrap_err()
            .code,
        "type_mismatch"
    );
}

/// Lowercase hex helper local to the tests (avoids depending on the `hex` dep
/// from inside this module's tests).
fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
