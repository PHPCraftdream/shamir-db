//! Codec traits with interning support

use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::value::InnerValue;

/// Trait for codecs that support on-the-fly key interning
///
/// This trait is used by TableContext to decode/encode
/// client data (JSON/MessagePack) with automatic interning.
pub trait InternedCodec: Send + Sync {
    /// Decode bytes to InnerValue, interning string keys
    fn decode_with_interner(&self, bytes: &[u8], interner: &Interner) -> Result<InnerValue, CodecError>;

    /// Encode InnerValue to bytes
    fn encode_with_interner(&self, value: &InnerValue, interner: &Interner) -> Result<Vec<u8>, CodecError>;

    /// Get codec format name (for debugging/logging)
    fn format_name(&self) -> &'static str;
}

// ============================================================================
// Interned JSON Codec
// ============================================================================

/// JSON codec with automatic key interning
pub struct JsonInternedCodec;

impl InternedCodec for JsonInternedCodec {
    fn decode_with_interner(&self, bytes: &[u8], interner: &Interner) -> Result<InnerValue, CodecError> {
        crate::codecs::interned_json::json_to_inner(interner, bytes)
    }

    fn encode_with_interner(&self, value: &InnerValue, interner: &Interner) -> Result<Vec<u8>, CodecError> {
        crate::codecs::interned_json::inner_to_json(interner, value)
    }

    fn format_name(&self) -> &'static str {
        "JSON"
    }
}

// ============================================================================
// Interned MessagePack Codec
// ============================================================================

/// MessagePack codec with automatic key interning
pub struct MsgPackInternedCodec;

impl InternedCodec for MsgPackInternedCodec {
    fn decode_with_interner(&self, bytes: &[u8], interner: &Interner) -> Result<InnerValue, CodecError> {
        crate::codecs::interned_msgpack::msgpack_to_inner(interner, bytes)
    }

    fn encode_with_interner(&self, value: &InnerValue, interner: &Interner) -> Result<Vec<u8>, CodecError> {
        crate::codecs::interned_msgpack::inner_to_msgpack(interner, value)
    }

    fn format_name(&self) -> &'static str {
        "MessagePack"
    }
}

// ============================================================================
// Codec Format Enum (for configuration)
// ============================================================================

/// Supported codec formats
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecFormat {
    Json,
    MessagePack,
}

impl CodecFormat {
    /// Create codec instance for this format
    pub fn codec(self) -> Box<dyn InternedCodec> {
        match self {
            CodecFormat::Json => Box::new(JsonInternedCodec),
            CodecFormat::MessagePack => Box::new(MsgPackInternedCodec),
        }
    }

    /// Get format name
    pub fn name(self) -> &'static str {
        match self {
            CodecFormat::Json => "JSON",
            CodecFormat::MessagePack => "MessagePack",
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::interner::Interner;
    use crate::types::common::new_map;

    fn test_roundtrip<C: InternedCodec>(codec: &C, format_name: &str) {
        let interner = Interner::new();

        // Create test data
        let mut map = new_map();
        let name_key = interner.touch_ind("name").unwrap().key().clone();
        let age_key = interner.touch_ind("age").unwrap().key().clone();

        map.insert(name_key, InnerValue::Str("Alice".to_string()));
        map.insert(age_key, InnerValue::Int(30));

        let value = InnerValue::Map(map);

        // Encode
        let encoded = codec.encode_with_interner(&value, &interner).unwrap();
        assert!(!encoded.is_empty(), "{} encoding should produce output", format_name);

        // Decode
        let decoded = codec.decode_with_interner(&encoded, &interner).unwrap();

        // Verify
        assert_eq!(decoded, value, "{} roundtrip should preserve data", format_name);
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
}
