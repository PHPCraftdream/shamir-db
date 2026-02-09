//! Binary serialization using bincode.

use bytes::Bytes;
use std::fmt;

/// Binary codec errors
#[derive(Debug)]
pub enum CodecError {
    Serialize(String),
    Deserialize(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(msg) => write!(f, "serialization error: {}", msg),
            Self::Deserialize(msg) => write!(f, "deserialization error: {}", msg),
        }
    }
}

impl std::error::Error for CodecError {}

/// Serialize a value to bytes using bincode.
///
/// # use shamir_db::types::codec::{self, to_bytes};
/// /// # #[derive(serde::Serialize, serde::Deserialize)]
/// # #[derive(serde::Serialize, serde::Deserialize)]
/// # #[derive(serde::Serialize, serde::Deserialize)]
/// # let my_value = 42i32;
/// let bytes = to_bytes(&my_value)?;
/// # Ok::<(), shamir_db::types::codec::CodecError>(())
pub fn to_bytes<T>(value: &T) -> Result<Bytes, CodecError>
where
    T: serde::Serialize,
{
    bincode::serialize(value)
        .map(|v| Bytes::from(v))
        .map_err(|e| CodecError::Serialize(e.to_string()))
}

/// Deserialize bytes to an owned value using bincode.
///
/// # use shamir_db::types::codec::{self, to_bytes, from_bytes};
/// /// # #[derive(serde::Serialize, serde::Deserialize)]
/// # #[derive(serde::Serialize, serde::Deserialize)]
/// /// # let original = 42i32;
/// # let bytes = to_bytes(&original)?;
/// let value: i32 = from_bytes(&bytes)?;
/// # Ok::<(), shamir_db::types::codec::CodecError>(())
pub fn from_bytes<T>(bytes: &[u8]) -> Result<T, CodecError>
where
    T: serde::de::DeserializeOwned,
{
    bincode::deserialize(bytes)
        .map_err(|e| CodecError::Deserialize(e.to_string()))
}
