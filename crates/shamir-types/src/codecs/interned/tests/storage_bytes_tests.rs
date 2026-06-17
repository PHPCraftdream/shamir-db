//! Byte-identity proof for the W2d direct `QueryValue -> id-keyed storage`
//! encoder ([`query_value_to_storage_bytes`]).
//!
//! Contract under test (MUST hold byte-for-byte, or storage is corrupt and
//! recovery diverges):
//!
//! ```text
//! query_value_to_storage_bytes(qv, f)
//!   == query_value_to_inner_with(qv, f).unwrap().to_bytes().unwrap()
//! ```
//!
//! for a battery that exercises every msgpack width boundary for both scalar
//! values and interned map-key ids, plus map key ORDER (insertion order, no
//! re-sort).

use crate::codecs::interned::messagepack::query_value_to_storage_bytes;
use crate::codecs::interned::query_value_to_inner_with;
use crate::core::interner::InternerKey;
use crate::types::common::{new_map, new_set};
use crate::types::value::{QueryValue, Value};
use bytes::Bytes;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_collections::TFxMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex, OnceLock};

/// Deterministic first-seen-order interner.
///
/// Returns a closure that assigns ids strictly in the order keys are FIRST
/// seen. Each call to this function returns an INDEPENDENT closure (fresh id
/// space starting at 0); a single closure instance is `Fn` and can be shared
/// across multiple encode calls for the same record — matching the production
/// invariant (one interner per record, shared by every consumer of that
/// record's keys).
fn make_first_seen_interner(
) -> impl Fn(&str) -> Result<InternerKey, crate::codecs::CodecError> + 'static {
    struct State {
        next_id: u64,
        seen: TFxMap<String, u64>,
    }
    let state: Arc<Mutex<State>> = Arc::new(Mutex::new(State {
        next_id: 0,
        seen: TFxMap::default(),
    }));
    move |key: &str| {
        let mut s = state.lock().expect("interner mutex poisoned");
        let id = if let Some(&id) = s.seen.get(key) {
            id
        } else {
            let id = s.next_id;
            s.next_id += 1;
            s.seen.insert(key.to_string(), id);
            id
        };
        Ok(InternerKey::new(id))
    }
}

/// Assert byte-identity using a SHARED fresh first-seen interner so both paths
/// assign identical ids to the same keys in the same order. Both encoders take
/// `&F`, so a single closure instance is reused across both calls — exactly
/// the production shape (one interner per record).
fn assert_byte_identical(label: &str, qv: &QueryValue) {
    let f = make_first_seen_interner();
    let direct = query_value_to_storage_bytes(qv, &f)
        .unwrap_or_else(|e| panic!("[{label}] direct encode failed: {e:?}"));
    let reference = query_value_to_inner_with(qv, &f)
        .unwrap_or_else(|e| panic!("[{label}] inner conversion failed: {e:?}"))
        .to_bytes()
        .unwrap_or_else(|e| panic!("[{label}] reference encode failed: {e:?}"));
    assert_eq!(
        direct.as_ref(),
        reference.as_ref(),
        "[{label}] byte mismatch\n  direct    = {direct_hex}\n  reference = {ref_hex}",
        direct_hex = hex_dump(&direct),
        ref_hex = hex_dump(&reference),
    );
}

fn hex_dump(b: &Bytes) -> String {
    b.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// --------------------------------------------------------------------------
// Int width boundaries
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_int_width_boundaries() {
    for &i in &[
        0i64,
        127,   // last positive fixint
        128,   // first u8
        255,   // last u8
        256,   // first u16
        65535, // last u16
        65536, // first u32
        i64::MAX,
        -1i64,
        -32, // last negative fixint
        -33, // first i8
        i64::MIN,
    ] {
        let qv = Value::Int(i);
        assert_byte_identical(&format!("int({i})"), &qv);
    }
}

// --------------------------------------------------------------------------
// F64 specials (incl. non-finite — rmp_serde writes them, serde_json would not)
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_f64_specials() {
    for &f in &[1.5f64, -0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let qv = Value::F64(f);
        assert_byte_identical(&format!("f64({f:?})"), &qv);
    }
}

// --------------------------------------------------------------------------
// Str / Bin width boundaries
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_str_boundaries() {
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("fixstr_last_31", &"a".repeat(31)),
        ("str8_first_32", &"b".repeat(32)),
        ("str16_first_256", &"c".repeat(256)),
    ];
    for (label, s) in cases {
        let qv = Value::Str((*s).to_string());
        assert_byte_identical(label, &qv);
    }
}

