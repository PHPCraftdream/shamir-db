//! Bisection regression tests for #983 (binary field value corruption).
//!
//! These tests pin the RUST-SIDE layers of the write→storage→read pipeline so
//! the bisection table can cite a concrete PASS at each layer. The defect itself
//! is on the TS client (see report); these guard the Rust floor.
//!
//! * Layer 1 — pure serde round-trip of a `QueryValue::Bin` (both via
//!   `rmp_serde::to_vec_named` and via hand-written JS-client `bin8` bytes).
//!   Isolates whether `deserialize_any` over a real `bin8` marker reaches
//!   `visit_bytes` in this repo's rmp-serde version.
//! * Layer 2 — storage encode/decode: `InnerValue::Bin` → interned on-disk
//!   encoder → `RecordView` lens → back to `QueryValue`. Exercises BOTH read
//!   paths (the `InnerValue` tree path AND the `RecordView` lens path) over the
//!   STORAGE form (`InnerValue::to_bytes` — bin-keyed) and asserts they preserve
//!   `Bin` empirically.

use crate::codecs::interned::{
    inner_to_msgpack, inner_value_to_query_value, msgpack_to_inner, record_view_to_query_value,
};
use crate::core::interner::Interner;
use crate::record_view::RecordView;
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, QueryValue, Value};

// ── Layer 1: pure serde round-trip of QueryValue::Bin ───────────────────────

#[test]
fn layer1_serde_roundtrip_queryvalue_bin() {
    // A QueryValue::Map containing a Value::Bin — the exact shape a record
    // like `{ id: "b1", blob: <bytes> }` takes on the wire.
    let mut m = new_map_wc(2);
    m.insert("id".to_string(), QueryValue::Str("b1".to_string()));
    m.insert(
        "blob".to_string(),
        QueryValue::Bin(vec![0, 1, 255, 254, 127, 128]),
    );
    let qv = QueryValue::Map(m);

    let encoded = rmp_serde::to_vec_named(&qv).expect("to_vec_named");
    let decoded: QueryValue = rmp_serde::from_slice(&encoded).expect("from_slice");

    assert_eq!(qv, decoded, "QueryValue serde round-trip lost information");

    // Pin the Bin leaf specifically (the symptom is that Bin-ness is lost).
    match &decoded {
        Value::Map(m) => match m.get("blob") {
            Some(Value::Bin(b)) => assert_eq!(b, &[0, 1, 255, 254, 127, 128]),
            other => panic!("blob is not Bin after round-trip: {other:?}"),
        },
        other => panic!("decoded value is not a Map: {other:?}"),
    }
}

#[test]
fn layer1_handwritten_js_bin8_decodes_to_queryvalue_bin() {
    // Hand-write the EXACT msgpack bytes a JS client sends for
    // `{ id: "b1", blob: Uint8Array([0,1,255,254,127,128]) }`:
    //   0x82                      fixmap, 2 entries
    //   0xa2 0x69 0x64            fixstr(2) "id"
    //   0xa2 0x62 0x31            fixstr(2) "b1"
    //   0xa4 0x62 0x6c 0x6f 0x62  fixstr(4) "blob"
    //   0xc4 0x06 0x00..0x80      bin8, len 6, payload
    let bytes: &[u8] = &[
        0x82, // fixmap(2)
        0xa2, b'i', b'd', // "id"
        0xa2, b'b', b'1', // "b1"
        0xa4, b'b', b'l', b'o', b'b', // "blob" (fixstr len 4)
        0xc4, 0x06, 0x00, 0x01, 0xff, 0xfe, 0x7f, 0x80, // bin8(6)
    ];

    let decoded: QueryValue = rmp_serde::from_slice(bytes).expect("from_slice");

    // The crux of Layer 1: does `deserialize_any` over a real bin8 marker reach
    // `visit_bytes` (→ Value::Bin) in this repo's rmp-serde version?
    match &decoded {
        Value::Map(m) => match m.get("blob") {
            Some(Value::Bin(b)) => assert_eq!(b, &[0, 1, 255, 254, 127, 128]),
            other => panic!(
                "hand-written bin8 did NOT decode to Bin (deserialize_any missed visit_bytes): {other:?}"
            ),
        },
        other => panic!("decoded value is not a Map: {other:?}"),
    }
}

