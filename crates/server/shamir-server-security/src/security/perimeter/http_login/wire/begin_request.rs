use super::reader::{bin_value, string_value, uint_value, Reader, Value};
use super::scope::{TokenScopeChannels, TokenScopeMode};
use super::{TokenScope, WireError};
use shamir_connect::common::username::NormalizedUsername;

/// Borrowed validated BEGIN body. `scope` owns only its bounded parsed database vector.
pub struct BeginRequest<'a> {
    /// Canonical username as sent by the client.
    pub user: &'a str,
    /// Client nonce.
    pub client_nonce: [u8; 32],
    /// Requested token scope.
    pub scope: TokenScope<'a>,
}

impl<'a> BeginRequest<'a> {
    /// Parse and validate a complete BEGIN MessagePack map (maximum 4,096 bytes).
    pub fn parse(bytes: &'a [u8]) -> Result<Self, WireError> {
        if bytes.len() > 4096 {
            return Err(WireError::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let pairs = reader.map(&[
            "protocol_version",
            "user",
            "client_nonce",
            "binding_mode",
            "token",
        ])?;
        reader.finish()?;
        let mut version = None;
        let mut user = None;
        let mut nonce = None;
        let mut binding = None;
        let mut token = None;
        for (key, value) in pairs {
            match key {
                "protocol_version" => version = Some(uint_value(value, 255)?),
                "user" => user = Some(string_value(value)?),
                "client_nonce" => nonce = Some(bin_value::<32>(value)?),
                "binding_mode" => binding = Some(uint_value(value, 255)?),
                "token" => token = Some(value),
                _ => return Err(WireError::Invalid),
            }
        }
        if version != Some(1) || binding != Some(3) {
            return Err(WireError::Invalid);
        }
        let user = user.ok_or(WireError::Invalid)?;
        if user.is_empty() || user.len() > 255 {
            return Err(WireError::Invalid);
        }
        let canonical = NormalizedUsername::from_raw(user).map_err(|_| WireError::Invalid)?;
        if canonical.as_str() != user {
            return Err(WireError::Invalid);
        }
        let client_nonce = nonce.ok_or(WireError::Invalid)?;
        if client_nonce.iter().all(|byte| *byte == 0) {
            return Err(WireError::Invalid);
        }
        let scope = TokenScope::parse_value(token.ok_or(WireError::Invalid)?)?;
        Ok(Self {
            user,
            client_nonce,
            scope,
        })
    }
}

impl<'a> TokenScope<'a> {
    fn parse_value(value: Value<'a>) -> Result<Self, WireError> {
        let Value::Map { count, bytes } = value else {
            return Err(WireError::Invalid);
        };
        if count != 4 {
            return Err(WireError::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let pairs = reader.map(&["label", "mode", "channels", "databases"])?;
        reader.finish()?;
        let mut label = None;
        let mut mode = None;
        let mut channels = None;
        let mut databases = None;
        for (key, value) in pairs {
            match key {
                "label" => label = Some(string_value(value)?),
                "mode" => mode = Some(string_value(value)?),
                "channels" => channels = Some(string_value(value)?),
                "databases" => databases = Some(value),
                _ => return Err(WireError::Invalid),
            }
        }
        let label = label.ok_or(WireError::Invalid)?;
        let mode = match mode.ok_or(WireError::Invalid)? {
            "read_only" => TokenScopeMode::ReadOnly,
            "read_write" => TokenScopeMode::ReadWrite,
            _ => return Err(WireError::Invalid),
        };
        let channels = match channels.ok_or(WireError::Invalid)? {
            "rest" => TokenScopeChannels::Rest,
            "ws" => TokenScopeChannels::Ws,
            "both" => TokenScopeChannels::Both,
            _ => return Err(WireError::Invalid),
        };
        let databases = match databases.ok_or(WireError::Invalid)? {
            Value::Nil => None,
            Value::Array { count, bytes } if (1..=4).contains(&count) => {
                let mut nested = Reader::new(bytes);
                let mut list = Vec::with_capacity(count);
                for _ in 0..count {
                    list.push(nested.string()?);
                }
                nested.finish()?;
                Some(list)
            }
            _ => return Err(WireError::Invalid),
        };
        let scope = Self {
            label,
            mode,
            channels,
            databases,
        };
        if scope.encode_scope().is_err() {
            return Err(WireError::Invalid);
        }
        Ok(scope)
    }
}
