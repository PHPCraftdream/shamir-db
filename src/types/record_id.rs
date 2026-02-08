use crate::types::base::{self, Base58Error};
use bytes::Bytes;
use chrono::Utc;
use rand::TryRngCore;
use rkyv::{Archive, Deserialize as RkyvDeserialize, Serialize as RkyvSerialize};
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Archive, RkyvSerialize, RkyvDeserialize)]
pub struct RecordId(pub [u8; 16]);

impl RecordId {
    pub fn new() -> Self {
        let mut bytes = [0u8; 16];

        // Timestamp part, relative to the custom epoch.
        let now_micros = Utc::now().timestamp_micros();
        let relative_micros = now_micros.saturating_sub(CUSTOM_EPOCH_MICROS);
        bytes[0..8].copy_from_slice(&relative_micros.to_be_bytes());

        // Random part
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes[8..16])
            .expect("Failed to get random bytes from OS");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::codec;
    use std::collections::HashSet;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_record_id_uniqueness() {
        let mut ids = HashSet::new();
        for _ in 0..100_000 {
            assert!(ids.insert(RecordId::new()));
        }
    }

    #[test]
    fn test_record_id_ordering() {
        let id1 = RecordId::new();
        thread::sleep(Duration::from_micros(10));
        let id2 = RecordId::new();
        assert!(id1 < id2, "id1 should be less than id2");
    }

    #[test]
    fn test_string_roundtrip() {
        let id = RecordId::new();
        let s = id.to_string();
        let reconstructed_id: RecordId = s.parse().unwrap();
        assert_eq!(id, reconstructed_id);
    }

    #[test]
    fn test_system_record_id_logic() {
        // Short name
        let id_short = RecordId::system("users");
        assert!(id_short.is_system());
        assert_eq!(&id_short.0[0..4], &[0, 0, 0, 0]);
        assert_eq!(&id_short.0[4..9], b"users");
        assert_eq!(id_short.0[9], 0); // Padding

        // 12-byte name
        let id_exact = RecordId::system("123456789012");
        assert_eq!(&id_exact.0[4..16], b"123456789012");

        // Long name (truncation)
        let id_long = RecordId::system("123456789012-extra");
        assert_eq!(
            id_exact, id_long,
            "Long name should be truncated to the same ID"
        );

        // Determinism
        let id_again = RecordId::system("users");
        assert_eq!(id_short, id_again);

        // User ID should not be system
        let user_id = RecordId::new();
        assert!(!user_id.is_system());
    }

    #[test]
    fn test_rkyv_roundtrip() {
        let id = RecordId::new();
        let bytes = codec::to_bytes(&id).unwrap();
        let deserialized: RecordId = codec::from_bytes(&bytes).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_rkyv_zero_copy() {
        let id = RecordId::new();
        let bytes = codec::to_bytes(&id).unwrap();
        let archived = codec::as_archived::<RecordId>(&bytes).unwrap();
        assert_eq!(archived.0, id.0);
    }
}
