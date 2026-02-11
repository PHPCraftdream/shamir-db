use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::Mutex;

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

/// User-provided key - the original string before interning.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub struct UserKey(pub String);

impl UserKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for UserKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for UserKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl UserKey {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str<S: AsRef<str>>(s: S) -> Self {
        UserKey(s.as_ref().to_string())
    }
}

impl std::str::FromStr for UserKey {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(UserKey(s.to_string()))
    }
}

impl serde::Serialize for UserKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for UserKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(UserKey(s))
    }
}

/// Result of touching a key - indicates if it was newly created or already existed.
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
pub enum TouchInd {
    New(InternedKey),
    Exists(InternedKey),
}

impl AsRef<[u8]> for TouchInd {
    fn as_ref(&self) -> &[u8] {
        match self {
            TouchInd::New(key) => key.as_bytes(),
            TouchInd::Exists(key) => key.as_bytes(),
        }
    }
}

impl TouchInd {
    /// Returns the interned key.
    pub fn key(&self) -> &InternedKey {
        match self {
            TouchInd::New(key) => key,
            TouchInd::Exists(key) => key,
        }
    }

    /// Returns true if the key was newly created.
    pub fn is_new(&self) -> bool {
        match self {
            TouchInd::New(_) => true,
            TouchInd::Exists(_) => false,
        }
    }
}

/// A thread-safe, two-way map for interning strings into compact binary IDs.
/// Keys use variable-size bytes (1/2/4/8 bytes) adapting to the number of interned keys.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternedKey>,
    map_interned_to_user: TDashMap<InternedKey, UserKey>,
    current_id: Mutex<u64>,
    key_size: Mutex<u8>,       // 1, 2, 4, or 8 bytes
    migration_lock: Mutex<()>, // Ensures only one thread migrates at a time
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    /// Creates a new, empty Interner with 1-byte keys.
    pub fn new() -> Interner {
        Interner {
            map_user_to_interned: new_dash_map_wc(64),
            map_interned_to_user: new_dash_map_wc(64),
            current_id: Mutex::new(0), // IDs start from 0
            key_size: Mutex::new(1),   // Start with 1-byte keys
            migration_lock: Mutex::new(()),
        }
    }

    /// Determine the key size based on the count of keys.
    /// Used both for migration decision and for determining size when loading state.
    /// Returns the appropriate byte size: 1 (u8), 2 (u16), 4 (u32), or 8 (u64).
    ///
    /// For migration: migrate when count >= threshold (255 for u8->u16, etc.)
    /// For loading state: if count is at threshold, assume we need the larger size
    fn calculate_key_size(count: usize) -> u8 {
        if count >= 4_000_000_001 {
            8
        } else if count >= 65536 {
            4
        } else if count >= 256 {
            2
        } else {
            1
        }
    }

    /// Migrate all keys to a new size when threshold is crossed.
    /// This creates new InternedKey instances with the updated byte size.
    fn migrate_keys(&self, new_size: u8) {
        // Collect all current mappings
        let old_mappings: Vec<(UserKey, InternedKey)> = self
            .map_user_to_interned
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect();

        // Clear both maps
        self.map_user_to_interned.clear();
        self.map_interned_to_user.clear();

        // Rebuild with new keys
        for (user_key, old_key) in old_mappings {
            let id = old_key.id();
            let new_key = InternedKey::new(id, new_size);
            self.map_user_to_interned
                .insert(user_key.clone(), new_key.clone());
            self.map_interned_to_user.insert(new_key, user_key);
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" the interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternedKey, UserKey)>) -> Self {
        if initial_data.is_empty() {
            return Self::new();
        }

        let map_user_to_interned = new_dash_map_wc(initial_data.len());
        let map_interned_to_user = new_dash_map_wc(initial_data.len());
        let mut max_id: u64 = 0;

        for (interned_key, user_key) in &initial_data {
            map_user_to_interned.insert(user_key.clone(), interned_key.clone());
            map_interned_to_user.insert(interned_key.clone(), user_key.clone());
            let id = interned_key.id();
            if id > max_id {
                max_id = id;
            }
        }

        let key_size = Self::calculate_key_size(initial_data.len());

        Interner {
            map_user_to_interned,
            map_interned_to_user,
            current_id: Mutex::new(max_id),
            key_size: Mutex::new(key_size),
            migration_lock: Mutex::new(()),
        }
    }

    /// Gets the ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        // First check if key exists without holding lock
        if let Some(existing) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        // Acquire migration lock to prevent concurrent migrations
        let _migration_guard = self.migration_lock.lock().unwrap();

        // Check again - another thread may have added the key or triggered migration while we waited
        if let Some(existing) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        // Check if we need to migrate to larger key size
        let current_count = self.map_user_to_interned.len();
        let new_size = Self::calculate_key_size(current_count + 1); // +1 for the new key we're about to add
        let old_size = *self.key_size.lock().unwrap();

        if new_size != old_size {
            // Perform migration - this rebuilds all keys with new size
            self.migrate_keys(new_size);
            *self.key_size.lock().unwrap() = new_size;

            // After migration, check one more time if another thread already added this key
            if let Some(existing) = self.map_user_to_interned.get(&key) {
                return Ok(TouchInd::Exists(existing.clone()));
            }
        }

        // Get the size to use for new key
        let size_to_use = *self.key_size.lock().unwrap();

        // Create new ID
        let new_key: InternedKey = {
            let mut current_id = self.current_id.lock().unwrap();
            *current_id += 1;
            InternedKey::new(*current_id, size_to_use)
        };

        // Insert new key
        self.map_user_to_interned
            .insert(key.clone(), new_key.clone());
        self.map_interned_to_user.insert(new_key.clone(), key);
        Ok(TouchInd::New(new_key))
    }

    /// Gets the user key corresponding to an interned key.
    pub fn get_str(&self, id: &InternedKey) -> Option<UserKey> {
        self.map_interned_to_user.get(id).map(|k| k.clone())
    }

    /// Gets the interned key corresponding to a user key.
    pub fn get_ind<S: AsRef<str>>(&self, str: S) -> Option<InternedKey> {
        let key = UserKey::from_str(str.as_ref());
        self.map_user_to_interned.get(&key).map(|id| id.clone())
    }

    /// Returns the current number of interned keys.
    pub fn len(&self) -> usize {
        self.map_user_to_interned.len()
    }

    /// Returns true if the interner is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get current key size in bytes.
    pub fn key_size(&self) -> u8 {
        *self.key_size.lock().unwrap()
    }
}
