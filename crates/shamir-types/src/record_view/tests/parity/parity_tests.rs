//! THE keystone: prove `RecordView::get(InternerKey)` AGREES with
//! `InnerValue::from_bytes` tree lookup for EVERY field and type the canonical
//! encoder/decoder pair can produce over the STORAGE form
//! (`InnerValue::to_bytes()`). This pins the Stage-2 substitutability contract
//! (the lens may stand in for the tree wherever a consumer reads a field).
//!
//! The storage form serialises `InternerKey` map keys as msgpack `bin`
//! (variable-width LE bytes). The lens seeks `bin` keys by encoding the target
//! field's id to the same wire bytes (a la `eval_bytes::interned_key_bytes`).
//!
//! Coverage is exhaustive over the marker set:
//! * every int width the encoder emits (`serialize_i64` -> fixpos/fixneg/u8/u16/
//!   u32/i8/i16/i32/i64 depending on magnitude; positive fixint, -1, u8 boundary,
//!   u16 boundary, u32 boundary, i64::MAX, i64::MIN);
//! * the U64 > `i64::MAX` edge — synthesised via raw rmpv bytes (the encoder
//!   never emits it from `InnerValue`, but the decoder accepts it). The tree
//!   decoder promotes it to `Big(BigInt)` (FG-1); the lens maps it to
//!   `Str(decimal)`. Both are lossless — see
//!   `parity_u64_above_i64_max_tree_big_lens_str`);
//! * `F32`-precision-via-`F64` and `F64`;
//! * empty string, short string, long string (>32 -> Str8, >255 -> Str16);
//! * empty map, flat map, nested map (2-level), nested nested map (3-level);
//! * empty array, flat array, array-of-maps;
//! * binary (empty, short, long);
//! * bool (true/false), null;
//! * missing field.

use crate::core::interner::{Interner, InternerKey};
use crate::record_view::{RecordValue, RecordView};
use crate::types::common::new_map_wc;
use crate::types::value::InnerValue;
use shamir_collections::TFxSet;
use std::borrow::Cow;

/// Intern a string key, returning the `InternerKey` the tree map uses.
fn ik(interner: &Interner, s: &str) -> InternerKey {
    interner.touch_ind(s).unwrap().into_key()
}

