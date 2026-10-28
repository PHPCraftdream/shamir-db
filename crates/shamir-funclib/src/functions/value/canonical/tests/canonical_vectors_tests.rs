use crate::canonical::{canonical_bytes, canonical_hash, PREV_HASH_FIELD, PREV_HASH_KEY_BYTES};
use shamir_types::types::common::{new_map, new_set, TMap};
use shamir_types::types::value::{QueryValue, Value};
use std::hash::{Hash, Hasher};

// Hand-derived from the frozen v1 type tags, lengths, payloads, and MessagePack key headers.
const TAGS: &[u8] = &[0, 1, 2, 3, 6, 7, 8, 9, 10];
fn map(pairs: &[(&str, QueryValue)]) -> QueryValue {
    let mut m = new_map();
    for (key, value) in pairs {
        m.insert((*key).to_owned(), value.clone());
    }
    Value::Map(m)
}
fn assert_vector(value: &QueryValue, expected: &[u8]) {
    assert_eq!(canonical_bytes(value).unwrap(), expected);
    assert_eq!(
        canonical_hash(value).unwrap(),
        blake3::hash(expected).to_hex().to_string()
    );
}

#[test]
fn rv_funclib_54359750_v1_canonical_byte_and_hash_vectors() {
    let mut set = new_set();
    set.insert(QueryValue::Int(2));
    set.insert(QueryValue::Int(1));
    let vectors: Vec<(QueryValue, Vec<u8>)> = vec![
        (QueryValue::Null, vec![0x00]),
        (QueryValue::Bool(true), vec![0x01, 0x01]),
        (
            QueryValue::Int(-1),
            vec![0x02, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ),
        (
            QueryValue::F64(1.5),
            vec![0x03, 0x3f, 0xf8, 0, 0, 0, 0, 0, 0],
        ),
        (
            QueryValue::Str("x".into()),
            vec![0x06, 0, 0, 0, 0, 0, 0, 0, 1, b'x'],
        ),
        (
            QueryValue::Bin(vec![0xab]),
            vec![0x07, 0, 0, 0, 0, 0, 0, 0, 1, 0xab],
        ),
        (
            QueryValue::List(vec![QueryValue::Null]),
            vec![0x08, 0, 0, 0, 0, 0, 0, 0, 1, 0],
        ),
        (
            QueryValue::Set(set),
            vec![
                0x09, 0, 0, 0, 0, 0, 0, 0, 2, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0,
                0, 2,
            ],
        ),
        (
            QueryValue::Dec("1.50".parse().unwrap()),
            vec![0x06, 0, 0, 0, 0, 0, 0, 0, 4, b'1', b'.', b'5', b'0'],
        ),
        (
            QueryValue::Big("123".parse().unwrap()),
            vec![0x06, 0, 0, 0, 0, 0, 0, 0, 3, b'1', b'2', b'3'],
        ),
        (
            map(&[("aa", QueryValue::Int(2)), ("b", QueryValue::Int(1))]),
            vec![
                0x0a, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0xa1, b'b', 0x02, 0, 0, 0, 0,
                0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0xa2, b'a', b'a', 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
            ],
        ),
        (
            map(&[("b", QueryValue::Int(1)), ("aa", QueryValue::Int(2))]),
            vec![
                0x0a, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0xa1, b'b', 0x02, 0, 0, 0, 0,
                0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 3, 0xa2, b'a', b'a', 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
            ],
        ),
    ];
    for (value, expected) in vectors {
        assert!(TAGS.contains(&expected[0]));
        assert_vector(&value, &expected);
    }
    let mut permuted_set = new_set();
    permuted_set.insert(QueryValue::Int(1));
    permuted_set.insert(QueryValue::Int(2));
    let set_bytes = [
        0x09, 0, 0, 0, 0, 0, 0, 0, 2, 0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 2,
    ];
    assert_vector(&QueryValue::Set(permuted_set), &set_bytes);

    let empty_map = map(&[]);
    assert_vector(&empty_map, &[0x0a, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_vector(&QueryValue::F64(0.0), &[0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_vector(&QueryValue::F64(-0.0), &[0x03, 0, 0, 0, 0, 0, 0, 0, 0]);
    let nan_a = QueryValue::F64(f64::from_bits(0x7ff0_0000_0000_0001));
    let nan_b = QueryValue::F64(f64::from_bits(0xfff8_1234_5678_9abc));
    let nan_bytes = [0x03, 0x7f, 0xf8, 0, 0, 0, 0, 0, 0];
    assert_vector(&nan_a, &nan_bytes);
    assert_vector(&nan_b, &nan_bytes);

    let nested = map(&[(
        "n",
        map(&[(PREV_HASH_FIELD, QueryValue::Str("kept".into()))]),
    )]);
    assert_vector(
        &nested,
        &[
            0x0a, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0xa1, b'n', 0x0a, 0, 0, 0, 0, 0,
            0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 11, 0xaa, b'_', b'p', b'r', b'e', b'v', b'_', b'h', b'a',
            b's', b'h', 0x06, 0, 0, 0, 0, 0, 0, 0, 4, b'k', b'e', b'p', b't',
        ],
    );
    let base = map(&[("a", QueryValue::Int(1))]);
    let with_prev = map(&[
        ("a", QueryValue::Int(1)),
        (PREV_HASH_FIELD, QueryValue::Str("ignored".into())),
    ]);
    let base_bytes = [
        0x0a, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0xa1, b'a', 0x02, 0, 0, 0, 0, 0, 0,
        0, 1,
    ];
    assert_vector(&base, &base_bytes);
    assert_vector(&with_prev, &base_bytes);

    let key31 = "a".repeat(31);
    let key32 = "b".repeat(32);
    let value = map(&[
        (key32.as_str(), QueryValue::Null),
        (key31.as_str(), QueryValue::Null),
    ]);
    let boundary = [
        0x0a, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 32, 0xbf, b'a', b'a', b'a', b'a', b'a',
        b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a',
        b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', b'a', 0, 0, 0, 0, 0, 0, 0, 0,
        34, 0xd9, 32, b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b',
        b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b', b'b',
        b'b', b'b', b'b', b'b', 0,
    ];
    assert_vector(&value, &boundary);
}

#[test]
fn rv_funclib_ac384a3a_precomputed_key_encoding_matches_msgpack() {
    assert_eq!(
        PREV_HASH_KEY_BYTES,
        rmp_serde::to_vec(PREV_HASH_FIELD).unwrap()
    );
}

#[test]
fn rv_funclib_0caab4e1_key_serialization_failure_is_returned() {
    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    struct FailingKey;
    impl Hash for FailingKey {
        fn hash<H: Hasher>(&self, _: &mut H) {}
    }
    impl serde::Serialize for FailingKey {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("intentional failure"))
        }
    }
    let mut values = TMap::<FailingKey, Value<FailingKey>>::default();
    values.insert(FailingKey, Value::Null);
    assert!(canonical_bytes(&Value::Map(values)).is_err());
}
