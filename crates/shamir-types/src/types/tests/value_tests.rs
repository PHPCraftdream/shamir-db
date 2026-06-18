#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use crate::core::interner::InternerKey;
    use crate::types::common::{new_map, new_set};
    use crate::types::value::{InnerValue, UserValue};
    use bytes::Bytes;
    use fxhash::FxHasher;
    use num_bigint::BigInt;
    use rust_decimal::Decimal;
    use std::hash::Hash;
    use std::hash::Hasher;
    use std::str::FromStr;

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
        assert_eq!(
            calculate_hash(&UserValue::Set(set1)),
            calculate_hash(&UserValue::Set(set2))
        );
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
        assert_eq!(
            calculate_hash(&UserValue::Map(map1)),
            calculate_hash(&UserValue::Map(map2))
        );
    }

    #[test]
    fn test_bytes_serialization_roundtrip() {
        let mut map = new_map();
        map.insert(InternerKey::new(42), InnerValue::Str("hello".to_string()));
        map.insert(InternerKey::new(255), InnerValue::Int(99));
        let value = InnerValue::Map(map);

        let bytes = value.to_bytes().unwrap();
        let reconstructed = InnerValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);

        let bytes_obj = Bytes::from(bytes.to_vec());
        let reconstructed2 = InnerValue::from_bytes(bytes_obj).unwrap();
        assert_eq!(value, reconstructed2);
    }

    #[test]
    fn test_all_value_types_serialization() {
        let test_cases = vec![
            UserValue::Null,
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
            let bytes = value.to_bytes().unwrap();
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
            let bytes = value.to_bytes().unwrap();
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
            let bytes = value.to_bytes().unwrap();
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
            UserValue::List(vec![UserValue::Bool(true), UserValue::Null]),
        ]);

        let bytes = value.to_bytes().unwrap();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_equality_for_all_types() {
        assert_eq!(UserValue::Null, UserValue::Null);

        assert_eq!(UserValue::Bool(true), UserValue::Bool(true));
        assert_ne!(UserValue::Bool(true), UserValue::Bool(false));

        assert_eq!(UserValue::Int(42), UserValue::Int(42));
        assert_ne!(UserValue::Int(42), UserValue::Int(43));

        assert_eq!(UserValue::F64(3.15), UserValue::F64(3.15));
        assert_ne!(UserValue::F64(3.15), UserValue::F64(2.72));

        // NaN equality
        assert_eq!(UserValue::F64(f64::NAN), UserValue::F64(f64::NAN));

        assert_eq!(
            UserValue::Str("test".to_string()),
            UserValue::Str("test".to_string())
        );
        assert_ne!(
            UserValue::Str("test".to_string()),
            UserValue::Str("other".to_string())
        );

        // Different types are not equal
        assert_ne!(UserValue::Int(42), UserValue::Str("42".to_string()));
        assert_ne!(UserValue::Bool(true), UserValue::Int(1));
    }

    #[test]
    fn test_null_serialization_roundtrip() {
        let value = UserValue::Null;
        let bytes = value.to_bytes().unwrap();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_null_in_list_roundtrip() {
        let value = UserValue::List(vec![
            UserValue::Int(1),
            UserValue::Null,
            UserValue::Str("test".to_string()),
        ]);
        let bytes = value.to_bytes().unwrap();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
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

        let list_bytes = empty_list.to_bytes().unwrap();
        assert_eq!(empty_list, UserValue::from_bytes(&list_bytes).unwrap());

        let map_bytes = empty_map.to_bytes().unwrap();
        assert_eq!(empty_map, UserValue::from_bytes(&map_bytes).unwrap());
    }

    #[test]
    fn test_large_collections() {
        let large_list = UserValue::List((0..1000).map(UserValue::Int).collect());
        let bytes = large_list.to_bytes().unwrap();
        assert_eq!(large_list, UserValue::from_bytes(&bytes).unwrap());

        let mut large_map = new_map();
        for i in 0..1000 {
            large_map.insert(format!("key{}", i), UserValue::Int(i));
        }
        let map_value = UserValue::Map(large_map);
        let bytes = map_value.to_bytes().unwrap();
        assert_eq!(map_value, UserValue::from_bytes(&bytes).unwrap());
    }

    #[test]
    fn test_deeply_nested_structures() {
        let mut nested = UserValue::Int(1);
        for _ in 0..10 {
            nested = UserValue::List(vec![nested]);
        }

        let bytes = nested.to_bytes().unwrap();
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
            let bytes = value.to_bytes().unwrap();
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
            let bytes = value.to_bytes().unwrap();
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
        let bytes = value.to_bytes().unwrap();
        let reconstructed = UserValue::from_bytes(&bytes).unwrap();
        assert_eq!(value, reconstructed);
    }

    #[test]
    fn test_inner_value_with_numeric_keys() {
        let mut map = new_map();
        map.insert(InternerKey::new(1), InnerValue::Str("zero".to_string()));
        map.insert(InternerKey::new(1000), InnerValue::Str("max".to_string()));
        map.insert(InternerKey::new(42), InnerValue::Int(42));

        let value = InnerValue::Map(map);
        let bytes = value.to_bytes().unwrap();
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
        assert!(
            result.is_err(),
            "Should fail to deserialize invalid MessagePack"
        );
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
        let special_values = vec![f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 0.0, -0.0];

        for &val in &special_values {
            let value = UserValue::F64(val);
            let bytes = value.to_bytes().unwrap();
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
            "79228162514264337593543950335", // Max Decimal
        ];

        for input_str in test_cases {
            let decimal = Decimal::from_str(input_str).unwrap();
            let value = UserValue::Dec(decimal);
            let bytes = value.to_bytes().unwrap();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            match reconstructed {
                UserValue::Str(s) => {
                    // Сравниваем строковое представление
                    assert_eq!(
                        s,
                        decimal.to_string(),
                        "Decimal from '{}' should roundtrip correctly",
                        input_str
                    );
                }
                _ => panic!(
                    "Expected Str after deserialization, got {:?}",
                    reconstructed
                ),
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
            "9223372036854775807",  // i64::MAX
            "-9223372036854775808", // i64::MIN
            "18446744073709551615", // u64::MAX
            "123456789012345678901234567890",
            "-999999999999999999999999999999",
            "340282366920938463463374607431768211456", // 2^128
            "115792089237316195423570985008687907853269984665640564039457584007913129639936", // 2^256
        ];

        for input_str in test_cases {
            let bigint = BigInt::from_str(input_str).unwrap();
            let value = UserValue::Big(bigint.clone());
            let bytes = value.to_bytes().unwrap();
            let reconstructed = UserValue::from_bytes(&bytes).unwrap();

            match reconstructed {
                UserValue::Str(s) => {
                    // Сравниваем строковое представление
                    assert_eq!(
                        s,
                        bigint.to_string(),
                        "BigInt from '{}' should roundtrip correctly",
                        input_str
                    );
                }
                _ => panic!(
                    "Expected Str after deserialization, got {:?}",
                    reconstructed
                ),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Hash / Eq for remaining types: Dec, Big, F64 non-NaN
    // -----------------------------------------------------------------------

    #[test]
    fn test_dec_hash_and_eq() {
        let d1 = UserValue::Dec(Decimal::from_str("1.23").unwrap());
        let d2 = UserValue::Dec(Decimal::from_str("1.23").unwrap());
        assert_eq!(d1, d2);
        assert_eq!(calculate_hash(&d1), calculate_hash(&d2));

        let d3 = UserValue::Dec(Decimal::from_str("4.56").unwrap());
        assert_ne!(d1, d3);
    }

    #[test]
    fn test_big_hash_and_eq() {
        let b1 = UserValue::Big(BigInt::from(999));
        let b2 = UserValue::Big(BigInt::from(999));
        assert_eq!(b1, b2);
        assert_eq!(calculate_hash(&b1), calculate_hash(&b2));

        let b3 = UserValue::Big(BigInt::from(1));
        assert_ne!(b1, b3);
    }

    #[test]
    fn test_f64_neg_zero_hash() {
        let p = UserValue::F64(0.0);
        let n = UserValue::F64(-0.0);
        // Different bit patterns → different hashes
        assert_ne!(calculate_hash(&p), calculate_hash(&n));
    }

    #[test]
    fn test_cross_type_inequality() {
        assert_ne!(UserValue::Null, UserValue::Int(0));
        assert_ne!(UserValue::Bool(false), UserValue::Int(0));
        assert_ne!(UserValue::Int(0), UserValue::F64(0.0));
        assert_ne!(UserValue::Str("1".to_string()), UserValue::Int(1));
        assert_ne!(UserValue::Bin(vec![]), UserValue::List(vec![]));
    }

    #[test]
    fn test_set_hash_order_independent() {
        let mut s1 = new_set();
        s1.insert(UserValue::Int(10));
        s1.insert(UserValue::Int(20));
        s1.insert(UserValue::Int(30));

        let mut s2 = new_set();
        s2.insert(UserValue::Int(30));
        s2.insert(UserValue::Int(10));
        s2.insert(UserValue::Int(20));

        assert_eq!(
            calculate_hash(&UserValue::Set(s1)),
            calculate_hash(&UserValue::Set(s2))
        );
    }

    #[test]
    fn test_map_hash_order_independent() {
        let mut m1 = new_map();
        m1.insert("a".to_string(), UserValue::Int(1));
        m1.insert("b".to_string(), UserValue::Int(2));

        let mut m2 = new_map();
        m2.insert("b".to_string(), UserValue::Int(2));
        m2.insert("a".to_string(), UserValue::Int(1));

        assert_eq!(
            calculate_hash(&UserValue::Map(m1)),
            calculate_hash(&UserValue::Map(m2))
        );
    }

    #[test]
    fn test_inner_value_map_deserialization() {
        // InnerValue uses InternerKey keys — hits the non-String branch
        let mut map = new_map();
        map.insert(InternerKey::new(1), InnerValue::Int(10));
        map.insert(InternerKey::new(2), InnerValue::Str("hello".to_string()));
        let val = InnerValue::Map(map);
        let bytes = rmp_serde::to_vec(&val).unwrap();
        let decoded: InnerValue = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn test_from_bytes_empty_input() {
        let result = UserValue::from_bytes([]);
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_truncated() {
        // Use a value whose encoding is multiple bytes long
        let bytes = UserValue::List(vec![UserValue::Int(1), UserValue::Int(2)])
            .to_bytes()
            .unwrap();
        // Truncate to just 1 byte (the fixarray header)
        let result = UserValue::from_bytes(&bytes[..1]);
        assert!(result.is_err());
    }

    #[test]
    fn test_msgpack_bytes_apis() {
        let val = UserValue::Int(42);
        let b = val.to_bytes().unwrap();
        assert!(!b.is_empty());
        let decoded = UserValue::from_bytes(b.as_ref()).unwrap();
        assert_eq!(val, decoded);
        // Also works with Bytes argument
        let decoded2 = UserValue::from_bytes(b).unwrap();
        assert_eq!(val, decoded2);
    }
}
