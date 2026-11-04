use std::fmt;
use std::io::ErrorKind;

use serde::de::{self, DeserializeOwned, IgnoredAny, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_bytes::Bytes;
use zeroize::Zeroizing;

use super::login_scope::{LoginChannels, LoginMode, LoginScope};
use crate::error::ClientError;

const PROTOCOL_VERSION: u8 = 1;
const BINDING_MODE: u8 = 3;
const ARGON2_V13: u8 = 0x13;

/// A msgpack `bin` of exactly `N` bytes; `str` and arrays are refused.
struct Bin<const N: usize>([u8; N]);

impl<'de, const N: usize> Deserialize<'de> for Bin<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct BinVisitor<const N: usize>;
        impl<const N: usize> Visitor<'_> for BinVisitor<N> {
            type Value = Bin<N>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a {N}-byte bin")
            }

            fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Bin<N>, E> {
                <[u8; N]>::try_from(v)
                    .map(Bin)
                    .map_err(|_| E::invalid_length(v.len(), &self))
            }
        }
        d.deserialize_bytes(BinVisitor::<N>)
    }
}

/// A string whose buffer is wiped on drop, including when a later field fails.
struct SecretString(Zeroizing<String>);

impl<'de> Deserialize<'de> for SecretString {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct SecretVisitor;
        impl Visitor<'_> for SecretVisitor {
            type Value = SecretString;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<SecretString, E> {
                Ok(SecretString(Zeroizing::new(v.to_owned())))
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &str) -> Result<SecretString, E> {
                self.visit_str(v)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<SecretString, E> {
                Ok(SecretString(Zeroizing::new(v)))
            }
        }
        d.deserialize_str(SecretVisitor)
    }
}

fn decode_strict<T: DeserializeOwned>(buf: &[u8]) -> Option<T> {
    if !matches!(buf.first(), Some(0x80..=0x8f | 0xde | 0xdf)) {
        return None;
    }
    // Reading from the slice borrows strings, so no copy of a token stays in a decoder buffer.
    let mut decoder = rmp_serde::Deserializer::from_read_ref(buf);
    let value = T::deserialize(&mut decoder).ok()?;
    match IgnoredAny::deserialize(&mut decoder) {
        Err(rmp_serde::decode::Error::InvalidMarkerRead(e))
            if e.kind() == ErrorKind::UnexpectedEof =>
        {
            Some(value)
        }
        _ => None,
    }
}

fn nullable_databases<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
    Option::<Vec<String>>::deserialize(d)
}

