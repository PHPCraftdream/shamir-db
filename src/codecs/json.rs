use crate::codecs::{Codec, CodecError};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{from_slice, to_vec};

/// A generic codec for the JSON format.
pub struct JsonCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for JsonCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        to_vec(value).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct SimpleStruct {
        a: i32,
        b: String,
    }

    #[test]
    fn test_generic_json_codec() {
        let codec = JsonCodec;
        let original = SimpleStruct { a: 10, b: "test".to_string() };
        let encoded = codec.encode(&original).unwrap();
        let decoded: SimpleStruct = codec.decode(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    mod user_value_tests {
        use super::*;
        use crate::types::common::{new_map, new_set};
        use crate::types::value::UserValue;
        use rust_decimal::Decimal;
        use num_bigint::BigInt;
        use std::str::FromStr;

        #[test]
        fn test_json_roundtrip() {
            let codec = JsonCodec;
            let mut map = new_map();
            map.insert("key1".to_string(), UserValue::Int(123));
            map.insert("key2".to_string(), UserValue::Str("value".to_string()));
            let values = vec![
                UserValue::Nil,
                UserValue::Bool(true),
                UserValue::Int(42),
                UserValue::F64(123.456),
                UserValue::Str("hello".to_string()),
                UserValue::List(vec![UserValue::Int(1), UserValue::Str("two".to_string())]),
                UserValue::Map(map),
            ];
            for original in values {
                let encoded = codec.encode(&original).unwrap();
                let decoded: UserValue = codec.decode(&encoded).unwrap();
                assert_eq!(original, decoded);
            }
        }

        #[test]
        fn test_decode_from_raw_json_string() {
            let codec = JsonCodec;
            let raw_json = r#"{
                "user": "test",
                "active": true,
                "roles": ["admin", "editor"],
                "prefs": {"theme": "dark", "notifications": 1},
                "last_login": null
            }"#;
            let mut prefs_map = new_map();
            prefs_map.insert("theme".to_string(), UserValue::Str("dark".to_string()));
            prefs_map.insert("notifications".to_string(), UserValue::Int(1));
            let mut expected_map = new_map();
            expected_map.insert("user".to_string(), UserValue::Str("test".to_string()));
            expected_map.insert("active".to_string(), UserValue::Bool(true));
            expected_map.insert("roles".to_string(), UserValue::List(vec![UserValue::Str("admin".to_string()), UserValue::Str("editor".to_string())]));
            expected_map.insert("prefs".to_string(), UserValue::Map(prefs_map));
            expected_map.insert("last_login".to_string(), UserValue::Nil);
            let expected_value = UserValue::Map(expected_map);
            let decoded_value: UserValue = codec.decode(raw_json.as_bytes()).unwrap();
            assert_eq!(decoded_value, expected_value);
        }

        #[test]
        fn test_decode_with_all_type_prefixes() {
            let codec = JsonCodec;
            let raw_json = r#"{
                "set:tags": ["rust", "db"],
                "arr:history": [{"op": "add", "val": 1}],
                "i:version": -10,
                "u:user_id": 1234567890,
                "float:pi": 3.14,
                "dec:price": "19.99",
                "big:large": "10000"
            }"#;
            let mut expected_set = new_set();
            expected_set.insert(UserValue::Str("rust".to_string()));
            expected_set.insert(UserValue::Str("db".to_string()));
            let mut op1 = new_map();
            op1.insert("op".to_string(), UserValue::Str("add".to_string()));
            op1.insert("val".to_string(), UserValue::Int(1));
            let expected_arr = vec![UserValue::Map(op1)];
            let mut expected_map = new_map();
            expected_map.insert("tags".to_string(), UserValue::Set(expected_set));
            expected_map.insert("history".to_string(), UserValue::List(expected_arr));
            expected_map.insert("version".to_string(), UserValue::Int(-10));
            expected_map.insert("user_id".to_string(), UserValue::Int(1234567890));
            expected_map.insert("pi".to_string(), UserValue::F64(3.14));
            expected_map.insert("price".to_string(), UserValue::Str("19.99".to_string()));
            expected_map.insert("large".to_string(), UserValue::Str("10000".to_string()));
            let expected_value = UserValue::Map(expected_map);
            let decoded_value: UserValue = codec.decode(raw_json.as_bytes()).unwrap();
            assert_eq!(decoded_value, expected_value);
        }

        #[test]
        fn test_decode_with_truly_large_bigint() {
            let codec = JsonCodec;
            let large_number_str = "1234567890123456789012345678901234567890";
            let raw_json = format!(r#"{{ "big:large": "{}" }}"#, large_number_str);
            let mut expected_map = new_map();
            expected_map.insert("large".to_string(), UserValue::Str(large_number_str.to_string()));
            let expected_value = UserValue::Map(expected_map);
            let decoded_value: UserValue = codec.decode(raw_json.as_bytes()).unwrap();
            assert_eq!(decoded_value, expected_value);
        }

        #[test]
        fn test_serialization_to_string_for_big_types() {
            let codec = JsonCodec;
            let large_number_str = "1234567890123456789012345678901234567890";
            let price_str = "199.99";
            let big_val = UserValue::Big(BigInt::from_str(large_number_str).unwrap());
            let dec_val = UserValue::Dec(Decimal::from_str(price_str).unwrap());
            let big_encoded = codec.encode(&big_val).unwrap();
            let dec_encoded = codec.encode(&dec_val).unwrap();
            assert_eq!(String::from_utf8(big_encoded).unwrap().trim(), format!(r#""{}""#, large_number_str));
            assert_eq!(String::from_utf8(dec_encoded).unwrap().trim(), format!(r#""{}""#, price_str));
        }

        #[test]
        fn test_fail_on_unknown_prefix() {
            let codec = JsonCodec;
            let json_unknown_prefix = r#"{ "foo:bar": 123 }"#;
            let result: Result<UserValue, _> = codec.decode(json_unknown_prefix.as_bytes());
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("unknown type prefix: 'foo'"));
        }

        #[test]
        fn test_decode_bigint_from_number() {
            let codec = JsonCodec;
            let json = r#"{ "big:balance": 12345 }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_ok());
            let value = result.unwrap();
            if let UserValue::Map(map) = value {
                assert_eq!(map.get("balance"), Some(&UserValue::Str("12345".to_string())));
            } else {
                panic!("Expected a map");
            }

            let json = r#"{ "big:balance": "12345" }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_ok());
            let value = result.unwrap();
            if let UserValue::Map(map) = value {
                assert_eq!(map.get("balance"), Some(&UserValue::Str("12345".to_string())));
            } else {
                panic!("Expected a map");
            }
        }

        #[test]
        fn test_decode_decimal_validation() {
            let codec = JsonCodec;
            // Valid decimal should decode to Str
            let json = r#"{ "dec:price": "19.99" }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_ok());
            let value = result.unwrap();
            if let UserValue::Map(map) = value {
                assert_eq!(map.get("price"), Some(&UserValue::Str("19.99".to_string())));
            } else {
                panic!("Expected a map");
            }

            // Invalid decimal should fail
            let json = r#"{ "dec:price": "not_a_decimal" }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_err());
        }

        #[test]
        fn test_decode_bigint_validation() {
            let codec = JsonCodec;
            // Valid bigint should decode to Str
            let json = r#"{ "big:balance": "123456789012345678901234567890" }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_ok());
            let value = result.unwrap();
            if let UserValue::Map(map) = value {
                assert_eq!(map.get("balance"), Some(&UserValue::Str("123456789012345678901234567890".to_string())));
            } else {
                panic!("Expected a map");
            }

            // Invalid bigint should fail
            let json = r#"{ "big:balance": "not_a_number" }"#;
            let result: Result<UserValue, _> = codec.decode(json.as_bytes());
            assert!(result.is_err());
        }
    }
}
