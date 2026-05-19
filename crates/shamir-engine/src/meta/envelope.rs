//! Versioned envelope for engine metadata.
//!
//! Wraps every persisted `__meta__/*` payload in
//! `[magic="SDB2"][version: u16][written_at_nanos: u64][payload: T]`
//! so future migrations can dispatch on `version` without ambiguity.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ENVELOPE_MAGIC: [u8; 4] = *b"SDB2";
pub const ENVELOPE_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaEnvelope<T> {
    pub magic: [u8; 4],
    pub version: u16,
    pub written_at_nanos: u64,
    pub payload: T,
}

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("bad magic: expected {expected:?}, got {got:?}")]
    BadMagic { expected: [u8; 4], got: [u8; 4] },
    #[error("unsupported envelope version: {0}")]
    UnsupportedVersion(u16),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("encode error: {0}")]
    Encode(String),
}

impl<T> MetaEnvelope<T>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    pub fn new(payload: T) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self {
            magic: ENVELOPE_MAGIC,
            version: ENVELOPE_VERSION,
            written_at_nanos: nanos,
            payload,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, MetaError> {
        bincode::serialize(self).map_err(|e| MetaError::Encode(e.to_string()))
    }

    pub fn open(bytes: &[u8]) -> Result<T, MetaError> {
        let env: MetaEnvelope<T> =
            bincode::deserialize(bytes).map_err(|e| MetaError::Decode(e.to_string()))?;
        if env.magic != ENVELOPE_MAGIC {
            return Err(MetaError::BadMagic {
                expected: ENVELOPE_MAGIC,
                got: env.magic,
            });
        }
        if env.version != ENVELOPE_VERSION {
            return Err(MetaError::UnsupportedVersion(env.version));
        }
        Ok(env.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        a: u32,
        b: String,
    }

    #[test]
    fn round_trip() {
        let payload = Sample { a: 42, b: "hello".into() };
        let env = MetaEnvelope::new(Sample { a: 42, b: "hello".into() });
        let bytes = env.encode().unwrap();
        let got: Sample = MetaEnvelope::open(&bytes).unwrap();
        assert_eq!(got, payload);
    }

    #[test]
    fn bad_magic_rejected() {
        let mut env = MetaEnvelope::new(Sample { a: 1, b: "x".into() });
        env.magic = *b"XXXX";
        let bytes = bincode::serialize(&env).unwrap();
        let err = MetaEnvelope::<Sample>::open(&bytes).unwrap_err();
        assert!(matches!(err, MetaError::BadMagic { .. }));
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut env = MetaEnvelope::new(Sample { a: 1, b: "x".into() });
        env.version = 999;
        let bytes = bincode::serialize(&env).unwrap();
        let err = MetaEnvelope::<Sample>::open(&bytes).unwrap_err();
        assert!(matches!(err, MetaError::UnsupportedVersion(999)));
    }
}