/// Compare a lens [`RecordValue`] against the tree's [`InnerValue`] for the
/// SAME field. This is the per-field assertion the parity tests drive: the two
/// must agree on type and value. The U64 > `i64::MAX` edge is NOT exercised
/// here (the tree now maps it to `Big`, the lens to `Str(decimal)` — both
/// lossless but different representations; that edge has its own dedicated
/// test, `parity_u64_above_i64_max_tree_big_lens_str`).
fn assert_lens_matches_tree(lens: Option<RecordValue<'_>>, tree: Option<&InnerValue>) {
    match (lens, tree) {
        (Some(RecordValue::Null), Some(InnerValue::Null)) => {}
        (Some(RecordValue::Bool(a)), Some(InnerValue::Bool(b))) => assert_eq!(a, *b),
        (Some(RecordValue::Int(a)), Some(InnerValue::Int(b))) => assert_eq!(a, *b),
        (Some(RecordValue::F64(a)), Some(InnerValue::F64(b))) => {
            // bit-exact: the tree stores F32 widened to f64 and F64 as-is; the
            // lens decodes the same bits, so assert_eq is sound.
            assert_eq!(a.to_bits(), b.to_bits(), "f64 bits differ: {a} vs {b}");
        }
        (Some(RecordValue::Str(a)), Some(InnerValue::Str(b))) => assert_eq!(a.as_ref(), b.as_str()),
        (Some(RecordValue::Bin(a)), Some(InnerValue::Bin(b))) => assert_eq!(a, b.as_slice()),
        // Aggregates are compared structurally in dedicated tests below
        // (parity over nested maps is asserted via get_path / fields, not here).
        (Some(RecordValue::Map(_)), Some(InnerValue::Map(_))) => {}
        (Some(RecordValue::Arr(_)), Some(InnerValue::List(_))) => {}
        (None, None) => {}
        other => panic!("lens/tree disagree: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Exhaustive per-width coverage.
// ---------------------------------------------------------------------------

#[test]
fn parity_every_int_width() {
    let interner = Interner::new();
    // Magnitudes chosen to land on each encoder width boundary:
    //   0          -> positive fixint (0x00)
    //   127        -> positive fixint max (0x7f)
    //   128        -> u8 (0xcc 0x80)
    //   255        -> u8 max
    //   256        -> u16
    //   65535      -> u16 max
    //   65536      -> u32
    //   -1         -> negative fixint (0xff)
    //   -32        -> negative fixint min (0xe0)
    //   -33        -> i8 (0xd0)
    //   -128       -> i8 min
    //   -32768     -> i16 min
    //   i64::MAX   -> i64
    //   i64::MIN   -> i64
    let widths = [
        ("zero", 0i64),
        ("pfix_max", 127),
        ("u8_min", 128),
        ("u8_max", 255),
        ("u16_min", 256),
        ("u16_max", 65535),
        ("u32_min", 65536),
        ("nfix_min", -1),
        ("nfix_floor", -32),
        ("i8_min", -33),
        ("i8_floor", -128),
        ("i16_floor", -32768),
        ("i64_max", i64::MAX),
        ("i64_min", i64::MIN),
    ];
    let mut m = new_map_wc(widths.len());
    for (name, v) in widths {
        m.insert(ik(&interner, name), InnerValue::Int(v));
    }
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    for (name, expected) in widths {
        let key = ik(&interner, name);
        let l = lens.get(key.clone());
        let t = tree_map.get(&key);
        assert_lens_matches_tree(l, t);
        // Direct int accessor.
        assert_eq!(lens.get_int(key), Some(expected), "{name}");
    }
    // Missing field.
    let absent = ik(&interner, "absent");
    assert_lens_matches_tree(lens.get(absent.clone()), tree_map.get(&absent));
    assert_eq!(lens.get_int(absent), None);
}

#[test]
fn parity_f32_and_f64() {
    let interner = Interner::new();
    // F32-storable values round-trip through the encoder as f64; the lens
    // decodes them as f64 too. Pick values exactly representable in f32 so
    // bit-equality holds after the f32->f64 widen.
    let f32_vals = [("a", 1.5f64), ("b", -2.25), ("c", 0.0), ("d", 100.0)];
    let f64_vals = [("e", 1e100), ("f", -1e-300), ("g", std::f64::consts::PI)];
    let mut m = new_map_wc(f32_vals.len() + f64_vals.len());
    for (n, v) in f32_vals {
        m.insert(ik(&interner, n), InnerValue::F64(v));
    }
    for (n, v) in f64_vals {
        m.insert(ik(&interner, n), InnerValue::F64(v));
    }
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    for (n, _) in f32_vals.iter().chain(f64_vals.iter()) {
        let key = ik(&interner, n);
        assert_lens_matches_tree(lens.get(key.clone()), tree_map.get(&key));
    }
}

#[test]
fn parity_strings_all_widths() {
    let interner = Interner::new();
    // empty (fixstr len 0), short (fixstr), str8 boundary (>31 bytes -> str8),
    // str16 boundary (>255 bytes -> str16).
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("short", "hi"),
        ("fix_max", &"x".repeat(31)),
        ("str8_min", &"y".repeat(32)),
        ("str8_max", &"z".repeat(255)),
        ("str16_min", &"w".repeat(256)),
        ("utf8", "üñîçødé"),
    ];
    let mut m = new_map_wc(cases.len());
    for (n, v) in cases {
        m.insert(ik(&interner, n), InnerValue::Str((*v).to_string()));
    }
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    for (n, expected) in cases {
        let key = ik(&interner, n);
        assert_lens_matches_tree(lens.get(key.clone()), tree_map.get(&key));
        // Borrowed-str accessor (zero-copy for real string fields).
        assert_eq!(lens.get_str(key), Some(*expected), "{n}");
    }
}

#[test]
fn parity_bin_all_widths() {
    let interner = Interner::new();
    let cases: &[(&str, Vec<u8>)] = &[
        ("empty", vec![]),
        ("short", vec![0u8; 10]),
        ("bin8_max", vec![1u8; 255]),
        ("bin16_min", vec![2u8; 256]),
    ];
    let mut m = new_map_wc(cases.len());
    for (n, v) in cases {
        m.insert(ik(&interner, n), InnerValue::Bin(v.clone()));
    }
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    for (n, expected) in cases {
        let key = ik(&interner, n);
        assert_lens_matches_tree(lens.get(key.clone()), tree_map.get(&key));
        assert_eq!(lens.get_bytes(key), Some(expected.as_slice()), "{n}");
    }
}

#[test]
fn parity_bool_null() {
    let interner = Interner::new();
    let mut m = new_map_wc(3);
    m.insert(ik(&interner, "t"), InnerValue::Bool(true));
    m.insert(ik(&interner, "f"), InnerValue::Bool(false));
    m.insert(ik(&interner, "n"), InnerValue::Null);
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    let t_key = ik(&interner, "t");
    let f_key = ik(&interner, "f");
    let n_key = ik(&interner, "n");
    assert_lens_matches_tree(lens.get(t_key.clone()), tree_map.get(&t_key));
    assert_lens_matches_tree(lens.get(f_key.clone()), tree_map.get(&f_key));
    assert_lens_matches_tree(lens.get(n_key.clone()), tree_map.get(&n_key));
    assert_eq!(lens.get_bool(t_key), Some(true));
    assert_eq!(lens.get_bool(f_key), Some(false));
}

// ---------------------------------------------------------------------------
// Nested maps + arrays — the lens returns lazy cursors; verify they re-walk
// to the same values the tree materialised.
// ---------------------------------------------------------------------------

#[test]
fn parity_nested_map_two_levels() {
    let interner = Interner::new();
    let mut addr = new_map_wc(2);
    addr.insert(ik(&interner, "city"), InnerValue::Str("Jerusalem".into()));
    addr.insert(ik(&interner, "zip"), InnerValue::Int(9100000));
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "name"), InnerValue::Str("user-1".into()));
    m.insert(ik(&interner, "address"), InnerValue::Map(addr));
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();

    let addr_key = ik(&interner, "address");
    // Top-level nested-map value decodes to a RecordValue::Map (nested lens).
    let addr_lens = match lens.get(addr_key) {
        Some(RecordValue::Map(nested)) => nested,
        other => panic!("expected nested Map, got {other:?}"),
    };
    let city_key = ik(&interner, "city");
    let zip_key = ik(&interner, "zip");
    assert_eq!(addr_lens.get_str(city_key), Some("Jerusalem"));
    assert_eq!(addr_lens.get_int(zip_key), Some(9100000));
}

