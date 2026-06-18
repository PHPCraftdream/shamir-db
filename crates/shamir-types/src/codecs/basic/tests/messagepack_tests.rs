#![allow(deprecated)]
use crate::codecs::basic::messagepack::MessagePackCodec;
use crate::codecs::Codec;
use crate::types::common::{new_map, new_set, TSet};
use crate::types::value::UserValue;
use num_bigint::BigInt;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
struct SimpleStruct {
    a: i32,
    b: String,
}

#[test]
fn test_generic_msgpack_codec() {
    let codec = MessagePackCodec;
    let original = SimpleStruct {
        a: 10,
        b: "test".to_string(),
    };

    let encoded = codec.encode(&original).unwrap();
    let decoded: SimpleStruct = codec.decode(&encoded).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_messagepack_roundtrip() {
    let codec = MessagePackCodec;
    let mut map = new_map();
    map.insert("key1".to_string(), UserValue::Int(123));
    map.insert("key2".to_string(), UserValue::Str("value".to_string()));
    let values = vec![
        UserValue::Null,
        UserValue::Bool(true),
        UserValue::Int(42),
        UserValue::F64(123.456),
        UserValue::Str("hello".to_string()),
        UserValue::Bin(vec![1, 2, 3]),
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
fn test_msgpack_conversion_with_all_hints() {
    // Build the rich UserValue directly (the old test went through a legacy
    // text codec with type-hint prefixes; the value produced by that step is
    // constructed here explicitly so the msgpack round-trip assertion is unchanged).
    let msgpack_codec = MessagePackCodec;

    let mut expected_set = new_set();
    expected_set.insert(UserValue::Str("rust".to_string()));
    expected_set.insert(UserValue::Str("db".to_string()));
    let mut rich_map = new_map();
    rich_map.insert("tags".to_string(), UserValue::Set(expected_set.clone()));
    rich_map.insert(
        "history".to_string(),
        UserValue::List(vec![UserValue::Int(1), UserValue::Int(2)]),
    );
    rich_map.insert("version".to_string(), UserValue::Int(-10));
    rich_map.insert("user_id".to_string(), UserValue::Int(987));
    rich_map.insert("a39".to_string(), UserValue::F64(3.9));
    // dec: and big: validate but store as Str
    rich_map.insert("price".to_string(), UserValue::Str("19.99".to_string()));
    rich_map.insert("large".to_string(), UserValue::Str("10000".to_string()));
    let initial_value = UserValue::Map(rich_map);

    let msgpack_bytes = msgpack_codec.encode(&initial_value).unwrap();
    let final_value: UserValue = msgpack_codec.decode(&msgpack_bytes).unwrap();

    if let UserValue::Map(final_map) = final_value {
        assert_eq!(
            final_map.get("large"),
            Some(&UserValue::Str("10000".to_string()))
        );
        assert_eq!(
            final_map.get("price"),
            Some(&UserValue::Str("19.99".to_string()))
        );
        assert_eq!(final_map.get("version"), Some(&UserValue::Int(-10)));
        assert_eq!(final_map.get("user_id"), Some(&UserValue::Int(987)));
        assert_eq!(final_map.get("a39"), Some(&UserValue::F64(3.9)));
        assert_eq!(
            final_map.get("history"),
            Some(&UserValue::List(vec![UserValue::Int(1), UserValue::Int(2)]))
        );
        if let Some(UserValue::List(tags_list)) = final_map.get("tags") {
            let tags_set_from_list: TSet<UserValue> = tags_list.iter().cloned().collect();
            assert_eq!(
                tags_set_from_list, expected_set,
                "Set contents should be preserved in List"
            );
        } else {
            panic!("'tags' key should have been a List");
        }
    } else {
        panic!("Final value should have been a Map");
    }
}

#[test]
fn test_serialization_to_string_for_big_types_msgpack() {
    let codec = MessagePackCodec;
    let large_number_str = "1234567890123456789012345678901234567890";
    let price_str = "199.99";

    let big_val = UserValue::Big(BigInt::from_str(large_number_str).unwrap());
    let dec_val = UserValue::Dec(Decimal::from_str(price_str).unwrap());

    // Encode BigInt, decode, and check that it became a Str
    let big_encoded = codec.encode(&big_val).unwrap();
    let big_decoded: UserValue = codec.decode(&big_encoded).unwrap();
    assert_eq!(big_decoded, UserValue::Str(large_number_str.to_string()));

    // Encode Decimal, decode, and check that it became a Str
    let dec_encoded = codec.encode(&dec_val).unwrap();
    let dec_decoded: UserValue = codec.decode(&dec_encoded).unwrap();
    assert_eq!(dec_decoded, UserValue::Str(price_str.to_string()));
}
