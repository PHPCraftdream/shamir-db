use crate::core::sort_codec::{
    encode_bool, encode_bytes, encode_f64, encode_i64, encode_null, encode_str, encode_u64,
};

fn enc_i64(v: i64) -> Vec<u8> {
    let mut b = Vec::new();
    encode_i64(&mut b, v);
    b
}
fn enc_u64(v: u64) -> Vec<u8> {
    let mut b = Vec::new();
    encode_u64(&mut b, v);
    b
}
fn enc_f64(v: f64) -> Vec<u8> {
    let mut b = Vec::new();
    encode_f64(&mut b, v).unwrap();
    b
}
fn enc_str(s: &str) -> Vec<u8> {
    let mut b = Vec::new();
    encode_str(&mut b, s);
    b
}
fn enc_bool(v: bool) -> Vec<u8> {
    let mut b = Vec::new();
    encode_bool(&mut b, v);
    b
}

#[test]
fn i64_sorts_correctly() {
    // Pick spread including negatives, zero, positives.
    let vals = [i64::MIN, -100, -1, 0, 1, 100, i64::MAX];
    let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_i64(v))).collect();
    encoded.sort_by(|a, b| a.1.cmp(&b.1));
    let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
    assert_eq!(sorted_vals, vals);
}

#[test]
fn u64_sorts_correctly() {
    let vals = [0, 1, 100, u64::MAX / 2, u64::MAX];
    let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_u64(v))).collect();
    encoded.sort_by(|a, b| a.1.cmp(&b.1));
    let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
    assert_eq!(sorted_vals, vals);
}

#[test]
fn f64_sorts_correctly() {
    let vals = [
        f64::NEG_INFINITY,
        -1e100,
        -1.0,
        -0.0,
        0.0,
        1e-100,
        1.0,
        1e100,
        f64::INFINITY,
    ];
    let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_f64(v))).collect();
    encoded.sort_by(|a, b| a.1.cmp(&b.1));
    let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
    for (i, (a, b)) in sorted_vals.iter().zip(vals.iter()).enumerate() {
        // -0.0 == 0.0 in float compare; our codec puts -0.0 BEFORE
        // 0.0 (sign-bit flip). Allow either order for that pair.
        if i < sorted_vals.len() - 1 {
            let next = sorted_vals[i + 1];
            assert!(
                a <= &next,
                "out of order at {i}: {a} > {next} (expected {b})"
            );
        }
    }
}

#[test]
fn f64_nan_refuses() {
    let mut buf = Vec::new();
    assert!(encode_f64(&mut buf, f64::NAN).is_err());
}

#[test]
fn str_sorts_lexicographically() {
    let vals = ["", "a", "aa", "ab", "b", "ba", "你好", "🦀"];
    let mut encoded: Vec<_> = vals.iter().map(|&v| (v, enc_str(v))).collect();
    encoded.sort_by(|a, b| a.1.cmp(&b.1));
    let sorted_vals: Vec<_> = encoded.into_iter().map(|(v, _)| v).collect();
    let mut expected = vals.to_vec();
    expected.sort();
    assert_eq!(sorted_vals, expected);
}

/// Regression: the sorted-index physical key is
/// `prefix || encode_str(s) || record_id_suffix`. If `encode_str`
/// is not self-delimiting, two values where one is a prefix of
/// the other can have their order inverted depending on the
/// suffix bytes — i.e. `"a" || [0xFF; 16]` sorts AFTER
/// `"aa" || [0x00; 16]` even though `"a" < "aa"` semantically.
///
/// Range bounds break the same way: an inclusive upper of
/// `enc("a") || [0xFF; 16]` would match `enc("aa") || ...` keys.
#[test]
fn str_with_suffix_preserves_order() {
    // Encoded values for two strings where one is a prefix of
    // the other. Append a worst-case suffix:
    //   "a"  + 16 bytes of 0xFF (maximum possible suffix)
    //   "aa" + 16 bytes of 0x00 (minimum possible suffix)
    // Lexicographically "a" < "aa", so the FIRST composite key
    // must sort BEFORE the second regardless of the suffix.
    let mut a_key = enc_str("a");
    a_key.extend_from_slice(&[0xFFu8; 16]);
    let mut aa_key = enc_str("aa");
    aa_key.extend_from_slice(&[0x00u8; 16]);
    assert!(
        a_key < aa_key,
        "enc(\"a\")||0xFF*16 must sort before enc(\"aa\")||0x00*16, \
         else sorted-index range queries on strings return wrong results"
    );

    // Same shape but with an extra distractor in the middle.
    let mut a_pad = enc_str("a");
    a_pad.extend_from_slice(b"zzzzzzzzzzzzzzzz"); // 16 'z' bytes
    let mut ab_key = enc_str("ab");
    ab_key.extend_from_slice(b"aaaaaaaaaaaaaaaa");
    assert!(
        a_pad < ab_key,
        "enc(\"a\")||'z'*16 must sort before enc(\"ab\")||'a'*16"
    );

    // Bytes encoding has the same flaw — cover it too.
    let mut buf_a = Vec::new();
    encode_bytes(&mut buf_a, b"\x01");
    buf_a.extend_from_slice(&[0xFFu8; 16]);
    let mut buf_aa = Vec::new();
    encode_bytes(&mut buf_aa, b"\x01\x01");
    buf_aa.extend_from_slice(&[0x00u8; 16]);
    assert!(
        buf_a < buf_aa,
        "encode_bytes must be self-delimiting for sorted-index keys"
    );
}

#[test]
fn bool_sorts_correctly() {
    assert!(enc_bool(false) < enc_bool(true));
}

#[test]
fn tags_order_types() {
    // Across types: Null < Bool < Int < Float < String.
    let null_buf = {
        let mut b = Vec::new();
        encode_null(&mut b);
        b
    };
    assert!(null_buf < enc_bool(false));
    assert!(enc_bool(true) < enc_i64(i64::MIN));
    assert!(enc_i64(i64::MAX) < enc_f64(f64::NEG_INFINITY));
    assert!(enc_f64(f64::INFINITY) < enc_str(""));
}

#[test]
fn composite_via_concatenation() {
    // (Int, String) — sort by (a, b).
    let mut k1 = Vec::new();
    encode_i64(&mut k1, 5);
    encode_str(&mut k1, "zzz");
    let mut k2 = Vec::new();
    encode_i64(&mut k2, 7);
    encode_str(&mut k2, "aaa");
    // 5 < 7 regardless of second column
    assert!(k1 < k2);

    let mut k3 = Vec::new();
    encode_i64(&mut k3, 5);
    encode_str(&mut k3, "aaa");
    // (5, "aaa") < (5, "zzz")
    assert!(k3 < k1);
}