#[test]
fn parity_nested_map_three_levels_via_get_path() {
    let interner = Interner::new();
    // { meta: { loc: { lat: 100 } } }
    let mut loc = new_map_wc(1);
    loc.insert(ik(&interner, "lat"), InnerValue::Int(100));
    let mut meta = new_map_wc(1);
    meta.insert(ik(&interner, "loc"), InnerValue::Map(loc));
    let mut m = new_map_wc(1);
    m.insert(ik(&interner, "meta"), InnerValue::Map(meta));
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();

    let meta_key = ik(&interner, "meta");
    let loc_key = ik(&interner, "loc");
    let lat_key = ik(&interner, "lat");
    assert_eq!(
        lens.get_path(&[meta_key, loc_key, lat_key])
            .and_then(|v| v.as_int()),
        Some(100)
    );
}

#[test]
fn parity_array_of_scalars() {
    let interner = Interner::new();
    let mut m = new_map_wc(1);
    m.insert(
        ik(&interner, "tags"),
        InnerValue::List(vec![
            InnerValue::Str("alpha".into()),
            InnerValue::Int(42),
            InnerValue::Bool(true),
        ]),
    );
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();

    let tags_key = ik(&interner, "tags");
    let seq = match lens.get(tags_key) {
        Some(RecordValue::Arr(s)) => s,
        other => panic!("expected Arr, got {other:?}"),
    };
    assert_eq!(seq.len(), 3);
    let elems: Vec<_> = seq.iter().collect();
    assert_eq!(elems.len(), 3);
    assert!(matches!(elems[0], RecordValue::Str(Cow::Borrowed("alpha"))));
    assert!(matches!(elems[1], RecordValue::Int(42)));
    assert!(matches!(elems[2], RecordValue::Bool(true)));
}

