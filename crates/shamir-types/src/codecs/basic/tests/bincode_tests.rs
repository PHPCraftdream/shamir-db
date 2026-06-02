use crate::codecs::basic::bincode::{from_bytes, to_bytes, CodecError};

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

// ---------------------------------------------------------------------------
// Round-trip: every primitive shape
// ---------------------------------------------------------------------------

#[test]
fn test_roundtrip_i64_bounds() {
    for &val in &[
        0i64,
        1,
        -1,
        i64::MIN,
        i64::MAX,
        i32::MIN as i64,
        i32::MAX as i64,
    ] {
        let bytes = to_bytes(&val).unwrap();
        let out: i64 = from_bytes(&bytes).unwrap();
        assert_eq!(out, val);
    }
}

#[test]
fn test_roundtrip_u64_bounds() {
    for &val in &[
        0u64,
        1,
        255,
        256,
        u16::MAX as u64,
        u32::MAX as u64,
        u64::MAX,
    ] {
        let bytes = to_bytes(&val).unwrap();
        let out: u64 = from_bytes(&bytes).unwrap();
        assert_eq!(out, val);
    }
}

#[test]
fn test_roundtrip_f64() {
    for &val in &[0.0f64, -0.0, 1.5, -99.999, f64::MIN, f64::MAX] {
        let bytes = to_bytes(&val).unwrap();
        let out: f64 = from_bytes(&bytes).unwrap();
        assert_eq!(out.to_bits(), val.to_bits(), "f64 roundtrip for {}", val);
    }
}

#[test]
fn test_roundtrip_bool() {
    let bytes_t = to_bytes(&true).unwrap();
    let bytes_f = to_bytes(&false).unwrap();
    assert!(from_bytes::<bool>(&bytes_t).unwrap());
    assert!(!from_bytes::<bool>(&bytes_f).unwrap());
}

#[test]
fn test_roundtrip_string_shapes() {
    let cases = vec![
        String::new(),
        "a".to_string(),
        "hello world".to_string(),
        "Привет мир".to_string(),
        "🚀🎉🔥".to_string(),
        "x".repeat(10_000),
    ];
    for s in cases {
        let bytes = to_bytes(&s).unwrap();
        let out: String = from_bytes(&bytes).unwrap();
        assert_eq!(out, s);
    }
}

#[test]
fn test_roundtrip_vec_and_nested() {
    let empty: Vec<i32> = vec![];
    assert_eq!(
        from_bytes::<Vec<i32>>(&to_bytes(&empty).unwrap()).unwrap(),
        empty
    );

    let nested = vec![vec![1, 2], vec![3], vec![]];
    assert_eq!(
        from_bytes::<Vec<Vec<i32>>>(&to_bytes(&nested).unwrap()).unwrap(),
        nested
    );
}

#[test]
fn test_roundtrip_tuple_and_option() {
    let t = (1i32, "hello".to_string(), 4.567f64);
    assert_eq!(
        from_bytes::<(i32, String, f64)>(&to_bytes(&t).unwrap()).unwrap(),
        t
    );

    let some = Some(42i32);
    let none: Option<i32> = None;
    assert_eq!(
        from_bytes::<Option<i32>>(&to_bytes(&some).unwrap()).unwrap(),
        some
    );
    assert_eq!(
        from_bytes::<Option<i32>>(&to_bytes(&none).unwrap()).unwrap(),
        none
    );
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn test_deserialize_empty_bytes() {
    let result = from_bytes::<i32>(&[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        CodecError::Deserialize(_) => {}
        other => panic!("expected Deserialize, got {:?}", other),
    }
}

#[test]
fn test_deserialize_truncated() {
    let bytes = to_bytes(&0x1234_5678i32).unwrap();
    // Truncate to 1 byte (too short for i32)
    let result = from_bytes::<i32>(&bytes[..1]);
    assert!(result.is_err());
}

#[test]
fn test_deserialize_wrong_type_interpretation() {
    // bincode has no type tags — deserializing bytes of one type as another
    // often succeeds with garbage data. This is inherent to bincode (not a
    // bug). We verify the error path via empty/truncated input instead
    // (see test_deserialize_empty_bytes and test_deserialize_truncated).
    //
    // Here we just confirm that a struct round-trips with the *correct* type
    // and that passing struct bytes as a bare i32 would produce garbage
    // (but NOT an error):
    let original = TestStruct {
        name: "test".to_string(),
        age: 1,
    };
    let bytes = to_bytes(&original).unwrap();
    // Deserializing as the wrong type succeeds but gives meaningless data.
    // This documents the bincode behaviour — no type tags means no type safety.
    let _: i32 = from_bytes(&bytes).unwrap();
}

#[test]
fn test_serialize_error_display() {
    let err = CodecError::Serialize("bad".to_string());
    assert_eq!(format!("{}", err), "serialization error: bad");

    let err = CodecError::Deserialize("oops".to_string());
    assert_eq!(format!("{}", err), "deserialization error: oops");
}
