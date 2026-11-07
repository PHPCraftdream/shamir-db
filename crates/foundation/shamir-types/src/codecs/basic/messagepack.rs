use crate::codecs::{Codec, CodecError};
use rmp_serde::{from_slice, to_vec_named};
use serde::{de::DeserializeOwned, Serialize};

/// A generic codec for the MessagePack format.
pub struct MessagePackCodec;

impl<T: Serialize + DeserializeOwned> Codec<T> for MessagePackCodec {
    fn encode(&self, value: &T) -> Result<Vec<u8>, CodecError> {
        to_vec_named(value).map_err(|e| CodecError::Encode(e.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<T, CodecError> {
        from_slice(bytes).map_err(|e| CodecError::Decode(e.to_string()))
    }
}
