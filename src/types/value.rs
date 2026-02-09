#![allow(deprecated)]

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use num_bigint::BigInt;
use crate::types::common::{TMap, TSet, new_map_wc};
use std::hash::{Hash, Hasher};
use fxhash::FxHasher;
use std::cmp::Ord;
use serde::de::{self, Deserializer, Visitor, MapAccess, SeqAccess};
use serde::ser::{Serializer, SerializeMap, SerializeSeq};
use std::fmt::{self, Debug};
use std::str::FromStr;
use std::any::TypeId;
use crate::core::interner::InternedKey;

/// User-facing value type with string keys
/// 
/// **DEPRECATED & FOR TESTS ONLY**
/// 
/// This type should only be used in tests for convenience.
/// Production code should use `InnerValue` directly with interning.
#[deprecated(since = "0.1.0", note = "Use InnerValue instead. UserValue is for tests only.")]
pub type UserValue = Value<String>;
pub type InnerValue = Value<InternedKey>;

#[derive(Debug, Clone)]
pub enum Value<Key: Eq + Hash + Ord + Clone + Serialize + Debug> {
    Nil,
    Bool(bool),
    Int(i64),
    F64(f64),
    Dec(Decimal),
    Big(BigInt),
    Str(String),
    Bin(Vec<u8>),
    List(Vec<Value<Key>>),
    Set(TSet<Value<Key>>),
    Map(TMap<Key, Value<Key>>),
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + for<'de> Deserialize<'de> + Debug + 'static> Value<Key> {
    /// Serializes the `Value` into `Bytes` using MessagePack.
    pub fn to_bytes(&self) -> Bytes {
        Bytes::from(rmp_serde::to_vec(self).expect("Failed to serialize Value"))
    }

    /// Deserializes bytes into a `Value` using MessagePack.
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, rmp_serde::decode::Error> {
        rmp_serde::from_slice(bytes.as_ref())
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Serialize for Value<Key> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Value::Nil => serializer.serialize_unit(),
            Value::Bool(b) => serializer.serialize_bool(*b),
            Value::Int(i) => serializer.serialize_i64(*i),
            Value::F64(f) => serializer.serialize_f64(*f),
            Value::Dec(d) => serializer.serialize_str(&d.to_string()),
            Value::Big(b) => serializer.serialize_str(&b.to_string()),
            Value::Str(s) => serializer.serialize_str(s),
            Value::Bin(b) => serializer.serialize_bytes(b),
            Value::List(l) => {
                let mut seq = serializer.serialize_seq(Some(l.len()))?;
                for element in l {
                    seq.serialize_element(element)?;
                }
                seq.end()
            }
            Value::Set(s) => {
                let mut seq = serializer.serialize_seq(Some(s.len()))?;
                for element in s {
                    seq.serialize_element(element)?;
                }
                seq.end()
            }
            Value::Map(m) => {
                let mut map = serializer.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    map.serialize_entry(k, v)?;
                }
                map.end()
            }
        }
    }
}

