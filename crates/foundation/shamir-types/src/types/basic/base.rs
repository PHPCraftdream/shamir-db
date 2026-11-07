//! A utility module for base-x encoding, specifically base58btc, using the `base-x` crate.

use thiserror::Error;

const ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

#[derive(Error, Debug, PartialEq, Eq)]
pub enum Base58Error {
    #[error("Invalid base58 string: {0}")]
    DecodeError(String),
    #[error("Invalid decoded length, expected {expected} but got {actual}")]
    InvalidLength { expected: usize, actual: usize },
}

/// Encodes a slice of bytes into a base58 string.
pub fn encode(data: &[u8]) -> String {
    base_x::encode(ALPHABET, data)
}

/// Decodes a base58 string into a vector of bytes.
pub fn decode(s: &str) -> Result<Vec<u8>, Base58Error> {
    base_x::decode(ALPHABET, s).map_err(|e| Base58Error::DecodeError(e.to_string()))
}

/// Decodes a base58 string into a 16-byte array.
pub fn decode16(s: &str) -> Result<[u8; 16], Base58Error> {
    let bytes = decode(s)?;
    bytes
        .try_into()
        .map_err(|v: Vec<u8>| Base58Error::InvalidLength {
            expected: 16,
            actual: v.len(),
        })
}
