//! Fixed canonical `shk1.<id>.<secret>` format. Secret buffers and encoded token strings are zeroizing.
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use std::fmt;
use zeroize::Zeroizing;

pub const TOKEN_PREFIX: &str = "shk1";
pub const TOKEN_LEN: usize = 71;
pub const ID_B64_LEN: usize = 22;
pub const SECRET_B64_LEN: usize = 43;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenId(pub [u8; 16]);
impl TokenId {
    pub fn to_b64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }
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
pub struct TokenSecret(Zeroizing<[u8; 32]>);
impl TokenSecret {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Debug for TokenSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<REDACTED>")
    }
}
pub struct ParsedToken {
    pub id: TokenId,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid token")]
pub struct TokenParseError;
#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("operating-system entropy unavailable")]
    Entropy,
}

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
pub fn encode_token(id: &TokenId, secret: &TokenSecret) -> Zeroizing<String> {
    let mut encoded = Zeroizing::new(String::with_capacity(TOKEN_LEN));
    encoded.push_str(TOKEN_PREFIX);
    encoded.push('.');
    URL_SAFE_NO_PAD.encode_string(id.0, &mut encoded);
    encoded.push('.');
    URL_SAFE_NO_PAD.encode_string(secret.as_bytes(), &mut encoded);
    encoded
}
pub fn generate_token_parts() -> Result<(TokenId, TokenSecret), TokenError> {
    let mut id = Zeroizing::new([0u8; 16]);
    let mut secret = Zeroizing::new([0u8; 32]);
    crate::common::crypto::try_random_bytes(id.as_mut()).map_err(|_| TokenError::Entropy)?;
    crate::common::crypto::try_random_bytes(secret.as_mut()).map_err(|_| TokenError::Entropy)?;
    Ok((TokenId(*id), TokenSecret(secret)))
}