fn parse_key_prefix(key: &str) -> (Option<&str>, &str) {
    if let Some((prefix, rest)) = key.split_once(':') {
        (Some(prefix), rest)
    } else {
        (None, key)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BigIntSource {
    Str(String),
    Int(i64),
    Uint(u64),
}

struct ValueVisitor<K>(std::marker::PhantomData<K>);

impl<'de, Key> Visitor<'de> for ValueVisitor<Key>
where
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug + 'static,
{
    type Value = Value<Key>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any valid SHAMIR value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> { Ok(Value::Bool(value)) }
    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> { Ok(Value::Int(value)) }
    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> { Ok(Value::Int(value as i64)) }
    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E> { Ok(Value::F64(value)) }
    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> where E: de::Error { Ok(Value::Str(value.to_owned())) }
    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> { Ok(Value::Str(value)) }
    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E> { Ok(Value::Bin(value.to_vec())) }
    fn visit_byte_buf<E>(self, value: Vec<u8>) -> Result<Self::Value, E> { Ok(Value::Bin(value)) }
    fn visit_none<E>(self) -> Result<Self::Value, E> { Ok(Value::Nil) }
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error> where D: Deserializer<'de> { deserializer.deserialize_any(self) }
    fn visit_unit<E>(self) -> Result<Self::Value, E> { Ok(Value::Nil) }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where A: SeqAccess<'de>,
    {
        let mut list = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element()? {
            list.push(elem);
        }
        Ok(Value::List(list))
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where M: MapAccess<'de>,
    {
        // For InnerValue (Key=u64) or other non-string keys, use direct deserialization.
        if TypeId::of::<Key>() != TypeId::of::<String>() {
            let mut inner_map = new_map_wc(map.size_hint().unwrap_or(0));
            while let Some((key, value)) = map.next_entry()? {
                inner_map.insert(key, value);
            }
            return Ok(Value::Map(inner_map));
        }

        // For UserValue (Key=String), use the special prefix-parsing logic.
        let mut inner_map = new_map_wc(map.size_hint().unwrap_or(0));
        while let Some(key_str) = map.next_key::<String>()? {
            let (prefix, real_key) = parse_key_prefix(&key_str);

            let value = match prefix {
                Some("i") => map.next_value::<i64>().map(Value::Int)?,
                Some("u") => map.next_value::<u64>().map(|v| Value::Int(v as i64))?,
                Some("float") => map.next_value::<f64>().map(Value::F64)?,
                Some("dec") => {
                    let s: String = map.next_value()?;
                    // Validate that it's a valid decimal, but store as string
                    let _ = Decimal::from_str(&s).map_err(de::Error::custom)?;
                    Value::Str(s)
                },
                Some("big") => {
                    let source: BigIntSource = map.next_value()?;
                    // Validate that it's a valid bigint, but store as string
                    let s = match source {
                        BigIntSource::Str(s) => {
                            let _ = BigInt::from_str(&s).map_err(de::Error::custom)?;
                            s
                        }
                        BigIntSource::Int(i) => BigInt::from(i).to_string(),
                        BigIntSource::Uint(u) => BigInt::from(u).to_string(),
                    };
                    Value::Str(s)
                },
                Some("arr") => map.next_value::<Vec<Value<Key>>>().map(Value::List)?,
                Some("set") => map.next_value::<TSet<Value<Key>>>().map(Value::Set)?,
                None => map.next_value()?,
                Some(unknown) => {
                    return Err(de::Error::custom(format!("unknown type prefix: '{}'", unknown)));
                }
            };

            let map_key = Key::deserialize(de::IntoDeserializer::into_deserializer(real_key))?;
            inner_map.insert(map_key, value);
        }
        Ok(Value::Map(inner_map))
    }
}

impl<'de, Key> Deserialize<'de> for Value<Key>
where
    Key: Deserialize<'de> + Eq + Hash + Ord + Clone + Serialize + Debug + 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValueVisitor(std::marker::PhantomData))
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> PartialEq for Value<Key> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::F64(a), Value::F64(b)) => {
                if a.is_nan() && b.is_nan() { true } else { a == b }
            }
            (Value::Dec(a), Value::Dec(b)) => a == b,
            (Value::Big(a), Value::Big(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bin(a), Value::Bin(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            _ => false,
        }
    }
}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Eq for Value<Key> {}

impl<Key: Eq + Hash + Ord + Clone + Serialize + Debug> Hash for Value<Key> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Value::Nil => {}
            Value::Bool(b) => b.hash(state),
            Value::Int(i) => i.hash(state),
            Value::F64(f) => f.to_bits().hash(state),
            Value::Dec(d) => d.hash(state),
            Value::Big(b) => b.hash(state),
            Value::Str(s) => s.hash(state),
            Value::Bin(b) => b.hash(state),
            Value::List(l) => l.hash(state),
            Value::Set(s) => {
                let mut xor_sum: u64 = 0;
                for v in s {
                    let mut hasher = FxHasher::default();
                    v.hash(&mut hasher);
                    xor_sum ^= hasher.finish();
                }
                xor_sum.hash(state);
            }
            Value::Map(m) => {
                // XOR approach - order-independent and no allocation
                let mut xor_sum: u64 = 0;
                for (k, v) in m {
                    let mut hasher = FxHasher::default();
                    k.hash(&mut hasher);
                    v.hash(&mut hasher);
                    xor_sum ^= hasher.finish();
                }
                xor_sum.hash(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::{new_map, new_set};

    fn calculate_hash<T: Hash>(t: &T) -> u64 {
        let mut hasher = FxHasher::default();
        t.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn test_set_hashing_is_deterministic() {
        let mut set1 = new_set();
        set1.insert(UserValue::Int(1));
        set1.insert(UserValue::Str("hello".to_string()));
        let mut set2 = new_set();
        set2.insert(UserValue::Str("hello".to_string()));
        set2.insert(UserValue::Int(1));
        assert_eq!(set1, set2);
        assert_eq!(calculate_hash(&UserValue::Set(set1)), calculate_hash(&UserValue::Set(set2)));
    }

    #[test]
    fn test_map_hashing_is_deterministic() {
        let mut map1 = new_map();
        map1.insert("a".to_string(), UserValue::Int(1));
        map1.insert("b".to_string(), UserValue::Str("world".to_string()));
        let mut map2 = new_map();
        map2.insert("b".to_string(), UserValue::Str("world".to_string()));
        map2.insert("a".to_string(), UserValue::Int(1));
        assert_eq!(map1, map2);
        assert_eq!(calculate_hash(&UserValue::Map(map1)), calculate_hash(&UserValue::Map(map2)));
    }

    #[test]
    fn test_bytes_serialization_roundtrip() {
        let mut map = new_map();
        map.insert(InternedKey::from_str("2m"), InnerValue::Str("hello".to_string()));
        map.insert(InternedKey::from_str("4P"), InnerValue::Int(99));
        let value = InnerValue::Map(map);

        let bytes = value.to_bytes();
        let reconstructed = InnerValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);

        let bytes_obj = Bytes::from(bytes.to_vec());
        let reconstructed2 = InnerValue::from_bytes(bytes_obj).unwrap();
        assert_eq!(value, reconstructed2);
    }

    #[test]
    fn test_all_value_types_serialization() {
        let test_cases = vec![
            UserValue::Nil,
            UserValue::Bool(true),
            UserValue::Bool(false),
            UserValue::Int(42),
            UserValue::Int(-42),
            UserValue::Int(i64::MAX),
            UserValue::Int(i64::MIN),
            UserValue::F64(std::f64::consts::PI),
            UserValue::F64(f64::INFINITY),
            UserValue::F64(f64::NEG_INFINITY),
            UserValue::Str("hello world".to_string()),
            UserValue::Str("".to_string()),
            UserValue::Bin(vec![1, 2, 3, 4, 5]),
            UserValue::Bin(vec![]),
        ];

        for value in test_cases {
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();
            assert_eq!(value, reconstructed, "Failed for: {:?}", value);
        }
    }

    #[test]
    fn test_decimal_serialization() {
        // Decimal and BigInt serialize as strings, so we test them separately
        let decimals = vec![
            Decimal::ZERO,
            Decimal::ONE,
            Decimal::from_str("0.000000001").unwrap(),
            Decimal::from_str("999999999999.999999999").unwrap(),
            Decimal::from_str("-123.456").unwrap(),
        ];

        for dec in decimals {
            let value = UserValue::Dec(dec);
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            // After deserialization, Decimal becomes Str due to MessagePack serialization
            match reconstructed {
                UserValue::Str(s) => {
                    assert_eq!(dec.to_string(), s, "Decimal should serialize to string");
                }
                _ => panic!("Expected Str, got {:?}", reconstructed),
            }
        }
    }

    #[test]
    fn test_bigint_serialization() {
        let bigints = vec![
            BigInt::from(0),
            BigInt::from(i64::MAX),
            BigInt::from(i64::MIN),
            BigInt::from_str("999999999999999999999999999999").unwrap(),
            BigInt::from_str("-999999999999999999999999999999").unwrap(),
        ];

        for big in bigints {
            let value = UserValue::Big(big.clone());
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            // After deserialization, BigInt becomes Str due to MessagePack serialization
            match reconstructed {
                UserValue::Str(s) => {
                    assert_eq!(big.to_string(), s, "BigInt should serialize to string");
                }
                _ => panic!("Expected Str, got {:?}", reconstructed),
            }
        }
    }

    #[test]
    fn test_nested_structures_serialization() {
        let mut inner_map = new_map();
        inner_map.insert("nested".to_string(), UserValue::Int(42));

        // Note: Sets serialize as arrays in MessagePack
        let value = UserValue::List(vec![
            UserValue::Map(inner_map),
            UserValue::List(vec![
                UserValue::Str("item1".to_string()),
                UserValue::Int(100),
            ]),
            UserValue::List(vec![UserValue::Bool(true), UserValue::Nil]),
        ]);

        let bytes = value.to_bytes();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_equality_for_all_types() {
        assert_eq!(UserValue::Nil, UserValue::Nil);

        assert_eq!(UserValue::Bool(true), UserValue::Bool(true));
        assert_ne!(UserValue::Bool(true), UserValue::Bool(false));

        assert_eq!(UserValue::Int(42), UserValue::Int(42));
        assert_ne!(UserValue::Int(42), UserValue::Int(43));

        assert_eq!(UserValue::F64(3.14), UserValue::F64(3.14));
        assert_ne!(UserValue::F64(3.14), UserValue::F64(2.71));

        // NaN equality
        assert_eq!(UserValue::F64(f64::NAN), UserValue::F64(f64::NAN));

        assert_eq!(UserValue::Str("test".to_string()), UserValue::Str("test".to_string()));
        assert_ne!(UserValue::Str("test".to_string()), UserValue::Str("other".to_string()));

        // Different types are not equal
        assert_ne!(UserValue::Int(42), UserValue::Str("42".to_string()));
        assert_ne!(UserValue::Bool(true), UserValue::Int(1));
    }

    #[test]
    fn test_hash_consistency() {
        let v1 = UserValue::Int(42);
        let v2 = UserValue::Int(42);
        assert_eq!(calculate_hash(&v1), calculate_hash(&v2));

        let v3 = UserValue::Int(43);
        assert_ne!(calculate_hash(&v1), calculate_hash(&v3));
    }

    #[test]
    fn test_nan_handling() {
        let nan1 = UserValue::F64(f64::NAN);
        let nan2 = UserValue::F64(f64::NAN);

        assert_eq!(nan1, nan2);
        assert_eq!(calculate_hash(&nan1), calculate_hash(&nan2));
    }

    #[test]
    fn test_empty_collections() {
        let empty_list = UserValue::List(vec![]);
        let empty_map = UserValue::Map(new_map());

        let list_bytes = empty_list.to_bytes();
        assert_eq!(empty_list, UserValue::from_bytes(&list_bytes).unwrap());

        let map_bytes = empty_map.to_bytes();
        assert_eq!(empty_map, UserValue::from_bytes(&map_bytes).unwrap());
    }

    #[test]
    fn test_large_collections() {
        let large_list = UserValue::List(
            (0..1000).map(|i| UserValue::Int(i)).collect()
        );
        let bytes = large_list.to_bytes();
        assert_eq!(large_list, UserValue::from_bytes(&bytes).unwrap());

        let mut large_map = new_map();
        for i in 0..1000 {
            large_map.insert(format!("key{}", i), UserValue::Int(i));
        }
        let map_value = UserValue::Map(large_map);
        let bytes = map_value.to_bytes();
        assert_eq!(map_value, UserValue::from_bytes(&bytes).unwrap());
    }

    #[test]
    fn test_deeply_nested_structures() {
        let mut nested = UserValue::Int(1);
        for _ in 0..10 {
            nested = UserValue::List(vec![nested]);
        }

        let bytes = nested.to_bytes();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(nested, reconstructed);
    }

    #[test]
    fn test_binary_data() {
        let binary_cases = vec![
            vec![],
            vec![0],
            vec![255],
            vec![0, 1, 2, 3, 4, 5],
            (0..=255).collect::<Vec<u8>>(),
        ];

        for bin in binary_cases {
            let value = UserValue::Bin(bin);
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();
            assert_eq!(value, reconstructed);
        }
    }

    #[test]
    fn test_unicode_strings() {
        let strings = vec![
            "",
            "hello",
            "Привет мир",
            "你好世界",
            "🚀🎉🔥",
            "Mixed: English, Русский, 中文, 🌍",
        ];

        for s in strings {
            let value = UserValue::Str(s.to_string());
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();
            assert_eq!(value, reconstructed);
        }
    }

    #[test]
    fn test_map_with_nested_values() {
        let mut inner_map = new_map();
        inner_map.insert("inner_key".to_string(), UserValue::Int(100));

        let mut outer_map = new_map();
        outer_map.insert("nested_map".to_string(), UserValue::Map(inner_map));
        outer_map.insert("simple".to_string(), UserValue::Bool(true));

        let value = UserValue::Map(outer_map);
        let bytes = value.to_bytes();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_inner_value_with_numeric_keys() {
        let mut map = new_map();
        map.insert(InternedKey::from_str("2"), InnerValue::Str("zero".to_string()));
        map.insert(InternedKey::from_str("zzzzzzzzzz"), InnerValue::Str("max".to_string()));
        map.insert(InternedKey::from_str("4P"), InnerValue::Int(42));

        let value = InnerValue::Map(map);
        let bytes = value.to_bytes();
        let reconstructed = InnerValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_hash_different_for_different_discriminants() {
        let int_val = UserValue::Int(42);
        let str_val = UserValue::Str("42".to_string());

        assert_ne!(calculate_hash(&int_val), calculate_hash(&str_val));
    }

    #[test]
    fn test_clone_preserves_equality() {
        let original = UserValue::List(vec![
            UserValue::Int(1),
            UserValue::Str("test".to_string()),
            UserValue::Bool(true),
        ]);

        let cloned = original.clone();
        assert_eq!(original, cloned);
        assert_eq!(calculate_hash(&original), calculate_hash(&cloned));
    }

    #[test]
    fn test_from_bytes_with_invalid_data() {
        // Completely invalid MessagePack data that should fail to deserialize
        let invalid_data = vec![0xC1]; // Reserved MessagePack type
        let result = UserValue::from_bytes(&invalid_data);
        assert!(result.is_err(), "Should fail to deserialize invalid MessagePack");
    }

    #[test]
    fn test_set_equality_ignores_order() {
        let mut set1 = new_set();
        set1.insert(UserValue::Int(1));
        set1.insert(UserValue::Int(2));
        set1.insert(UserValue::Int(3));

        let mut set2 = new_set();
        set2.insert(UserValue::Int(3));
        set2.insert(UserValue::Int(1));
        set2.insert(UserValue::Int(2));

        assert_eq!(UserValue::Set(set1), UserValue::Set(set2));
    }

    #[test]
    fn test_map_equality_ignores_order() {
        let mut map1 = new_map();
        map1.insert("x".to_string(), UserValue::Int(1));
        map1.insert("y".to_string(), UserValue::Int(2));

        let mut map2 = new_map();
        map2.insert("y".to_string(), UserValue::Int(2));
        map2.insert("x".to_string(), UserValue::Int(1));

        assert_eq!(UserValue::Map(map1), UserValue::Map(map2));
    }

    #[test]
    fn test_f64_special_values() {
        let special_values = vec![
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            0.0,
            -0.0,
        ];

        for &val in &special_values {
            let value = UserValue::F64(val);
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            match (value, reconstructed) {
                (UserValue::F64(a), UserValue::F64(b)) => {
                    if a.is_nan() {
                        assert!(b.is_nan());
                    } else {
                        assert_eq!(a, b);
                    }
                }
                _ => panic!("Type mismatch"),
            }
        }
    }

    #[test]
    fn test_decimal_roundtrip_preserves_string_representation() {
        let test_cases = vec![
            "0",
            "1",
            "123.456",
            "-123.456",
            "0.000000001",
            "999999999999.999999999",
            "-0.5",
            "1.0",
            "79228162514264337593543950335",  // Max Decimal
        ];

        for input_str in test_cases {
            let decimal = Decimal::from_str(input_str).unwrap();
            let value = UserValue::Dec(decimal);
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            match reconstructed {
                UserValue::Str(s) => {
                    // Сравниваем строковое представление
                    assert_eq!(s, decimal.to_string(),
                               "Decimal from '{}' should roundtrip correctly", input_str);
                }
                _ => panic!("Expected Str after deserialization, got {:?}", reconstructed),
            }
        }
    }

    #[test]
    fn test_bigint_roundtrip_preserves_string_representation() {
        let test_cases = vec![
            "0",
            "1",
            "-1",
            "42",
            "-42",
            "9223372036854775807",   // i64::MAX
            "-9223372036854775808",  // i64::MIN
            "18446744073709551615",  // u64::MAX
            "123456789012345678901234567890",
            "-999999999999999999999999999999",
            "340282366920938463463374607431768211456",  // 2^128
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",  // 2^256
        ];

        for input_str in test_cases {
            let bigint = BigInt::from_str(input_str).unwrap();
            let value = UserValue::Big(bigint.clone());
            let bytes = value.to_bytes();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            match reconstructed {
                UserValue::Str(s) => {
                    // Сравниваем строковое представление
                    assert_eq!(s, bigint.to_string(),
                               "BigInt from '{}' should roundtrip correctly", input_str);
                }
                _ => panic!("Expected Str after deserialization, got {:?}", reconstructed),
            }
        }
    }
}
