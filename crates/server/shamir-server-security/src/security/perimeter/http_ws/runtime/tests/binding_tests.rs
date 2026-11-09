//! Token WebSocket loop tests.

use serde::Deserialize;

use super::super::binding::{encode_binding, session_deadline};

#[derive(Deserialize)]
struct Binding {
    ok: bool,
    #[serde(with = "serde_bytes")]
    sid: Vec<u8>,
    expires_at_ns: u64,
}

#[test]
fn binding_contains_exactly_the_expected_fields_and_nonzero_sid() {
    let mut sid = [0_u8; 32];
    sid[0] = 7;
    let bytes = encode_binding(&sid, 42).expect("binding serializes");
    let fields: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
        rmp_serde::from_slice(&bytes).expect("binding msgpack");
    assert_eq!(
        fields.keys().map(String::as_str).collect::<Vec<_>>(),
        ["expires_at_ns", "ok", "sid"]
    );
    let decoded: Binding = rmp_serde::from_slice(&bytes).expect("typed binding");
    assert!(decoded.ok);
    assert_eq!(decoded.sid, sid);
    assert_eq!(decoded.expires_at_ns, 42);
}

#[test]
fn session_deadline_saturates_overflow_to_token_expiry() {
    let token_expiry = u64::MAX - 1;
    assert_eq!(
        session_deadline(u64::MAX - 2, token_expiry, 1),
        token_expiry
    );
    assert_eq!(session_deadline(10, 100, 1), 100);
    let opened_at = 10_000_000_000_000;
    let token_expiry = u64::MAX;
    let expected = opened_at + 2 * 3_600_000_000_000;
    assert_eq!(session_deadline(opened_at, token_expiry, 2), expected);
}
