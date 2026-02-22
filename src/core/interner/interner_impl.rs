use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::Mutex;

use super::{InternedKey, TouchInd, UserKey};

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

    /// Determine key size based on the count of keys.
    /// Used both for migration decisions and for determining size when loading state.
    /// Returns the appropriate byte size: 1 (u8), 2 (u16), 4 (u32), or 8 (u64).
    ///
    /// For migration: migrate when count >= threshold (255 for u8->u16, etc.)
    /// For loading state: if count is at threshold, assume we need to use a larger size
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
    /// This creates new InternedKey instances with updated byte size.
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
    /// This is used to "hydrate" interner from a persistent store.
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

    /// Gets an ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        // First check if key exists without holding lock
        if let Some(existing) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        // Acquire migration lock to prevent concurrent migrations
        let _migration_guard = self.migration_lock.lock().unwrap();

        // Check again - another thread may have added key or triggered migration while we waited
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

    /// Create an InternedKey from a numeric ID using current key size.
    pub fn make_key(&self, id: u64) -> InternedKey {
        let size = self.key_size();
        InternedKey::new(id, size)
    }
}
