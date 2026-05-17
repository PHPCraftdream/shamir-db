use bytes::Bytes;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// Interned binary key - represents a compressed ID stored as variable-size bytes.
/// Size adapts dynamically: 1, 2, 4, or 8 bytes based on id value.
#[derive(Clone, Debug)]
pub struct InternerKey(pub(crate) Bytes);

impl InternerKey {
    /// Create a new interned key from u64 with minimal byte size.
    pub fn new(id: u64) -> Self {
        let bytes = if id <= u8::MAX as u64 {
            Bytes::copy_from_slice(&[id as u8])
        } else if id <= u16::MAX as u64 {
            Bytes::copy_from_slice(&(id as u16).to_le_bytes())
        } else if id <= u32::MAX as u64 {
            Bytes::copy_from_slice(&(id as u32).to_le_bytes())
        } else {
            Bytes::copy_from_slice(&id.to_le_bytes())
        };
        Self(bytes)
    }

    /// Convert bytes back to u64 ID.
    pub fn id(&self) -> u64 {
        match self.0.len() {
            1 => self.0[0] as u64,
            2 => u16::from_le_bytes([self.0[0], self.0[1]]) as u64,
            4 => u32::from_le_bytes([self.0[0], self.0[1], self.0[2], self.0[3]]) as u64,
            8 => u64::from_le_bytes([
                self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6],
                self.0[7],
            ]),
            _ => unreachable!(
            "InternerKey invariant broken: length {} (must be 1/2/4/8)",
            self.0.len()
        ),
        }
    }

    /// Get raw bytes reference.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the inner bytes representation.
    pub fn bytes(&self) -> &Bytes {
        &self.0
    }

    /// Take ownership of the inner bytes.
    pub fn into_bytes(self) -> Bytes {
        self.0
    }
}

// Hash based on id, not bytes - allows keys of different sizes to match
impl Hash for InternerKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id().hash(state);
    }
}

// Eq based on id, not bytes
impl PartialEq for InternerKey {
    fn eq(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
}

impl Eq for InternerKey {}

// Ord based on id
impl PartialOrd for InternerKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InternerKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id().cmp(&other.id())
    }
}

impl serde::Serialize for InternerKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for InternerKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        InternerKey::from_raw_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl InternerKey {
    /// Create from raw bytes (for deserialization).
    fn from_raw_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        let len = bytes.len();
        if len != 1 && len != 2 && len != 4 && len != 8 {
            return Err("Invalid InternedKey length: must be 1, 2, 4, or 8 bytes");
        }
        Ok(Self(Bytes::copy_from_slice(bytes)))
    }
}