#[test]
fn storage_bytes_bin_boundaries() {
    let cases: &[(&str, Vec<u8>)] = &[
        ("bin_empty", vec![]),
        ("bin8_last_255", vec![0xABu8; 255]),
        ("bin16_first_256", vec![0xCDu8; 256]),
    ];
    for (label, b) in cases {
        let qv = Value::Bin(b.clone());
        assert_byte_identical(label, &qv);
    }
}

// --------------------------------------------------------------------------
// Bool / Null
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_bool_null() {
    assert_byte_identical("bool_true", &Value::Bool(true));
    assert_byte_identical("bool_false", &Value::Bool(false));
    assert_byte_identical("null", &Value::Null);
}

// --------------------------------------------------------------------------
// Dec / Big (serialize_str contract)
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_dec_big() {
    let dec = Decimal::from_str("3.14159").unwrap();
    assert_byte_identical("dec", &Value::Dec(dec));

    let big = BigInt::from_str("123456789012345678901234567890").unwrap();
    assert_byte_identical("big", &Value::Big(big));

    // Big that does not fit in i64 either — must still serialize as str.
    let big_neg = BigInt::from_str("-99999999999999999999999999").unwrap();
    assert_byte_identical("big_neg", &Value::Big(big_neg));
}

// --------------------------------------------------------------------------
// Nested structures
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_nested_map() {
    // 3-level nested map: outer -> inner -> innermost.
    let mut innermost = new_map();
    innermost.insert("k3".to_string(), Value::Int(3));
    innermost.insert("k1".to_string(), Value::Str("v1".into()));
    // deliberately non-sorted insertion to test ORDER preservation at depth
    innermost.insert("k2".to_string(), Value::Bool(false));

    let mut inner = new_map();
    inner.insert("nested".to_string(), Value::Map(innermost));
    inner.insert("x".to_string(), Value::F64(2.5));

    let mut outer = new_map();
    outer.insert("a".to_string(), Value::Map(inner));
    outer.insert("top".to_string(), Value::Int(42));

    let qv = Value::Map(outer);
    assert_byte_identical("nested_map_3lvl", &qv);
}

#[test]
fn storage_bytes_list_mixed() {
    let list = vec![
        Value::Int(1),
        Value::Str("two".into()),
        Value::Bool(true),
        Value::Null,
        Value::F64(4.0),
        Value::Bin(vec![0u8, 1, 2]),
    ];
    let qv = Value::List(list);
    assert_byte_identical("list_mixed", &qv);
}

#[test]
fn storage_bytes_set() {
    let mut s = new_set();
    s.insert(Value::Int(10));
    s.insert(Value::Str("a".into()));
    s.insert(Value::Bool(false));
    let qv = Value::Set(s);
    assert_byte_identical("set", &qv);
}

