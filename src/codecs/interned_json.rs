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
    pub fn decode_to_inner(&self, bytes: &[u8]) -> Result<InnerValue, CodecError> {
        // Parse JSON to serde_json::Value
        let json_value: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|e| CodecError::Decode(e.to_string()))?;

        // Transform to InnerValue with interning
        Ok(transform_json_to_inner(json_value, self.interner))
    }

    /// Encode InnerValue to JSON bytes.
    pub fn encode_from_inner(&self, value: &InnerValue) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(value).map_err(|e| CodecError::Encode(e.to_string()))
    }
}

/// Transforms serde_json::Value to InnerValue, interning all string keys.
/// This is a single-pass transformation from JSON to InnerValue.
fn transform_json_to_inner(value: serde_json::Value, interner: &Interner) -> InnerValue {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::Int(u as i64)
            } else {
                Value::F64(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::Str(s),
        serde_json::Value::Array(arr) => {
            let inner_list = arr
                .into_iter()
                .map(|v| transform_json_to_inner(v, interner))
                .collect();
            Value::List(inner_list)
        }
        serde_json::Value::Object(obj) => {
            let mut inner_map = new_map_wc(obj.len());
            for (key, val) in obj {
                let interned_key = interner
                    .touch_ind(&key)
                    .expect("failed to intern key")
                    .val();
                let inner_val = transform_json_to_inner(val, interner);
                inner_map.insert(interned_key, inner_val);
            }
            Value::Map(inner_map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_simple_map() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Raw JSON bytes - no UserValue involved
        let json = br#"{"name":"Alice"}"#;

        let result = codec.decode_to_inner(json).unwrap();

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

        // Raw JSON bytes
        let json = br#"{"name":"Bob","age":30}"#;

        let result = codec.decode_to_inner(json).unwrap();

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

        // Raw JSON bytes
        let json = br#"{"name":"Alice","age":30}"#;

        // Decode directly to InnerValue with interning
        let inner_value = codec.decode_to_inner(json).unwrap();

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

        // Raw JSON: {"user": {"name": "Bob", "age": 30}}
        let json = br#"{"user":{"name":"Bob","age":30}}"#;

        let result = codec.decode_to_inner(json).unwrap();

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

        // Raw JSON with various types
        let json = br#"[null,true,false,42,3.14,"hello",[1,2]]"#;

        let result = codec.decode_to_inner(json).unwrap();

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

        // First call - raw JSON
        let json1 = br#"{"name":"Alice"}"#;
        codec.decode_to_inner(json1).unwrap();

        // Second call with same key
        let json2 = br#"{"name":"Bob"}"#;
        codec.decode_to_inner(json2).unwrap();

        // "name" should have the same ID (first key gets ID 1)
        let name_id = interner.get_ind("name").unwrap();
        assert_eq!(name_id, 1);
    }

    #[test]
    fn test_binary_data() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Raw JSON with array
        let json = br#"[1,2,3,4,5]"#;
        let result = codec.decode_to_inner(json).unwrap();

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

        // First call - raw JSON
        let json1 = br#"{"name":"Alice","email":"alice@example.com"}"#;
        codec.decode_to_inner(json1).unwrap();

        // Second call with overlapping keys
        let json2 = br#"{"name":"Bob","age":25}"#;
        let result = codec.decode_to_inner(json2).unwrap();

        // Check that keys are shared (JSON objects iterate in alphabetical order)
        assert_eq!(interner.get_ind("email"), Some(1)); // First alphabetically
        assert_eq!(interner.get_ind("name"), Some(2));
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

        // Raw JSON with unicode
        let json = r#"{"привет":"мир","emoji":"🚀🎉"}"#.as_bytes();
        let result = codec.decode_to_inner(json).unwrap();

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
        let json1 = b"{}";
        let result1 = codec.decode_to_inner(json1).unwrap();
        assert_eq!(result1, InnerValue::Map(new_map_wc(0)));

        // Empty list
        let json2 = b"[]";
        let result2 = codec.decode_to_inner(json2).unwrap();
        assert_eq!(result2, InnerValue::List(vec![]));
    }

    #[test]
    fn test_doc_decode_to_inner() {
        let interner = Interner::new();
        let codec = InternedJsonCodec::new(&interner);

        // Raw JSON bytes - direct decoding
        let json = br#"{"name":"Alice"}"#;

        // Decode to InnerValue with interned keys
        let inner_value = codec.decode_to_inner(json).unwrap();
        assert!(matches!(inner_value, InnerValue::Map(_)));
    }
}
