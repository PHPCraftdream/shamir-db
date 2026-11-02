//! Token WebSocket loop tests.

use serde::Deserialize;
use serde_bytes;

use super::super::binding::{encode_binding, session_deadline};
use super::super::token_loop::{encode_prereserved, ErrorReply};

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
    let value: rmpv::Value = rmp_serde::from_slice(&bytes).expect("binding msgpack");
    let rmpv::Value::Map(fields) = value else {
        panic!("binding is map");
    };
    let keys: Vec<String> = fields
        .iter()
        .map(|(key, _)| key.as_str().expect("string key").to_owned())
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().any(|key| key == "ok"));
    assert!(keys.iter().any(|key| key == "sid"));
    assert!(keys.iter().any(|key| key == "expires_at_ns"));
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
}

#[test]
fn loop_wire_error_uses_optional_rid_and_length_prefixed_shared_buffer() {
    let bytes = encode_prereserved(&ErrorReply {
        rid: Some(4),
        error: "invalid_request",
    })
    .expect("encode");
    let size = u32::from_be_bytes(bytes[..4].try_into().expect("prefix")) as usize;
    assert_eq!(size, bytes.len() - 4);
    let value: rmpv::Value = rmp_serde::from_slice(&bytes[4..]).expect("msgpack");
    assert!(matches!(value, rmpv::Value::Map(_)));
}
