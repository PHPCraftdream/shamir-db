use crate::types::base::{self, Base58Error};
use bytes::Bytes;
use chrono::Utc;
use rand::RngCore;
use rand_xoshiro::Xoshiro256PlusPlus;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
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
        Self::from_ts(Utc::now().timestamp_micros())
    }

    /// Returns the current wall-clock timestamp in microseconds —
    /// the same scale used by `RecordId::new()` / `RecordId::from_ts`.
    /// Call once before a batch loop, then feed the result to `from_ts`
    /// per row.
    #[inline]
    pub fn now_micros() -> i64 {
        Utc::now().timestamp_micros()
    }

    /// Creates a `RecordId` with a caller-supplied `timestamp_micros`
    /// (absolute, same scale as `Utc::now().timestamp_micros()`) and a
    /// fresh 8-byte random tail.  Use for single-row inserts where no
    /// intra-batch ordering is needed.
    pub fn from_ts(timestamp_micros: i64) -> Self {
        let mut bytes = [0u8; 16];

        // Timestamp part, relative to the custom epoch.
        let relative_micros = timestamp_micros.saturating_sub(CUSTOM_EPOCH_MICROS);
        bytes[0..8].copy_from_slice(&relative_micros.to_be_bytes());

        // Random part — thread-local Xoshiro256++ seeded once from OsRng.
        // ~1-2 ns/call vs ~5 ns for ChaCha (thread_rng). Xoshiro256++ has
        // excellent statistical quality (passes BigCrush) and is sufficient
        // for collision-resistant IDs; CSPRNG is unnecessary here.
        Self::fill_random_tail(&mut bytes[8..16]);
        Self(bytes)
    }

    /// Creates a `RecordId` with a caller-supplied `timestamp_micros` and
    /// an ascending `seq` counter embedded in the tail's high 4 bytes.
    /// Layout: `[0..8] BE relative_ts | [8..12] BE seq (u32) | [12..16] 4 random bytes`.
    ///
    /// Use in batch paths: one `now_micros()` call per batch, then
    /// `from_ts_seq(batch_ts, 0)`, `from_ts_seq(batch_ts, 1)`, ...
    /// This preserves intra-batch monotonicity (ascending byte order)
    /// while keeping the single clock-read hoist from L13.
    pub fn from_ts_seq(timestamp_micros: i64, seq: u32) -> Self {
        let mut bytes = [0u8; 16];

        let relative_micros = timestamp_micros.saturating_sub(CUSTOM_EPOCH_MICROS);
        bytes[0..8].copy_from_slice(&relative_micros.to_be_bytes());

        // Sequence counter in tail's high 4 bytes — gives lex-ascending
        // order within a batch sharing the same timestamp prefix.
        bytes[8..12].copy_from_slice(&seq.to_be_bytes());

        // 4 random bytes for collision resistance across concurrent batches
        // sharing the same ts + seq (practically impossible but defensive).
        Self::fill_random_tail(&mut bytes[12..16]);
        Self(bytes)
    }

    /// Fills the given slice with random bytes from a thread-local
    /// Xoshiro256++ PRNG seeded once from OsRng.
    fn fill_random_tail(dest: &mut [u8]) {
        thread_local! {
            static RNG: RefCell<Xoshiro256PlusPlus> = {
                use rand::SeedableRng;
                RefCell::new(Xoshiro256PlusPlus::from_os_rng())
            };
        }
        RNG.with(|rng| rng.borrow_mut().fill_bytes(dest));
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

    /// Construct a `RecordId` from a byte slice. Returns `None` if the
    /// slice isn't exactly 16 bytes.
    pub fn try_from_bytes(b: &[u8]) -> Option<Self> {
        let arr: [u8; 16] = b.try_into().ok()?;
        Some(Self(arr))
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
