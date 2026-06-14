use crate::codecs::interned::{
    CodecFormat, InternedCodec, JsonInternedCodec, MsgPackInternedCodec,
};
use crate::core::interner::Interner;
use crate::types::common::new_map;
use crate::types::value::InnerValue;

fn test_roundtrip<C: InternedCodec>(codec: &C, format_name: &str) {
    let interner = Interner::new();

    // Create test data
    let mut map = new_map();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let age_key = interner.touch_ind("age").unwrap().into_key();

    map.insert(name_key, InnerValue::Str("Alice".to_string()));
    map.insert(age_key, InnerValue::Int(30));

    let value = InnerValue::Map(map);

    // Encode
    let encoded = codec.encode_with_interner(&value, &interner).unwrap();
    assert!(
        !encoded.is_empty(),
        "{} encoding should produce output",
        format_name
    );

    // Decode
    let decoded = codec.decode_with_interner(&encoded, &interner).unwrap();

    // Verify
    assert_eq!(
        decoded, value,
        "{} roundtrip should preserve data",
        format_name
    );
}

#[test]
fn test_json_interned_codec() {
    let codec = JsonInternedCodec;
    test_roundtrip(&codec, "JSON");
}

#[test]
fn test_msgpack_interned_codec() {
    let codec = MsgPackInternedCodec;
    test_roundtrip(&codec, "MessagePack");
}

#[test]
fn test_codec_format() {
    let json_codec = CodecFormat::Json.codec();
    let msgpack_codec = CodecFormat::MessagePack.codec();

    assert_eq!(json_codec.format_name(), "JSON");
    assert_eq!(msgpack_codec.format_name(), "MessagePack");
    assert_eq!(CodecFormat::Json.name(), "JSON");
    assert_eq!(CodecFormat::MessagePack.name(), "MessagePack");
}
