//! Fixed canonical `shk1.<id>.<secret>` format. Secret buffers and encoded token strings are zeroizing.
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use std::fmt;
use zeroize::Zeroizing;

/// Canonical token version prefix.
pub const TOKEN_PREFIX: &str = "shk1";
/// Canonical encoded token length in bytes.
pub const TOKEN_LEN: usize = 71;
/// Unpadded base64url identifier length in bytes.
pub const ID_B64_LEN: usize = 22;
/// Unpadded base64url secret length in bytes.
pub const SECRET_B64_LEN: usize = 43;

/// Opaque 128-bit token identifier.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(
    /// Raw identifier bytes.
    pub [u8; 16],
);
impl TokenId {
    /// Encodes the identifier as canonical unpadded base64url.
    pub fn to_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }
    /// Decodes a canonical 22-byte unpadded base64url identifier.
    pub fn from_b64(input: &str) -> Result<Self, TokenParseError> {
        if input.len() != ID_B64_LEN {
            return Err(TokenParseError);
        }
        let mut bytes = Zeroizing::new([0u8; 16]);
        URL_SAFE_NO_PAD
            .decode_slice(input, bytes.as_mut())
            .map_err(|_| TokenParseError)?;
        Ok(Self(*bytes))
    }
}
impl fmt::Debug for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_b64())
    }
}
/// Zeroizing 256-bit token secret; its debug representation is redacted.
pub struct TokenSecret(Zeroizing<[u8; 32]>);
impl TokenSecret {
    /// Borrows the secret bytes without transferring ownership.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Debug for TokenSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<REDACTED>")
    }
}
/// Parsed identifier and zeroizing secret.
pub struct ParsedToken {
    /// Identifier encoded in the token.
    pub id: TokenId,
    /// Secret encoded in the token; debug output remains redacted.
    pub secret: TokenSecret,
}
impl fmt::Debug for ParsedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedToken")
            .field("id", &self.id)
            .field("secret", &"<REDACTED>")
            .finish()
    }
}
/// Opaque failure for every malformed token representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid token")]
pub struct TokenParseError;
/// Failure to generate token material.
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("operating-system entropy unavailable")]
    /// Operating-system entropy was unavailable.
    Entropy,
}

/// Parses a canonical token from raw header bytes.
pub fn parse_token(input: &[u8]) -> Result<ParsedToken, TokenParseError> {
    if input.len() != TOKEN_LEN {
        return Err(TokenParseError);
    }
    if &input[..5] != b"shk1." || input[27] != b'.' {
        return Err(TokenParseError);
    }
    if !input[5..27]
        .iter()
        .chain(input[28..].iter())
        .all(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-')
    {
        return Err(TokenParseError);
    }
    let id_text = std::str::from_utf8(&input[5..27]).map_err(|_| TokenParseError)?;
    let secret_text = std::str::from_utf8(&input[28..]).map_err(|_| TokenParseError)?;
    let id = TokenId::from_b64(id_text)?;
    let mut secret = Zeroizing::new([0u8; 32]);
    URL_SAFE_NO_PAD
        .decode_slice(secret_text, secret.as_mut())
        .map_err(|_| TokenParseError)?;
    Ok(ParsedToken {
        id,
        secret: TokenSecret(secret),
    })
}
/// Encodes token parts into a zeroizing canonical token string.
pub fn encode_token(id: &TokenId, secret: &TokenSecret) -> Zeroizing<String> {
    let mut encoded = Zeroizing::new(String::with_capacity(TOKEN_LEN));
    encoded.push_str(TOKEN_PREFIX);
    encoded.push('.');
    URL_SAFE_NO_PAD.encode_string(id.0, &mut encoded);
    encoded.push('.');
    URL_SAFE_NO_PAD.encode_string(secret.as_bytes(), &mut encoded);
    encoded
}
/// Generates a random identifier and secret using operating-system entropy.
pub fn generate_token_parts() -> Result<(TokenId, TokenSecret), TokenError> {
    let mut id = Zeroizing::new([0u8; 16]);
    let mut secret = Zeroizing::new([0u8; 32]);
    crate::common::crypto::try_random_bytes(id.as_mut()).map_err(|_| TokenError::Entropy)?;
    crate::common::crypto::try_random_bytes(secret.as_mut()).map_err(|_| TokenError::Entropy)?;
    Ok((TokenId(*id), TokenSecret(secret)))
}
