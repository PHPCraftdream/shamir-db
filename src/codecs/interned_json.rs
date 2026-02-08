//! JSON codec with on-the-fly key interning.
//!
//! This codec deserializes JSON directly into InnerValue (Value<u16>)
//! by interning string keys during deserialization.

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::common::new_map_wc;
use crate::types::value::{InnerValue, Value};

/// A codec that deserializes JSON directly to InnerValue with key interning.
pub struct InternedJsonCodec<'a> {
    interner: &'a Interner,
}

impl<'a> InternedJsonCodec<'a> {
    pub fn new(interner: &'a Interner) -> Self {
        Self { interner }
    }

    /// Decode JSON bytes directly to InnerValue, interning string keys.
    ///
    /// # Example
    /// ```rust
    /// use shamir_db::codecs::interned_json::InternedJsonCodec;
    /// use shamir_db::core::interner::Interner;
    /// use shamir_db::types::value::UserValue;
    /// use shamir_db::types::common::new_map;
    ///
    /// let interner = Interner::new();
    /// let codec = InternedJsonCodec::new(&interner);
    ///
    /// // Create a UserValue and encode to JSON
    /// let mut user_map = new_map();
    /// user_map.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    /// let user_value = UserValue::Map(user_map);
    /// let json = serde_json::to_vec(&user_value).unwrap();
    ///
    /// // Decode to InnerValue with interned keys
    /// let inner_value = codec.decode_to_inner(&json).unwrap();
    /// ```
    pub fn decode_to_inner(&self, bytes: &[u8]) -> Result<InnerValue, CodecError> {
        // First deserialize to UserValue (String keys)
        let user_value: UserValue = serde_json::from_slice(bytes)
            .map_err(|e| CodecError::Decode(e.to_string()))?;

        // Then transform to InnerValue with interning
        transform_to_inner(&user_value, self.interner)
    }

    /// Encode InnerValue to JSON bytes.
    pub fn encode_from_inner(&self, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(value).map_err(|e| CodecError::Encode(e.to_string()))
    }
}

use crate::types::value::UserValue;

