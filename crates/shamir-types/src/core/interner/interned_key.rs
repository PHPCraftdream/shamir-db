use bytes::Bytes;
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// Interned binary key - represents a compressed ID stored as an inline u64.
/// Zero heap allocation. Wire format adapts dynamically: 1, 2, 4, or 8 bytes
/// based on id value (for serialization compatibility).
#[derive(Clone, Debug)]
pub struct InternerKey(u64);

impl InternerKey {
    /// Create a new interned key from u64.
    #[inline]
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Get the u64 ID.
    #[inline]
    pub fn id(&self) -> u64 {
        self.0
    }

    /// Get the minimal wire-format bytes for this key (owned).
    /// Returns 1, 2, 4, or 8 bytes depending on value magnitude.
    #[inline]
    pub fn to_wire_bytes(&self) -> Bytes {
        if self.0 <= u8::MAX as u64 {
            Bytes::copy_from_slice(&[self.0 as u8])
        } else if self.0 <= u16::MAX as u64 {
            Bytes::copy_from_slice(&(self.0 as u16).to_le_bytes())
        } else if self.0 <= u32::MAX as u64 {
            Bytes::copy_from_slice(&(self.0 as u32).to_le_bytes())
        } else {
            Bytes::copy_from_slice(&self.0.to_le_bytes())
        }
    }

    /// Wire-format byte length (1, 2, 4, or 8).
    #[inline]
    pub fn wire_len(&self) -> usize {
        if self.0 <= u8::MAX as u64 {
            1
        } else if self.0 <= u16::MAX as u64 {
            2
        } else if self.0 <= u32::MAX as u64 {
            4
        } else {
            8
        }
    }

    /// Borrow the inner bytes representation (allocates on demand).
    /// Prefer `.id()` or `.to_wire_bytes()` for new code.
    #[inline]
    pub fn bytes(&self) -> Bytes {
        self.to_wire_bytes()
    }

    /// Take ownership of the inner bytes (allocates on demand).
    /// Prefer `.id()` or `.to_wire_bytes()` for new code.
    #[inline]
    pub fn into_bytes(self) -> Bytes {
        self.to_wire_bytes()
    }

    /// Get raw bytes as a fixed-size array written to stack.
    /// Returns a buffer and the valid length within it.
    #[inline]
    pub fn as_bytes_buf(&self) -> ([u8; 8], usize) {
        let mut buf = [0u8; 8];
        let len = self.wire_len();
        match len {
            1 => buf[0] = self.0 as u8,
            2 => buf[..2].copy_from_slice(&(self.0 as u16).to_le_bytes()),
            4 => buf[..4].copy_from_slice(&(self.0 as u32).to_le_bytes()),
            8 => buf = self.0.to_le_bytes(),
            _ => unreachable!(),
        }
        (buf, len)
    }
}

impl Hash for InternerKey {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for InternerKey {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for InternerKey {}

impl PartialOrd for InternerKey {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for InternerKey {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl serde::Serialize for InternerKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (buf, len) = self.as_bytes_buf();
        serializer.serialize_bytes(&buf[..len])
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
        let id = match bytes.len() {
            1 => bytes[0] as u64,
            2 => u16::from_le_bytes([bytes[0], bytes[1]]) as u64,
            4 => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as u64,
            8 => u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            _ => return Err("Invalid InternedKey length: must be 1, 2, 4, or 8 bytes"),
        };
        Ok(Self(id))
    }
}
