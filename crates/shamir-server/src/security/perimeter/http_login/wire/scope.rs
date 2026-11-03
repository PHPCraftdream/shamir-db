use super::WireError;
use shamir_connect::server::http_login::{
    encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope,
};
use shamir_connect::server::token::{TokenChannels, TokenMode};

/// Parsed, bounded token scope retained without duplicating its encoded transcript form.
pub struct TokenScope<'a> {
    /// UTF-8 label.
    pub label: &'a str,
    /// Token rights mode.
    pub mode: TokenMode,
    /// Permitted transport channels.
    pub channels: TokenChannels,
    /// Sorted database allowlist; `None` means all databases.
    pub databases: Option<Vec<&'a str>>,
}

/// Token scope rights mode.
pub type TokenScopeMode = TokenMode;

/// Token transport channels accepted in a scope.
pub type TokenScopeChannels = TokenChannels;

impl TokenScope<'_> {
    /// Validate and encode scope bytes, never exceeding the protocol's 512-byte limit.
    pub fn encode_scope(&self) -> Result<Vec<u8>, WireError> {
        let databases = self.databases.as_deref();
        encode_scope(&HttpLoginScope {
            label: self.label,
            mode: match self.mode {
                TokenMode::ReadOnly => HttpLoginMode::ReadOnly,
                TokenMode::ReadWrite => HttpLoginMode::ReadWrite,
            },
            channels: match self.channels {
                TokenChannels::Rest => HttpLoginChannels::Rest,
                TokenChannels::Ws => HttpLoginChannels::Ws,
                TokenChannels::Both => HttpLoginChannels::Both,
            },
            databases,
        })
        .map_err(|_| WireError::Invalid)
    }

    /// Convert scope mode to the existing token issuance type.
    pub fn token_mode(&self) -> TokenMode {
        self.mode
    }

    /// Convert channel choice to the existing token issuance type.
    pub fn token_channels(&self) -> TokenChannels {
        self.channels
    }

    /// Recover scope from the transcript's stored encoded scope, with all canonical checks.
    pub fn from_scope_bytes(bytes: &'_ [u8]) -> Result<TokenScope<'_>, WireError> {
        if bytes.len() > 512 {
            return Err(WireError::Invalid);
        }
        let mut reader = super::reader::Reader::new(bytes);
        let label_len = usize::from(read_u16(&mut reader)?);
        let label =
            core::str::from_utf8(reader.take_public(label_len)?).map_err(|_| WireError::Invalid)?;
        let mode = match reader.byte_public()? {
            0 => TokenMode::ReadOnly,
            1 => TokenMode::ReadWrite,
            _ => return Err(WireError::Invalid),
        };
        let channels = match reader.byte_public()? {
            0 => TokenChannels::Rest,
            1 => TokenChannels::Ws,
            2 => TokenChannels::Both,
            _ => return Err(WireError::Invalid),
        };
        let count = reader.byte_public()?;
        let databases = if count == 0xff {
            None
        } else if (1..=4).contains(&count) {
            let mut names = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let length = usize::from(read_u16(&mut reader)?);
                names.push(
                    core::str::from_utf8(reader.take_public(length)?)
                        .map_err(|_| WireError::Invalid)?,
                );
            }
            Some(names)
        } else {
            return Err(WireError::Invalid);
        };
        reader.finish()?;
        let scope = TokenScope {
            label,
            mode,
            channels,
            databases,
        };
        if scope.encode_scope()?.as_slice() != bytes {
            return Err(WireError::Invalid);
        }
        Ok(scope)
    }
}

fn read_u16(reader: &mut super::reader::Reader<'_>) -> Result<u16, WireError> {
    let bytes = reader.take_public(2)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}