// ── Layer 2: storage encode/decode preserves Bin on BOTH read paths ─────────

/// Build an InnerValue::Map whose `blob` field is `InnerValue::Bin`, the shape
/// that lands on disk after the write path interns the record's keys.
fn bin_record(interner: &Interner) -> InnerValue {
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let blob_key = interner.touch_ind("blob").unwrap().into_key();
    let mut m = new_map_wc(2);
    m.insert(id_key, InnerValue::Str("b1".to_string()));
    m.insert(blob_key, InnerValue::Bin(vec![0, 1, 255, 254, 127, 128]));
    InnerValue::Map(m)
}

#[test]
fn layer2_tree_path_storage_roundtrip_preserves_bin() {
    let interner = Interner::new();
    let record = bin_record(&interner);

    // STORAGE round-trip via the on-disk form (`to_bytes` → bin-keyed msgpack).
    let bytes = record.to_bytes().expect("to_bytes");
    let decoded = InnerValue::from_bytes(&bytes).expect("from_bytes");

    let blob_key = interner.touch_ind("blob").unwrap().into_key();
    match &decoded {
        InnerValue::Map(m) => match m.get(&blob_key) {
            Some(InnerValue::Bin(b)) => assert_eq!(b, &[0, 1, 255, 254, 127, 128]),
            other => panic!("tree-path (storage): blob is not Bin after round-trip: {other:?}"),
        },
        other => panic!("tree-path (storage): decoded value is not a Map: {other:?}"),
    }
}

#[test]
fn layer2_wire_path_inner_to_msgpack_to_inner_preserves_bin() {
    let interner = Interner::new();
    let record = bin_record(&interner);

    // WIRE/response round-trip: `inner_to_msgpack` de-interns keys to strings;
    // `msgpack_to_inner` re-interns them. Both sides agree on string keys.
    let bytes = inner_to_msgpack(&interner, &record).expect("inner_to_msgpack");
    let decoded = msgpack_to_inner(&interner, &bytes).expect("msgpack_to_inner");

    let blob_key = interner.touch_ind("blob").unwrap().into_key();
    match &decoded {
        InnerValue::Map(m) => match m.get(&blob_key) {
            Some(InnerValue::Bin(b)) => assert_eq!(b, &[0, 1, 255, 254, 127, 128]),
            other => panic!("wire-path: blob is not Bin after round-trip: {other:?}"),
        },
        other => panic!("wire-path: decoded value is not a Map: {other:?}"),
    }
}

#[test]
fn layer2_lens_path_recordview_deintern_preserves_bin() {
    let interner = Interner::new();
    let record = bin_record(&interner);

    // The STORAGE bytes (bin-keyed) the storage layer hands to a RecordView lens.
    let bytes = record.to_bytes().expect("to_bytes");

    // ── lens path: RecordView over the storage bytes → QueryValue ──
    let view = RecordView::new(&bytes).expect("RecordView::new");
    let qv_lens = record_view_to_query_value(&view, &interner).expect("lens de-intern");
    match &qv_lens {
        QueryValue::Map(m) => match m.get("blob") {
            Some(Value::Bin(b)) => assert_eq!(b, &[0, 1, 255, 254, 127, 128]),
            other => panic!("lens-path: blob is not Bin: {other:?}"),
        },
        other => panic!("lens-path: decoded value is not a Map: {other:?}"),
    }

    // ── tree path (control): InnerValue tree → QueryValue ──
    // The codec doc comment claims lens-path == tree-path arm-for-arm; verify
    // that claim empirically for `Bin` rather than trusting it.
    let decoded = InnerValue::from_bytes(&bytes).expect("from_bytes");
    let qv_tree = inner_value_to_query_value(&decoded, &interner).expect("tree de-intern");
    assert_eq!(
        qv_lens, qv_tree,
        "lens-path and tree-path disagree on a Bin value"
    );
}
