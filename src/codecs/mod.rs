use serde::{de::DeserializeOwned, Serialize};
use thiserror::Error;

pub mod json;
pub mod message_pack;
pub mod interned_msgpack;
pub mod interned_json;
pub mod bytes;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Failed to encode data: {0}")]
    Encode(String),
    #[error("Failed to decode data: {0}")]
    Decode(String),
}

/// A generic trait for encoding and decoding any serializable type.
/// `DeserializeOwned` is used as a bound, which means the type `T`
/// does not borrow from the input data. This is a common choice for simplicity.
pub trait Codec<T: Serialize + DeserializeOwned> {
    /// Encodes a value of type `T` into a byte vector.
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError>;

    /// Decodes a byte slice into a value of type `T`.
    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError>;
}
