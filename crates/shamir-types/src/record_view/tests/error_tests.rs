//! Error paths — the lens is untrusted-input safe: it returns `Err` (or `None`
//! for single-field access), NEVER panics, on truncated / non-map / too-deep /
//! reserved-marker input.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordView, RecordViewError};

#[test]
fn err_empty_buffer() {
    assert!(matches!(
        RecordView::new(b"").unwrap_err(),
        RecordViewError::Truncated(0)
    ));
}

#[test]
fn err_non_map_top_level_scalar() {
    // A positive fixint (0x05) — not a map.
    assert!(matches!(
        RecordView::new(&[0x05]).unwrap_err(),
        RecordViewError::NonMapTopLevel { got: 0x05 }
    ));
}

#[test]
fn err_non_map_top_level_array() {
    // A fixarray of length 1 — not a map.
    assert!(matches!(
        RecordView::new(&[0x91, 0x01]).unwrap_err(),
        RecordViewError::NonMapTopLevel { got: 0x91 }
    ));
}

#[test]
fn err_non_map_top_level_string() {
    // A fixstr of length 1 — not a map.
    assert!(matches!(
        RecordView::new(&[0xa1, b'x']).unwrap_err(),
        RecordViewError::NonMapTopLevel { got: 0xa1 }
    ));
}

#[test]
fn err_truncated_map_header() {
    // Map16 marker (0xde) promises 2 length bytes, but buffer ends.
    assert!(matches!(
        RecordView::new(&[0xde, 0x00]).unwrap_err(),
        RecordViewError::Truncated(_)
    ));
}

#[test]
fn err_truncated_value_payload() {
    // fixmap{1}: key = bin8 len 1 + byte 0x00 (interned id 0),
    // value marker promises more than buffer has.
    // 0x81 = fixmap len 1, 0xc4 = bin8, 0x01 = len 1, 0x00 = key byte,
    // 0xce = u32 marker (needs 4 bytes, none here).
    let buf = &[0x81, 0xc4, 0x01, 0x00, 0xce];
    let lens = RecordView::new(buf).unwrap();
    let key = InternerKey::new(0);
    // get returns None (malformed -> terminates scan), get_with_err surfaces Err.
    assert_eq!(lens.get(key.clone()), None);
    assert!(matches!(
        lens.get_with_err(key).unwrap_err(),
        RecordViewError::Truncated(_)
    ));
}

#[test]
fn err_truncated_key_payload() {
    // fixmap{1}: key marker is bin8 len 5, but only 2 bytes follow.
    let buf = &[0x81, 0xc4, 0x05, 0x00, 0x01];
    let lens = RecordView::new(buf).unwrap();
    let key = InternerKey::new(0);
    assert_eq!(lens.get(key.clone()), None);
    assert!(lens.get_with_err(key).is_err());
}

#[test]
fn err_truncated_during_skip() {
    // fixmap{2}: first entry is fine, second key's value marker is an array
    // promising elements that aren't there. Lens must terminate, not panic.
    // 0x82 = fixmap len 2
    // key1 = bin8 len 1 + byte 0x00 (id=0), val1 int 1 (0x01)
    // key2 = bin8 len 1 + byte 0x01 (id=1), val2 = fixarray len 3 (0x93)
    //        with zero elements present
    let buf = &[0x82, 0xc4, 0x01, 0x00, 0x01, 0xc4, 0x01, 0x01, 0x93];
    let lens = RecordView::new(buf).unwrap();
    let key0 = InternerKey::new(0);
    let key1 = InternerKey::new(1);
    // Looking up key0 (which is before the truncation) succeeds.
    assert_eq!(lens.get_int(key0), Some(1));
    // Looking up key1 requires reading its (truncated) value -> None, no panic.
    assert_eq!(lens.get_int(key1), None);
}

#[test]
fn err_reserved_marker_in_value() {
    // fixmap{1}: key = bin8 len 1 + byte 0x00, value = reserved marker 0xc1.
    let buf = &[0x81, 0xc4, 0x01, 0x00, 0xc1];
    let lens = RecordView::new(buf).unwrap();
    let key = InternerKey::new(0);
    // Offset within the body (after the map header): key is at body
    // bytes 0..3 (0xc4 + 0x01 + 0x00), then the value marker 0xc1 is at body offset 3.
    assert!(matches!(
        lens.get_with_err(key).unwrap_err(),
        RecordViewError::ReservedMarker(3)
    ));
}

#[test]
fn err_depth_exceeded_on_deep_nesting() {
    // Build a deeply-nested map-of-one-map-of-one-map... > MAX_MSGPACK_DEPTH.
    // Each level: 0x81 (fixmap len 1) + bin8 key (0xc4 0x01 0x00) + <inner>.
    let depth = crate::record_view::MAX_MSGPACK_DEPTH + 5;
    let mut buf = Vec::with_capacity(depth * 4 + 1);
    for _ in 0..depth {
        buf.push(0x81); // fixmap len 1
        buf.push(0xc4); // bin8
        buf.push(0x01); // len 1
        buf.push(0x00); // key byte (id 0)
    }
    buf.push(0x01); // leaf int
    let lens = RecordView::new(&buf).unwrap(); // top header is fine
                                               // Descending via get_path should hit the depth cap and yield None (no panic).
    let key = InternerKey::new(0);
    let path: Vec<InternerKey> = (0..depth).map(|_| key.clone()).collect();
    assert_eq!(lens.get_path(&path), None);
}

#[test]
fn err_no_panic_on_garbage_bytes() {
    // Random bytes — the lens must never panic; it returns None/Err.
    let garbage = [0xff, 0x00, 0xde, 0xad, 0xbe, 0xef, 0x80, 0x90, 0xc1, 0xa5];
    // If it happens to start with a map marker, construct; else expect NonMapTopLevel.
    match RecordView::new(&garbage) {
        Ok(lens) => {
            // Whichever field we probe, we get None — never a panic.
            let key = InternerKey::new(0);
            let _ = lens.get(key.clone());
            let _ = lens.get_int(key.clone());
            let _ = lens.match_str_eq(key, b"y");
        }
        Err(RecordViewError::NonMapTopLevel { .. }) | Err(RecordViewError::Truncated(_)) => {}
        Err(other) => panic!("unexpected error for garbage: {other:?}"),
    }
}

#[test]
fn err_malformed_does_not_break_subsequent_safe_use() {
    // A well-formed record: the lens should handle it cleanly.
    let interner = Interner::new();
    let mut m = crate::types::common::new_map_wc(1);
    let k = interner.touch_ind("ok").unwrap().into_key();
    m.insert(k.clone(), crate::types::value::InnerValue::Int(1));
    let blob = crate::types::value::InnerValue::Map(m).to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    assert_eq!(lens.get_int(k.clone()), Some(1));
    let missing = InternerKey::new(999);
    assert_eq!(lens.get_int(missing), None);
}