#[derive(Debug)]
pub(crate) struct Challenge {
    pub(crate) security: u8,
    pub(crate) listener_id: [u8; 16],
    pub(crate) handshake_id: [u8; 32],
    pub(crate) salt: [u8; 16],
    pub(crate) memory_kb: u32,
    pub(crate) time: u32,
    pub(crate) parallelism: u32,
    pub(crate) argon2_version: u8,
    pub(crate) server_nonce: [u8; 32],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChallenge {
    protocol_version: u8,
    binding_mode: u8,
    security: u8,
    listener_id: Bin<16>,
    handshake_id: Bin<32>,
    salt: Bin<16>,
    kdf: String,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    server_nonce: Bin<32>,
}

fn invalid_challenge() -> ClientError {
    ClientError::Protocol("invalid login challenge".into())
}

/// Strict decode of the begin reply; the first msgpack map must carry exactly the spec keys.
pub(crate) fn decode_challenge(buf: &[u8]) -> Result<Challenge, ClientError> {
    let raw: RawChallenge = decode_strict(buf).ok_or_else(invalid_challenge)?;
    if raw.protocol_version != PROTOCOL_VERSION
        || raw.binding_mode != BINDING_MODE
        || raw.kdf != "argon2id"
        || raw.argon2_version != ARGON2_V13
    {
        return Err(invalid_challenge());
    }
    Ok(Challenge {
        security: raw.security,
        listener_id: raw.listener_id.0,
        handshake_id: raw.handshake_id.0,
        salt: raw.salt.0,
        memory_kb: raw.memory_kb,
        time: raw.time,
        parallelism: raw.parallelism,
        argon2_version: raw.argon2_version,
        server_nonce: raw.server_nonce.0,
    })
}

pub(crate) struct FinishSuccess {
    pub(crate) server_signature: [u8; 32],
    pub(crate) token: Zeroizing<String>,
    pub(crate) id: [u8; 16],
    pub(crate) label: String,
    pub(crate) owner: String,
    pub(crate) mode: LoginMode,
    pub(crate) channels: LoginChannels,
    pub(crate) databases: Option<Vec<String>>,
    pub(crate) created_at_ns: u64,
    pub(crate) expires_at_ns: u64,
}

impl fmt::Debug for FinishSuccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinishSuccess")
            .field("server_signature", &"<redacted>")
            .field("token", &"<redacted>")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("owner", &self.owner)
            .field("mode", &self.mode)
            .field("channels", &self.channels)
            .field("databases", &self.databases)
            .field("created_at_ns", &self.created_at_ns)
            .field("expires_at_ns", &self.expires_at_ns)
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFinish {
    protocol_version: u8,
    server_signature: Bin<32>,
    token: SecretString,
    id: Bin<16>,
    label: String,
    owner: String,
    mode: LoginMode,
    channels: LoginChannels,
    #[serde(deserialize_with = "nullable_databases")]
    databases: Option<Vec<String>>,
    created_at_ns: u64,
    expires_at_ns: u64,
}

fn invalid_finish() -> ClientError {
    ClientError::Protocol("invalid login response".into())
}

/// Strict decode of the finish reply; the token never leaves a zeroizing buffer.
pub(crate) fn decode_finish(buf: &[u8]) -> Result<FinishSuccess, ClientError> {
    let raw: RawFinish = decode_strict(buf).ok_or_else(invalid_finish)?;
    if raw.protocol_version != PROTOCOL_VERSION {
        return Err(invalid_finish());
    }
    Ok(FinishSuccess {
        server_signature: raw.server_signature.0,
        token: raw.token.0,
        id: raw.id.0,
        label: raw.label,
        owner: raw.owner,
        mode: raw.mode,
        channels: raw.channels,
        databases: raw.databases,
        created_at_ns: raw.created_at_ns,
        expires_at_ns: raw.expires_at_ns,
    })
}

#[derive(Serialize)]
struct BeginScope<'a> {
    label: &'a str,
    mode: LoginMode,
    channels: LoginChannels,
    databases: Option<&'a [String]>,
}

#[derive(Serialize)]
struct Begin<'a> {
    protocol_version: u8,
    user: &'a str,
    client_nonce: &'a Bytes,
    binding_mode: u8,
    token: BeginScope<'a>,
}

pub(crate) fn encode_begin(
    user: &str,
    client_nonce: &[u8; 32],
    scope: &LoginScope,
) -> Result<Vec<u8>, ClientError> {
    Ok(rmp_serde::to_vec_named(&Begin {
        protocol_version: PROTOCOL_VERSION,
        user,
        client_nonce: Bytes::new(client_nonce),
        binding_mode: BINDING_MODE,
        token: BeginScope {
            label: &scope.label,
            mode: scope.mode,
            channels: scope.channels,
            databases: scope.databases.as_deref(),
        },
    })?)
}

#[derive(Serialize)]
struct Finish<'a> {
    protocol_version: u8,
    handshake_id: &'a Bytes,
    client_proof: &'a Bytes,
}

pub(crate) fn encode_finish(
    handshake_id: &[u8; 32],
    client_proof: &[u8; 32],
) -> Result<Vec<u8>, ClientError> {
    Ok(rmp_serde::to_vec_named(&Finish {
        protocol_version: PROTOCOL_VERSION,
        handshake_id: Bytes::new(handshake_id),
        client_proof: Bytes::new(client_proof),
    })?)
}
