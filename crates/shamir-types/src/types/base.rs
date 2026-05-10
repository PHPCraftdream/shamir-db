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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base58_encode_decode_roundtrip() {
        let data = b"hello world";
        let encoded = encode(data);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(data, decoded.as_slice());
        assert_eq!(encoded, "StV1DL6CwTryKyV");
    }

    #[test]
    fn test_decode16_roundtrip() {
        let data = b"0123456789abcdef"; // 16 bytes
        let encoded = encode(data);
        let decoded = decode16(&encoded).unwrap();
        assert_eq!(*data, decoded);
    }

    #[test]
    fn test_decode16_invalid_length() {
        let data = b"not 16 bytes";
        let encoded = encode(data);
        let result = decode16(&encoded);
        assert!(matches!(result, Err(Base58Error::InvalidLength { .. })));
    }

    #[test]
    fn test_decode16_invalid_character() {
        let invalid_str = "0123456789abcdeO"; // 'O' is not in the alphabet
        let result = decode16(invalid_str);
        // We just care that it's a decode error, not about the specific message.
        assert!(matches!(result, Err(Base58Error::DecodeError(_))));
    }

    #[test]
    fn test_base58_with_leading_zeros() {
        let data = &[0, 0, 1, 2, 3];
        let encoded = encode(data);
        assert!(encoded.starts_with("11"));
        let decoded = decode(&encoded).unwrap();
        assert_eq!(data, decoded.as_slice());
    }

    #[test]
    fn test_invalid_character() {
        let invalid_str = "StV1DL6CwTryKyV0"; // '0' is not in the alphabet
        let result = decode(invalid_str);
        // We just care that it's a decode error, not about the specific message.
        assert!(matches!(result, Err(Base58Error::DecodeError(_))));
    }
}
