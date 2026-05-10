use serde::{de::DeserializeOwned, Serialize};

use super::CodecError;

/// A generic trait for encoding and decoding any serializable type.
/// `DeserializeOwned` is used as a bound, which means the type `T`
/// does not borrow from the input data. This is a common choice for simplicity.
pub trait Codec<T: Serialize + DeserializeOwned> {
    /// Encodes a value of type `T` into a byte vector.
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError>;

    /// Decodes a byte slice into a value of type `T`.
    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError>;
}
