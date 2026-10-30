//! Binary serialization using bincode.

use bytes::Bytes;

use super::super::error::CodecError;

/// Serialize a value to bytes using bincode.
///
/// ```rust,ignore
/// fn main() -> Result<(), shamir_types::codecs::CodecError> {
///     use shamir_types::codecs::basic::to_bytes;
///
///     let my_value = 42i32;
///     let bytes = to_bytes(&my_value)?;
///     let _ = bytes;
///     Ok(())
/// }
/// ```
pub fn to_bytes<T>(value: &T) -> Result<Bytes, CodecError>
where
    T: serde::Serialize,
{
    bincode::serialize(value)
        .map(Bytes::from)
        .map_err(|e| CodecError::Encode(e.to_string()))
}

/// Deserialize bytes to an owned value using bincode.
///
/// This helper provides no recursion-depth protection. For untrusted input,
/// use it only with statically nonrecursive roots; recursive roots require a
/// type-specific decoder that enforces depth during deserialization. Byte and
/// allocation limits are separate from a recursion-depth limit.
///
/// ```rust,ignore
/// fn main() -> Result<(), shamir_types::codecs::CodecError> {
///     use shamir_types::codecs::basic::{from_bytes, to_bytes};
///
///     let original = 42i32;
///     let bytes = to_bytes(&original)?;
///     let value: i32 = from_bytes(&bytes)?;
///     assert_eq!(value, original);
///     Ok(())
/// }
/// ```
pub fn from_bytes<T>(bytes: &[u8]) -> Result<T, CodecError>
where
    T: serde::de::DeserializeOwned,
{
    bincode::deserialize(bytes).map_err(|e| CodecError::Decode(e.to_string()))
}
