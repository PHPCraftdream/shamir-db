//! Category 6 — `Debug` smoke test.
//!
//! `Debug` must not panic and must produce output that reflects the byte
//! slice, not the representation (no `Inline`/`Heap` enum leakage).

use super::super::KeyBytes;

#[test]
fn debug_does_not_panic_for_inline_and_heap() {
    let inline = KeyBytes::from_slice(&[1, 2, 3]);
    let heap = KeyBytes::from_slice(&[0u8; 100]);
    let _ = format!("{inline:?}");
    let _ = format!("{heap:?}");
}

#[test]
fn debug_matches_byte_slice_debug_not_repr_enum() {
    // The exact representation (inline vs heap) must NOT appear in Debug
    // output — both should look like the underlying byte slice's Debug.
    let bytes = [0xDEu8, 0xAD, 0xBE, 0xEF];
    let kb = KeyBytes::from_slice(&bytes);
    let kb_dbg = format!("{kb:?}");
    let slice_dbg = format!("{:?}", &bytes[..]);
    assert_eq!(kb_dbg, slice_dbg, "Debug must equal byte-slice Debug");
    // And it must not leak the enum's variant names.
    assert!(!kb_dbg.contains("Inline"), "Debug leaked Inline variant");
    assert!(!kb_dbg.contains("Heap"), "Debug leaked Heap variant");
}
