use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::Mutex;

use super::{InternerKey, TouchInd, UserKey};

/// A thread-safe, two-way map for interning strings into compact binary IDs.
/// Keys use variable-size bytes (1/2/4/8 bytes) based on id value.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternerKey>,
    map_interned_to_user: TDashMap<InternerKey, UserKey>,
    current_id: Mutex<u64>,
}

impl Default for Interner {
    fn default() -> Self {
        Self::new()
    }
}

impl Interner {
    /// Creates a new, empty Interner.
    pub fn new() -> Interner {
        Interner {
            map_user_to_interned: new_dash_map_wc(64),
            map_interned_to_user: new_dash_map_wc(64),
            current_id: Mutex::new(0),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternerKey, UserKey)>) -> Self {
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

        Interner {
            map_user_to_interned,
            map_interned_to_user,
            current_id: Mutex::new(max_id),
        }
    }

    /// Gets an ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        // Check if key exists
        if let Some(existing) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        // Create new ID under lock
        let new_key: InternerKey = {
            let mut current_id = self.current_id.lock().unwrap();
            *current_id += 1;
            InternerKey::new(*current_id)
        };

        // Use entry API to handle race condition
        use dashmap::mapref::entry::Entry;
        match self.map_user_to_interned.entry(key.clone()) {
            Entry::Occupied(existing) => {
                // Another thread inserted first
                Ok(TouchInd::Exists(existing.get().clone()))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(new_key.clone());
                self.map_interned_to_user.insert(new_key.clone(), key);
                Ok(TouchInd::New(new_key))
            }
        }
    }

    /// Gets the user key corresponding to an interned key.
    pub fn get_str(&self, id: &InternerKey) -> Option<UserKey> {
        self.map_interned_to_user.get(id).map(|k| k.clone())
    }

    /// Gets the interned key corresponding to a user key.
    pub fn get_ind<S: AsRef<str>>(&self, str: S) -> Option<InternerKey> {
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

    /// Create an InternedKey from a numeric ID.
    pub fn make_key(&self, id: u64) -> InternerKey {
        InternerKey::new(id)
    }

    /// Returns all interned entries as (InternerKey, UserKey) pairs.
    pub fn all_entries(&self) -> Vec<(InternerKey, UserKey)> {
        self.map_interned_to_user
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}
