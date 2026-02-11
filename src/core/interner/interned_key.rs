use bytes::Bytes;

/// Interned binary key - represents a compressed ID stored as variable-size bytes.
/// Size adapts dynamically: 1, 2, 4, or 8 bytes based on interned key count.
#[derive(PartialEq, Eq, Hash, Clone, Debug, PartialOrd, Ord)]
pub struct InternedKey(pub Bytes);

impl InternedKey {
    /// Create a new interned key from u64 with specific byte size.
    /// Size determines how many bytes to use: 1 (u8), 2 (u16), 4 (u32), or 8 (u64).
    pub fn new(id: u64, size: u8) -> Self {
        let bytes = match size {
            1 => {
                assert!(id <= u8::MAX as u64, "ID {} exceeds u8 capacity", id);
                Bytes::copy_from_slice(&[id as u8])
            }
            2 => {
                assert!(id <= u16::MAX as u64, "ID {} exceeds u16 capacity", id);
                Bytes::copy_from_slice(&(id as u16).to_le_bytes())
            }
            4 => {
                assert!(id <= u32::MAX as u64, "ID {} exceeds u32 capacity", id);
                Bytes::copy_from_slice(&(id as u32).to_le_bytes())
            }
            8 => Bytes::copy_from_slice(&id.to_le_bytes()),
            _ => panic!("Invalid key size: {} (must be 1, 2, 4, or 8)", size),
        };
        Self(bytes)
    }

    /// Convert bytes back to u64 ID.
    pub fn id(&self) -> u64 {
        match self.0.len() {
            1 => self.0[0] as u64,
            2 => {
                let arr: [u8; 2] = [self.0[0], self.0[1]];
                u16::from_le_bytes(arr) as u64
            }
            4 => {
                let arr: [u8; 4] = [self.0[0], self.0[1], self.0[2], self.0[3]];
                u32::from_le_bytes(arr) as u64
            }
            8 => {
                let arr: [u8; 8] = [
                    self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6],
                    self.0[7],
                ];
                u64::from_le_bytes(arr)
            }
            _ => panic!(
                "Invalid key length: {} (must be 1, 2, 4, or 8)",
                self.0.len()
            ),
        }
    }

    /// Get raw bytes reference.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl serde::Serialize for InternedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for InternedKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        InternedKey::from_raw_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

impl InternedKey {
    /// Create from raw bytes (for deserialization).
    fn from_raw_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        let len = bytes.len();
        if len != 1 && len != 2 && len != 4 && len != 8 {
            return Err("Invalid InternedKey length: must be 1, 2, 4, or 8 bytes");
        }
        Ok(Self(Bytes::copy_from_slice(bytes)))
    }
}
