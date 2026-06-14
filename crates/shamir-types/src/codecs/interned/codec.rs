//! Codec traits with interning support

use crate::codecs::interned::json::{inner_to_json, json_to_inner};
use crate::codecs::interned::messagepack::{inner_to_msgpack, msgpack_to_inner};
use crate::codecs::CodecError;
use crate::core::interner::Interner;
use crate::types::value::InnerValue;

/// Trait for codecs that support on-the-fly key interning
///
/// This trait is used by TableContext to decode/encode
/// client data (JSON/MessagePack) with automatic interning.
pub trait InternedCodec: Send + Sync {
    /// Decode bytes to InnerValue, interning string keys
    fn decode_with_interner(
        &self,
        bytes: &[u8],
        interner: &Interner,
    ) -> Result<InnerValue, CodecError>;

    /// Encode InnerValue to bytes
    fn encode_with_interner(
        &self,
        value: &InnerValue,
        interner: &Interner,
    ) -> Result<Vec<u8>, CodecError>;

    /// Get codec format name (for debugging/logging)
    fn format_name(&self) -> &'static str;
}

// ============================================================================
// Interned JSON Codec
// ============================================================================

/// JSON codec with automatic key interning
pub struct JsonInternedCodec;

impl InternedCodec for JsonInternedCodec {
    fn decode_with_interner(
        &self,
        bytes: &[u8],
        interner: &Interner,
    ) -> Result<InnerValue, CodecError> {
        json_to_inner(interner, bytes)
    }

    fn encode_with_interner(
        &self,
        value: &InnerValue,
        interner: &Interner,
    ) -> Result<Vec<u8>, CodecError> {
        inner_to_json(interner, value)
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
    fn decode_with_interner(
        &self,
        bytes: &[u8],
        interner: &Interner,
    ) -> Result<InnerValue, CodecError> {
        msgpack_to_inner(interner, bytes)
    }

    fn encode_with_interner(
        &self,
        value: &InnerValue,
        interner: &Interner,
    ) -> Result<Vec<u8>, CodecError> {
        inner_to_msgpack(interner, value)
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
