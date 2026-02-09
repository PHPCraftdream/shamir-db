use crate::codecs::bytes::{to_bytes, from_bytes};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct TestStruct {
    name: String,
    age: u32,
}

#[test]
fn test_roundtrip() {
    let original = TestStruct {
        name: "Alice".to_string(),
        age: 30,
    };

    let bytes = to_bytes(&original).unwrap();
    let deserialized: TestStruct = from_bytes(&bytes).unwrap();

    assert_eq!(original, deserialized);
}