// --------------------------------------------------------------------------
// Map KEY id width boundaries
//
// A custom intern_fn returns the DESIRED id for a given name, so we can drive
// the InternerKey wire width across the 1/2/4/8-byte boundaries exactly.
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_map_key_id_width_boundaries() {
    // ids chosen to land exactly on bin-byte-width boundaries:
    //   255  -> 1-byte LE  (bin8 hdr + len=1)
    //   256  -> 2-byte LE  (bin8 hdr + len=2)
    //   65536-> 4-byte LE  (bin8 hdr + len=4)
    //   2^33 -> 8-byte LE  (bin8 hdr + len=8)
    let ids: [(String, u64); 4] = [
        ("k_255".to_string(), 255),
        ("k_256".to_string(), 256),
        ("k_65536".to_string(), 65536),
        ("k_big".to_string(), 1u64 << 33),
    ];

    // Build a lookup table the intern closure reads from. Each path (direct +
    // reference) gets its OWN closure instance but they read from the SAME
    // static table so id assignment is identical.
    static TABLE: OnceLock<TFxMap<String, u64>> = OnceLock::new();
    let _ = TABLE.set({
        let mut m = TFxMap::default();
        for (k, v) in &ids {
            m.insert(k.clone(), *v);
        }
        m
    });

    let make_fn = || {
        |key: &str| -> Result<InternerKey, crate::codecs::CodecError> {
            let tbl = TABLE.get().expect("table init");
            tbl.get(key)
                .copied()
                .map(InternerKey::new)
                .ok_or_else(|| crate::codecs::CodecError::Decode(format!("no id for key '{key}'")))
        }
    };

    let mut m = new_map();
    for (k, _id) in &ids {
        m.insert(k.clone(), Value::Int(7));
    }
    let qv = Value::Map(m);

    let direct = query_value_to_storage_bytes(&qv, &make_fn()).expect("direct encode");
    let reference = query_value_to_inner_with(&qv, &make_fn())
        .expect("inner convert")
        .to_bytes()
        .expect("ref encode");
    assert_eq!(
        direct.as_ref(),
        reference.as_ref(),
        "map key id width boundary mismatch\n  direct    = {}\n  reference = {}",
        hex_dump(&direct),
        hex_dump(&reference),
    );

    // Sanity: confirm each id width actually appears in the bytes. The bin8
    // marker is 0xc4 followed by a 1-byte length, then the LE id bytes.
    let bytes = direct.as_ref();
    for (_name, id) in &ids {
        let (want_len, le_bytes) = if *id <= u8::MAX as u64 {
            (1u8, (*id as u8).to_le_bytes().to_vec())
        } else if *id <= u16::MAX as u64 {
            (2u8, (*id as u16).to_le_bytes().to_vec())
        } else if *id <= u32::MAX as u64 {
            (4u8, (*id as u32).to_le_bytes().to_vec())
        } else {
            (8u8, id.to_le_bytes().to_vec())
        };
        // find bin8 marker + length byte + the LE id bytes
        let mut found = false;
        for window_start in 0..bytes.len().saturating_sub(2 + le_bytes.len()) {
            if bytes[window_start] == 0xc4
                && bytes[window_start + 1] == want_len
                && bytes[window_start + 2..window_start + 2 + le_bytes.len()] == le_bytes[..]
            {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "expected bin8 id={id} (len={want_len}, bytes={le_bytes:?}) not found in encoded bytes {}",
            hex_dump(&direct)
        );
    }
}

// --------------------------------------------------------------------------
// Map KEY ORDER preservation (insertion order, NOT sorted)
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_map_key_order_unsorted() {
    // Insert keys in deliberately NON-sorted order; the encoder must preserve
    // it (TMap/IndexMap iteration order), matching the reference path.
    let mut m = new_map();
    m.insert("zebra".to_string(), Value::Int(1));
    m.insert("apple".to_string(), Value::Int(2));
    m.insert("mango".to_string(), Value::Int(3));
    m.insert("banana".to_string(), Value::Int(4));
    m.insert("cherry".to_string(), Value::Int(5));
    let qv = Value::Map(m);
    assert_byte_identical("map_key_order_unsorted", &qv);

    // And a second permutation to be sure — order really is preserved.
    let mut m2 = new_map();
    m2.insert("c".to_string(), Value::Int(1));
    m2.insert("a".to_string(), Value::Int(2));
    m2.insert("b".to_string(), Value::Int(3));
    let qv2 = Value::Map(m2);
    assert_byte_identical("map_key_order_cab", &qv2);

    // The two permutations must produce DIFFERENT bytes (otherwise order is
    // being lost in both paths identically — which would still be a bug).
    // Independent interners (one per record) so each starts at id 0.
    let f_a = make_first_seen_interner();
    let bytes_a = query_value_to_storage_bytes(&qv, &f_a).unwrap();
    let f_b = make_first_seen_interner();
    let bytes_b = query_value_to_storage_bytes(&qv2, &f_b).unwrap();
    assert_ne!(
        bytes_a.as_ref(),
        bytes_b.as_ref(),
        "different key orders produced identical bytes — order not actually encoded"
    );
}

// --------------------------------------------------------------------------
// Single-closure-across-both-paths smoke (the production invariant: ONE
// interner shared by every consumer of a record's keys, not two).
// --------------------------------------------------------------------------

#[test]
fn storage_bytes_shared_interner_smoke() {
    let f = make_first_seen_interner();
    let mut m = new_map();
    m.insert("first".to_string(), Value::Int(1));
    m.insert("second".to_string(), Value::Int(2));
    let qv = Value::Map(m);

    // Same closure instance f used for BOTH encoders — exactly the production
    // shape. Both walks see the same id assignments.
    let direct = query_value_to_storage_bytes(&qv, &f).unwrap();
    let reference = query_value_to_inner_with(&qv, &f)
        .unwrap()
        .to_bytes()
        .unwrap();
    assert_eq!(direct.as_ref(), reference.as_ref());
}
