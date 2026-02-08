use crate::core::interner::{Interner, InternedKey, UserKey};
use crate::types::common::{new_map_wc, new_set_wc};
use crate::types::value::{InnerValue, UserValue, Value};

/// The result of a `user_to_inner` transformation.
///
/// This struct contains the resulting `InnerValue` and an optional collection
/// of any string keys that were newly interned during the transformation.
#[derive(Debug)]
pub struct TransformResult {
    pub inner_value: InnerValue,
    pub new_keys: Option<Vec<(InternedKey, UserKey)>>,
}

impl TransformResult {
    /// Checks if any new keys were created during the transformation.
    /// Returns true if `new_keys` is `Some` and not empty.
    pub fn has_new_keys(&self) -> bool {
        self.new_keys.as_ref().map_or(false, |v| !v.is_empty())
    }

    /// Consumes the result and returns its constituent parts.
    pub fn into_parts(self) -> (InnerValue, Option<Vec<(InternedKey, UserKey)>>) {
        (self.inner_value, self.new_keys)
    }

    /// Consumes the result and returns just the inner value.
    pub fn into_inner_value(self) -> InnerValue {
        self.inner_value
    }
}

fn user_to_inner_rec(
    value: &UserValue,
    interner: &Interner,
    new_keys: &mut Option<Vec<(InternedKey, UserKey)>>,
) -> InnerValue {
    match value {
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        Value::Int(i) => Value::Int(*i),
        Value::F64(f) => Value::F64(*f),
        Value::Dec(d) => Value::Dec(d.clone()),
        Value::Big(b) => Value::Big(b.clone()),
        Value::Str(s) => Value::Str(s.clone()),
        Value::Bin(b) => Value::Bin(b.clone()),
        Value::List(list) => {
            let inner_list = list
                .iter()
                .map(|v| user_to_inner_rec(v, interner, new_keys))
                .collect();
            Value::List(inner_list)
        }
        Value::Set(set) => {
            let mut inner_set = new_set_wc(set.len());
            for v in set {
                inner_set.insert(user_to_inner_rec(v, interner, new_keys));
            }
            Value::Set(inner_set)
        }
        Value::Map(map) => {
            let mut inner_map = new_map_wc(map.len());
            for (key, val) in map {
                let interned_key = interner.touch_ind(key).unwrap();
                if interned_key.is_new() {
                    new_keys
                        .get_or_insert_with(Vec::new)
                        .push((interned_key.key().clone(), UserKey::from_str(key)));
                }
                let inner_val = user_to_inner_rec(val, interner, new_keys);
                inner_map.insert(interned_key.key().clone(), inner_val);
            }
            Value::Map(inner_map)
        }
    }
}

/// Transforms a UserValue to an InnerValue, collecting newly interned keys.
///
/// This function is optimized to avoid heap allocations for the key collection
/// if no new keys are found.
pub fn user_to_inner(value: &UserValue, interner: &Interner) -> TransformResult {
    let mut new_keys: Option<Vec<(InternedKey, UserKey)>> = None;
    let inner_value = user_to_inner_rec(value, interner, &mut new_keys);
    TransformResult {
        inner_value,
        new_keys,
    }
}

pub fn inner_to_user(value: &InnerValue, interner: &Interner) -> UserValue {
    match value {
        Value::Nil => Value::Nil,
        Value::Bool(b) => Value::Bool(*b),
        Value::Int(i) => Value::Int(*i),
        Value::F64(f) => Value::F64(*f),
        Value::Dec(d) => Value::Dec(d.clone()),
        Value::Big(b) => Value::Big(b.clone()),
        Value::Str(s) => Value::Str(s.clone()),
        Value::Bin(b) => Value::Bin(b.clone()),
        Value::List(list) => {
            let user_list = list.iter().map(|v| inner_to_user(v, interner)).collect();
            Value::List(user_list)
        }
        Value::Set(set) => {
            let mut user_set = new_set_wc(set.len());
            for v in set {
                user_set.insert(inner_to_user(v, interner));
            }
            Value::Set(user_set)
        }
        Value::Map(map) => {
            let mut user_map = new_map_wc(map.len());
            for (key_id, val) in map {
                let key = interner
                    .get_str(key_id)
                    .expect("Data corruption: interned key not found");
                let user_val = inner_to_user(val, interner);
                user_map.insert(key.as_str().to_string(), user_val);
            }
            Value::Map(user_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::json::JsonCodec;
    use crate::codecs::Codec;
    use crate::types::common::{new_map, new_set};
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
        let original_value = UserValue::List(vec![UserValue::Map(user_map), UserValue::Set(user_set)]);
        let result = user_to_inner(&original_value, &interner);

        // Verify new keys were collected
        assert!(result.has_new_keys());
        let keys = result.new_keys.as_ref().unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.iter().any(|(_, s)| s.as_str() == "name"));
        assert!(keys.iter().any(|(_, s)| s.as_str() == "age"));
        assert!(keys.iter().any(|(_, s)| s.as_str() == "balance"));

        // Verify that calling again with the same keys yields no new keys
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
        let json_codec = JsonCodec;
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

        // --- Optional but recommended: Verify the contents of user_value_1 to be sure ---
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
        expected_map.insert(
            "price".to_string(),
            UserValue::Str("99.95".to_string()),
        );
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
