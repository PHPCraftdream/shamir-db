use crate::codecs::{Codec, CodecError};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{from_slice, to_vec};

/// A generic codec for the JSON format.
pub struct JsonCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for JsonCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        to_vec(value).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}
