use crate::types::base::{self, Base58Error};
use bytes::Bytes;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The custom epoch for our RecordId timestamps, set to 2026-01-31 00:00:00 UTC.
/// This makes the timestamp part of the ID smaller and more manageable.
/// Value: `chrono::Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap().timestamp_micros()`
const CUSTOM_EPOCH_MICROS: i64 = 1_769_817_600_000_000;

/// A prefix of 4 zero bytes, representing a timestamp of 0 relative to the epoch.
/// This is used to identify system records, as a real timestamp will never be zero.
const SYSTEM_RECORD_PREFIX: &[u8] = &[0, 0, 0, 0];

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordId(pub [u8; 16]);

impl RecordId {
    pub fn new() -> Self {
        let mut bytes = [0u8; 16];

        // Timestamp part, relative to the custom epoch.
        let now_micros = Utc::now().timestamp_micros();
        let relative_micros = now_micros.saturating_sub(CUSTOM_EPOCH_MICROS);
        bytes[0..8].copy_from_slice(&relative_micros.to_be_bytes());

        // Random part — `thread_rng` is a thread-local ChaCha
        // CSPRNG re-seeded periodically from `OsRng`. Per-call
        // cost is ~5 ns (pure user-space) vs. ~150 ns for
        // `OsRng` (BCryptGenRandom syscall on Windows). RecordId
        // gets called once per inserted record, so this is the
        // dominant non-timestamp cost on hot insert paths.
        rand::thread_rng().fill_bytes(&mut bytes[8..16]);
        Self(bytes)
    }

    /// Creates a deterministic, system-level `RecordId` from a string name.
    /// These IDs are used for internal metadata and are distinguished by a zero-timestamp prefix.
    /// The name is directly copied into the ID, truncated to 12 bytes.
    pub fn system(name: &str) -> Self {
        let mut bytes = [0u8; 16];
        // The first 4 bytes are the system prefix (zeros).
        // The rest is filled with the name's bytes.
        let name_bytes = name.as_bytes();
        let len_to_copy = std::cmp::min(name_bytes.len(), 12);
        bytes[4..4 + len_to_copy].copy_from_slice(&name_bytes[..len_to_copy]);
        Self(bytes)
    }

    /// Checks if the `RecordId` is a system record.
    /// Returns `true` if the first 4 bytes are zero.
    pub fn is_system(&self) -> bool {
        self.0.starts_with(SYSTEM_RECORD_PREFIX)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn to_bytes(&self) -> Bytes {
        Bytes::copy_from_slice(&self.0)
    }
}

impl Default for RecordId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RecordId({})", self)
    }
}

impl fmt::Display for RecordId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", base::encode(&self.0))
    }
}

#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct RecordIdError(#[from] Base58Error);

impl FromStr for RecordId {
    type Err = RecordIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let arr = base::decode16(s)?;
        Ok(RecordId(arr))
    }
}

impl Serialize for RecordId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for RecordId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: &[u8] = serde::Deserialize::deserialize(deserializer)?;
        let arr: [u8; 16] = bytes
            .try_into()
            .map_err(|_| serde::de::Error::invalid_length(bytes.len(), &"16 bytes"))?;
        Ok(RecordId(arr))
    }
}
