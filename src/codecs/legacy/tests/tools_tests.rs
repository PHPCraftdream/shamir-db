#[cfg(test)]
mod tests {
    use crate::codecs::transform::{inner_to_user, user_to_inner};
    use crate::codecs::Codec;
    use crate::core::interner::Interner;
    use crate::core::interner::{InternerKey, UserKey};
    use crate::types::common::{new_map, new_set};
    use crate::types::value::{InnerValue, UserValue};
    use num_bigint::BigInt;

    #[test]
    fn test_round_trip_transformation() {
        let interner = Interner::new();
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("John Doe".to_string()));
        user_map.insert("age".to_string(), UserValue::Int(30));
        user_map.insert(
            "balance".to_string(),
            UserValue::Big(BigInt::from(1_000_000_000_000i64)),
        );
        let mut user_set = new_set();
        user_set.insert(UserValue::Str("tag1".to_string()));
        user_set.insert(UserValue::Str("tag2".to_string()));
        let original_value =
            UserValue::List(vec![UserValue::Map(user_map), UserValue::Set(user_set)]);
        let result = user_to_inner(&original_value, &interner);

        // Verify new keys were collected
        assert!(result.has_new_keys());
        let keys = result.new_keys.as_ref().unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys
            .iter()
            .any(|(_, s): &(InternerKey, UserKey)| s.as_str() == "name"));
        assert!(keys
            .iter()
            .any(|(_, s): &(InternerKey, UserKey)| s.as_str() == "age"));
        assert!(keys
            .iter()
            .any(|(_, s): &(InternerKey, UserKey)| s.as_str() == "balance"));

        // Verify that calling again with same keys yields no new keys
        let result_again = user_to_inner(&original_value, &interner);
        assert!(!result_again.has_new_keys());
        assert!(result_again.new_keys.is_none());

        let name_id = interner.get_ind("name").unwrap();
        let age_id = interner.get_ind("age").unwrap();
        let balance_id = interner.get_ind("balance").unwrap();
        let mut expected_inner_map = new_map();
        expected_inner_map.insert(name_id, InnerValue::Str("John Doe".to_string()));
        expected_inner_map.insert(age_id, InnerValue::Int(30));
        expected_inner_map.insert(
            balance_id,
            InnerValue::Big(BigInt::from(1_000_000_000_000i64)),
        );
        let mut expected_inner_set = new_set();
        expected_inner_set.insert(InnerValue::Str("tag1".to_string()));
        expected_inner_set.insert(InnerValue::Str("tag2".to_string()));
        let expected_value = InnerValue::List(vec![
            InnerValue::Map(expected_inner_map),
            InnerValue::Set(expected_inner_set),
        ]);

        let inner_value_from_result = result.into_inner_value();
        assert_eq!(inner_value_from_result, expected_value);

        let final_value = inner_to_user(&inner_value_from_result, &interner);
        assert_eq!(original_value, final_value);
    }

    #[test]
    fn test_full_lifecycle_transformation() {
        let json_codec = crate::codecs::basic::json::JsonCodec;
        let interner = Interner::new();
        let large_number_str = "123456789012345678901234567890";
        let raw_json_1 = format!(
            r#"
        {{
            "set:tags": ["a", "b"],
            "big:balance": "{}",
            "dec:price": "99.95",
            "float:ratio": 0.123,
            "arr:history": [1, 2, 3],
            "simple_list": ["one", 2, false, null],
            "user": {{
                "name": "test",
                "i:id": 101,
                "active": true
            }}
        }}
        "#,
            large_number_str
        );

        let user_value_1: UserValue = json_codec.decode(raw_json_1.as_bytes()).unwrap();
        let result = user_to_inner(&user_value_1, &interner);
        assert!(result.has_new_keys(), "Expected new keys on first pass");

        let user_value_2 = inner_to_user(&result.inner_value, &interner);

        assert_eq!(
            user_value_1, user_value_2,
            "The user_to_inner and inner_to_user transformation cycle failed"
        );

        // --- Optional but recommended: Verify contents of user_value_1 to be sure ---
        let mut expected_set = new_set();
        expected_set.insert(UserValue::Str("a".to_string()));
        expected_set.insert(UserValue::Str("b".to_string()));

        let mut user_sub_map = new_map();
        user_sub_map.insert("name".to_string(), UserValue::Str("test".to_string()));
        user_sub_map.insert("id".to_string(), UserValue::Int(101));
        user_sub_map.insert("active".to_string(), UserValue::Bool(true));

        let mut expected_map = new_map();
        expected_map.insert("tags".to_string(), UserValue::Set(expected_set));
        expected_map.insert(
            "balance".to_string(),
            UserValue::Str(large_number_str.to_string()),
        );
        expected_map.insert("price".to_string(), UserValue::Str("99.95".to_string()));
        expected_map.insert("ratio".to_string(), UserValue::F64(0.123));
        expected_map.insert(
            "history".to_string(),
            UserValue::List(vec![
                UserValue::Int(1),
                UserValue::Int(2),
                UserValue::Int(3),
            ]),
        );
        expected_map.insert(
            "simple_list".to_string(),
            UserValue::List(vec![
                UserValue::Str("one".to_string()),
                UserValue::Int(2),
                UserValue::Bool(false),
                UserValue::Nil,
            ]),
        );
        expected_map.insert("user".to_string(), UserValue::Map(user_sub_map));

        assert_eq!(
            user_value_1,
            UserValue::Map(expected_map),
            "Initial JSON parsing created an unexpected structure"
        );
    }
}
