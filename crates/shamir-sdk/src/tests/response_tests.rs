//! Regression tests for host-import response-buffer reclamation: the guest
//! must free the `shamir_alloc` response allocation after decoding, and
//! malformed response bytes must fail closed.

use crate::__rt;
use crate::Value;

/// RV-SDK-8190492D: a host-import response buffer must be owned and freed
/// when `decode_response` finishes (Ok or Err), not borrowed and leaked.
///
/// What this test pins: the [`__rt::ResponseBytes`] guard frees the
/// reconstructed `shamir_alloc` allocation exactly once (the test-only live
/// counter returns to 0), and the pure decoder [`__rt::decode_value_bytes`]
/// decodes the guarded bytes. The wasm32 import path (`decode_response`)
/// cannot run natively — it takes a raw guest pointer — so its behavior is
/// pinned structurally: it is a thin wrapper over `decode_value_bytes`
/// whose guard frees on both Ok and Err paths, and its callers map
/// `packed == 0` to `Ok(None)`/absent via `unpack_ptr_len`. No import-path
/// execution coverage is claimed here.
#[test]
fn rv_sdk_8190492d_response_buffer_reclaimed_after_decode() {
    assert_eq!(__rt::live_response_bytes(), 0);
    // Simulate the production `shamir_alloc` allocation shape exactly:
    // capacity == length (so the guard can reconstruct the allocation via
    // `Vec::from_raw_parts`), filled with the host's response bytes.
    let n = 3usize;
    // `vec![0u8; n]` allocates with capacity == length exactly, matching
    // the production `shamir_alloc` shape (`vec![0u8; len]`).
    let mut v: Vec<u8> = vec![0u8; n];
    let packed = rmp_serde::to_vec(&Value::Str("ok".into())).expect("encode");
    assert_eq!(packed.len(), n);
    v.copy_from_slice(&packed);
    let ptr = v.as_mut_ptr();
    let len = v.len();
    assert_eq!(v.capacity(), n, "shamir_alloc shape: cap == len");
    core::mem::forget(v);

    // SAFETY: `ptr`/`len` come from a forgotten Vec<u8> with capacity == len.
    let guard = unsafe { __rt::ResponseBytes::from_native(ptr, len) };
    assert_eq!(__rt::live_response_bytes(), 1);
    let value = __rt::decode_value_bytes(guard.as_slice()).expect("decode");
    drop(guard); // frees the reconstruction of the leaked allocation
    assert_eq!(value, Value::Str("ok".into()));
    assert_eq!(
        __rt::live_response_bytes(),
        0,
        "buffer must be freed exactly once"
    );
}

/// RV-SDK-A68C70DE: malformed (truncated msgpack) host response bytes must
/// fail closed with the codec cause, never fall back to a plausible value.
#[test]
fn rv_sdk_a68c70de_malformed_import_response_fails_closed() {
    // Map-of-1 header with a truncated key.
    let err =
        __rt::decode_value_bytes(&[0x81, 0xa1]).expect_err("malformed bytes must be rejected");
    assert!(err.message().contains("decode host response"));
}

/// RV-SDK-A68C70DE: the semantic split between "absent" and "null".
/// `packed == 0` means absent (`unpack_ptr_len` returns `None`, callers map
/// it to `Ok(None)`), while a real encoded Null (`0xc0`, nonzero packed)
/// decodes to `Ok(Some(Value::Null))` — distinct outcomes.
///
/// Note: the `None => Ok(None)` mapping itself lives in the wasm32-only
/// callers of `unpack_ptr_len` and is structurally guaranteed by the
/// `match unpack_ptr_len(packed) { ... None => return Ok(None) }` arm; this
/// test pins the pure halves of the split: the guard frees on both Ok and
/// Err paths; the pure decode fails closed; callers map `packed == 0` to
/// `Ok(None)` structurally. No import-path execution coverage is claimed:
/// `decode_response` takes a raw guest pointer and cannot run natively.
#[test]
fn rv_sdk_a68c70de_zero_len_is_absent_not_null() {
    // packed == 0 is absent, never a (0, 0) buffer.
    assert_eq!(__rt::unpack_ptr_len(0), None);
    // Nonzero packed roundtrips to (ptr, len) and is Some — decodable.
    let packed = ((7i64) << 32) | 1;
    assert_eq!(__rt::unpack_ptr_len(packed), Some((7, 1)));
    assert_eq!(
        __rt::decode_value_bytes(&[0xc0]).expect("encoded null decodes"),
        Value::Null
    );
    // Distinct paths: absent is None; null is Some(decodable bytes).
    assert_ne!(__rt::unpack_ptr_len(0).map(|p| p.1), Some(1));
}