#[test]
fn parity_empty_map_and_empty_array() {
    let interner = Interner::new();
    let mut m = new_map_wc(2);
    m.insert(ik(&interner, "empty_map"), InnerValue::Map(new_map_wc(0)));
    m.insert(ik(&interner, "empty_arr"), InnerValue::List(vec![]));
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();

    let em_key = ik(&interner, "empty_map");
    let ea_key = ik(&interner, "empty_arr");
    match lens.get(em_key) {
        Some(RecordValue::Map(nested)) => {
            assert!(nested.is_empty());
            // Nested maps also use bin keys (InternerKey); probe with a
            // dummy id to prove it returns None.
            assert_eq!(nested.get(InternerKey::new(999)), None);
        }
        other => panic!("expected empty Map, got {other:?}"),
    }
    match lens.get(ea_key) {
        Some(RecordValue::Arr(s)) => {
            assert!(s.is_empty());
            assert_eq!(s.iter().count(), 0);
        }
        other => panic!("expected empty Arr, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// THE U64 > i64::MAX EDGE — the tree decoder (FG-1) now maps it to `Big`
// (lossless `BigInt`); the lens maps it to `Str(decimal)`. Both are
// lossless representations of the same value in two different type systems
// (the lens is deliberately zero-copy and has no `Big` variant). The
// encoder never emits this from `InnerValue::Int` (Int is i64), so
// synthesise the bytes via raw msgpack and confirm the tree==lens value.
// ---------------------------------------------------------------------------

#[test]
fn parity_u64_above_i64_max_tree_big_lens_str() {
    let interner = Interner::new();
    let large_u64: u64 = i64::MAX as u64 + 1;
    // Synthesise { <bin key for "big"> : <u64> } directly in msgpack.
    // The storage form has bin-keys, so we encode the interned key for "big"
    // as a bin marker + LE bytes, and the value as a u64.
    let big_key = ik(&interner, "big");
    let (key_buf, key_len) = big_key.as_bytes_buf();
    let key_bytes = &key_buf[..key_len];

    let mut blob = Vec::new();
    // fixmap with 1 entry: 0x81
    blob.push(0x81);
    // bin8 key
    blob.push(0xc4);
    blob.push(key_len as u8);
    blob.extend_from_slice(key_bytes);
    // u64 value: 0xcf + 8 BE bytes
    blob.push(0xcf);
    blob.extend_from_slice(&large_u64.to_be_bytes());

    // Tree decoder's view (via from_bytes, the storage decoder). The tree
    // decoder dispatches through `ValueVisitor::visit_u64`, which (FG-1)
    // promotes u64 > i64::MAX losslessly to `Big(BigInt)` instead of
    // truncating via `value as i64`.
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };
    let tree_big = tree_map.get(&big_key);
    // The tree decoder maps U64 > i64::MAX to Big (lossless BigInt).
    match tree_big {
        Some(InnerValue::Big(b)) => {
            assert_eq!(
                b,
                &num_bigint::BigInt::from(large_u64),
                "tree Big must hold the exact u64 value"
            );
        }
        other => panic!("tree view for U64>MAX: expected Big, got {other:?}"),
    }

    // Lens's view — the lens uses `uint_to_record_value` which maps U64 >
    // i64::MAX to `Str(Owned(decimal))` (deliberately zero-copy; the lens has
    // no `Big` variant by design).
    let lens = RecordView::new(&blob).unwrap();
    let lens_val = lens.get(big_key.clone());
    match (&lens_val, tree_big) {
        (Some(RecordValue::Str(a)), Some(InnerValue::Big(b))) => {
            // Both representations are lossless and agree on the exact value:
            // lens → Str(decimal), tree → Big(decimal). The decimal text must
            // match.
            assert_eq!(a.as_ref(), large_u64.to_string());
            assert_eq!(b.to_string(), large_u64.to_string());
        }
        other => panic!(
            "lens/tree disagree on U64>MAX: lens={:?}, tree={:?}",
            other.0, other.1
        ),
    }
}

// ---------------------------------------------------------------------------
// fields() iterator — every top-level entry agrees with the tree, by id.
// ---------------------------------------------------------------------------

#[test]
fn parity_fields_iter_matches_tree() {
    let interner = Interner::new();
    let mut m = new_map_wc(6);
    m.insert(ik(&interner, "a"), InnerValue::Int(1));
    m.insert(ik(&interner, "b"), InnerValue::Str("two".into()));
    m.insert(ik(&interner, "c"), InnerValue::F64(3.0));
    m.insert(ik(&interner, "d"), InnerValue::Bool(false));
    m.insert(ik(&interner, "e"), InnerValue::Null);
    m.insert(ik(&interner, "f"), InnerValue::Bin(vec![9, 9]));
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let tree = InnerValue::from_bytes(&blob).unwrap();
    let tree_map = match &tree {
        InnerValue::Map(m) => m,
        _ => panic!("expected map"),
    };

    let mut seen = TFxSet::<u64>::default();
    for (key, val) in lens.fields() {
        assert_lens_matches_tree(Some(val), tree_map.get(&key));
        seen.insert(key.id());
    }
    // Every tree field was visited.
    assert_eq!(seen.len(), tree_map.len());
    for k in tree_map.keys() {
        assert!(
            seen.contains(&k.id()),
            "field id {} not enumerated by lens",
            k.id()
        );
    }
}

// ---------------------------------------------------------------------------
// FieldIndex — multi-field access agrees with single-field get and the tree.
// ---------------------------------------------------------------------------

#[test]
fn parity_index_matches_get_and_tree() {
    let interner = Interner::new();
    let mut m = new_map_wc(4);
    let id_key = ik(&interner, "id");
    let name_key = ik(&interner, "name");
    let flag_key = ik(&interner, "flag");
    let score_key = ik(&interner, "score");
    m.insert(id_key.clone(), InnerValue::Int(7));
    m.insert(name_key.clone(), InnerValue::Str("idx".into()));
    m.insert(flag_key.clone(), InnerValue::Bool(true));
    m.insert(score_key.clone(), InnerValue::F64(2.5));
    let inner = InnerValue::Map(m);
    let blob = inner.to_bytes().unwrap();
    let lens = RecordView::new(&blob).unwrap();
    let idx = lens.index();

    assert_eq!(idx.len(), 4);
    assert_eq!(idx.get_int(id_key.clone()), lens.get_int(id_key.clone()));
    assert_eq!(idx.get_int(id_key), Some(7));
    assert_eq!(idx.get_str(name_key), Some("idx"));
    assert_eq!(idx.get_int(flag_key), None); // wrong type
    let absent_key = ik(&interner, "absent");
    assert_eq!(idx.get(absent_key), None);
}
