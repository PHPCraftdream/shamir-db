use crate::types::base::{decode, decode16, encode, Base58Error};

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
