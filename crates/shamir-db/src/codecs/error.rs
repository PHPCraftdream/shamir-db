use thiserror::Error;

#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Failed to encode data: {0}")]
    Encode(String),
    #[error("Failed to decode data: {0}")]
    Decode(String),
}