/// Transforms UserValue to InnerValue, interning all string keys.
/// This is the core function that eliminates the need for transform::user_to_inner.
fn transform_to_inner(value: &UserValue, interner: &Interner) -> Result<InnerValue, CodecError> {
    Ok(match value {
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
                .map(|v| transform_to_inner(v, interner))
                .collect::<Result<Vec<_>, _>>()?;
            Value::List(inner_list)
        }
        Value::Set(set) => {
            let mut inner_set = crate::types::common::new_set_wc(set.len());
            for v in set {
                inner_set.insert(transform_to_inner(v, interner)?);
            }
            Value::Set(inner_set)
        }
        Value::Map(map) => {
            let mut inner_map = new_map_wc(map.len());
            for (key, val) in map {
                let interned_key = interner.touch_ind(key).map_err(|e| CodecError::Decode(e.to_string()))?.val();
                let inner_val = transform_to_inner(val, interner)?;
                inner_map.insert(interned_key, inner_val);
            }
            Value::Map(inner_map)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::common::{new_map, new_set};

    /// Helper to encode UserValue to JSON
    fn to_json(value: &UserValue) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn test_decode_simple_map() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue and encode to JSON
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        let user_value = UserValue::Map(user_map);
        let json = to_json(&user_value);

        let result = codec.decode_to_inner(&json).unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.len(), 1);
            // Check "name" was interned
            let name_id = interner.get_ind("name").expect("name should be interned");
            assert!(map.contains_key(&name_id));
            assert_eq!(
                map.get(&name_id),
                Some(&InnerValue::Str("Alice".to_string()))
            );
        } else {
            panic!("Expected Map, got {:?}", result);
        }
    }

    #[test]
    fn test_decode_multiple_keys() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue with multiple keys
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        user_map.insert("age".to_string(), UserValue::Int(30));
        let user_value = UserValue::Map(user_map);
        let json = to_json(&user_value);

        let result = codec.decode_to_inner(&json).unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.len(), 2);
            assert!(interner.get_ind("name").is_some());
            assert!(interner.get_ind("age").is_some());
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue, encode to JSON
        let mut user_map = new_map();
        user_map.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        user_map.insert("age".to_string(), UserValue::Int(30));
        let user_value = UserValue::Map(user_map);

        // Encode UserValue to JSON
        let json = to_json(&user_value);

        // Decode directly to InnerValue with interning
        let inner_value = codec.decode_to_inner(&json).unwrap();

        // Check keys were interned
        assert!(interner.get_ind("name").is_some());
        assert!(interner.get_ind("age").is_some());

        // Check values are correct
        if let InnerValue::Map(map) = inner_value {
            let name_id = interner.get_ind("name").unwrap();
            let age_id = interner.get_ind("age").unwrap();
            assert_eq!(map.get(&name_id), Some(&InnerValue::Str("Alice".to_string())));
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(30)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_nested_map() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create nested UserValue: {"user": {"name": "Bob", "age": 30}}
        let mut inner_map = new_map();
        inner_map.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        inner_map.insert("age".to_string(), UserValue::Int(30));

        let mut user_map = new_map();
        user_map.insert("user".to_string(), UserValue::Map(inner_map));
        let user_value = UserValue::Map(user_map);

        let json = to_json(&user_value);
        let result = codec.decode_to_inner(&json).unwrap();

        if let InnerValue::Map(outer) = result {
            assert_eq!(outer.len(), 1);
            assert!(interner.get_ind("user").is_some());
            assert!(interner.get_ind("name").is_some());
            assert!(interner.get_ind("age").is_some());
        } else {
            panic!("Expected nested Map");
        }
    }

    #[test]
    fn test_all_value_types() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue with various types
        let user_value = UserValue::List(vec![
            UserValue::Nil,
            UserValue::Bool(true),
            UserValue::Bool(false),
            UserValue::Int(42),
            UserValue::F64(3.14),
            UserValue::Str("hello".to_string()),
            UserValue::List(vec![UserValue::Int(1), UserValue::Int(2)]),
        ]);

        let json = to_json(&user_value);
        let result = codec.decode_to_inner(&json).unwrap();

        if let InnerValue::List(list) = result {
            assert_eq!(list.len(), 7);
            assert_eq!(list[0], InnerValue::Nil);
            assert_eq!(list[1], InnerValue::Bool(true));
            assert_eq!(list[2], InnerValue::Bool(false));
            assert_eq!(list[3], InnerValue::Int(42));
            assert_eq!(list[4], InnerValue::F64(3.14));
            assert_eq!(list[5], InnerValue::Str("hello".to_string()));
            assert_eq!(list[6], InnerValue::List(vec![InnerValue::Int(1), InnerValue::Int(2)]));
        } else {
            panic!("Expected List");
        }
    }

    #[test]
    fn test_interning_is_deterministic() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // First call - encode UserValue to JSON
        let mut map1 = new_map();
        map1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        let json1 = to_json(&UserValue::Map(map1));
        codec.decode_to_inner(&json1).unwrap();

        // Second call with same key
        let mut map2 = new_map();
        map2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        let json2 = to_json(&UserValue::Map(map2));
        codec.decode_to_inner(&json2).unwrap();

        // "name" should have the same ID (first key gets ID 1)
        let name_id = interner.get_ind("name").unwrap();
        assert_eq!(name_id, 1);
    }

    #[test]
    fn test_set_encoding() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue with a set
        // Note: Sets are encoded as arrays in JSON
        let mut user_set = new_set();
        user_set.insert(UserValue::Str("rust".to_string()));
        user_set.insert(UserValue::Str("db".to_string()));
        let user_value = UserValue::Set(user_set);

        let json = to_json(&user_value);
        let result = codec.decode_to_inner(&json).unwrap();

        // After roundtrip through JSON, Set becomes List
        // (because JSON doesn't have a set type)
        if let InnerValue::List(list) = result {
            assert_eq!(list.len(), 2);
            assert!(list.contains(&InnerValue::Str("rust".to_string())));
            assert!(list.contains(&InnerValue::Str("db".to_string())));
        } else {
            panic!("Expected List (Set encoded as array in JSON), got {:?}", result);
        }
    }

    #[test]
    fn test_binary_data() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue with binary data
        // Note: Bin is encoded as array in JSON
        let user_value = UserValue::Bin(vec![1, 2, 3, 4, 5]);
        let json = to_json(&user_value);
        let result = codec.decode_to_inner(&json).unwrap();

        // Bin becomes List after JSON roundtrip
        assert_eq!(result, InnerValue::List(vec![
            InnerValue::Int(1),
            InnerValue::Int(2),
            InnerValue::Int(3),
            InnerValue::Int(4),
            InnerValue::Int(5),
        ]));
    }

    #[test]
    fn test_multiple_calls_with_same_keys() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // First call with some keys
        let mut map1 = new_map();
        map1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
        map1.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
        let json1 = to_json(&UserValue::Map(map1));
        codec.decode_to_inner(&json1).unwrap();

        // Second call with overlapping keys
        let mut map2 = new_map();
        map2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
        map2.insert("age".to_string(), UserValue::Int(25));
        let json2 = to_json(&UserValue::Map(map2));
        let result = codec.decode_to_inner(&json2).unwrap();

        // Check that keys are shared
        assert_eq!(interner.get_ind("name"), Some(1)); // First key
        assert_eq!(interner.get_ind("email"), Some(2));
        assert_eq!(interner.get_ind("age"), Some(3)); // New key

        // Check result is correct
        if let InnerValue::Map(map) = result {
            let name_id = interner.get_ind("name").unwrap();
            let age_id = interner.get_ind("age").unwrap();
            assert_eq!(map.get(&name_id), Some(&InnerValue::Str("Bob".to_string())));
            assert_eq!(map.get(&age_id), Some(&InnerValue::Int(25)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_json_with_unicode() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Create UserValue with unicode strings
        let mut user_map = new_map();
        user_map.insert("привет".to_string(), UserValue::Str("мир".to_string()));
        user_map.insert("emoji".to_string(), UserValue::Str("🚀🎉".to_string()));
        let user_value = UserValue::Map(user_map);

        let json = to_json(&user_value);
        let result = codec.decode_to_inner(&json).unwrap();

        if let InnerValue::Map(map) = result {
            assert_eq!(map.len(), 2);
            assert!(interner.get_ind("привет").is_some());
            assert!(interner.get_ind("emoji").is_some());
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_json_empty_map_and_array() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Empty map
        let user_value1 = UserValue::Map(new_map());
        let json1 = to_json(&user_value1);
        let result1 = codec.decode_to_inner(&json1).unwrap();
        assert_eq!(result1, InnerValue::Map(new_map_wc(0)));

        // Empty list
        let user_value2 = UserValue::List(vec![]);
        let json2 = to_json(&user_value2);
        let result2 = codec.decode_to_inner(&json2).unwrap();
        assert_eq!(result2, InnerValue::List(vec![]));
    }
}
