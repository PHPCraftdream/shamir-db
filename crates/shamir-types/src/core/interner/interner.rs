use crate::types::common::{new_dash_map_wc, TDashMap};
use std::sync::{Mutex, RwLock};

use super::{InternerKey, TouchInd, UserKey};

/// A thread-safe, two-way map for interning strings into compact binary IDs.
///
/// **Reverse-lookup layout (Opt F):** the `id → UserKey` direction is
/// stored as a `RwLock<Vec<Option<UserKey>>>` indexed by `id as usize`.
/// `get_str` becomes a single bounds-check + clone instead of a
/// `DashMap` shard lookup + key hash + Arc-style clone, which is ~3-5×
/// cheaper per call. The interner is monotonic (entries are never
/// removed), so the vec only grows — no holes to compact, and the
/// `Option<UserKey>` only carries `None` at positions never allocated
/// (e.g. id=0, reserved as a sentinel).
///
/// The forward direction (`UserKey → id`) stays a `TDashMap` because
/// it's hit only on the write path, where lock-free is the bigger
/// win.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternerKey>,
    /// Reverse direction — `vec[id as usize] = Some(UserKey)`. Indexed
    /// by raw `id`; entry `0` is always `None` (sentinel, ids start at 1).
    reverse: RwLock<Vec<Option<UserKey>>>,
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
            reverse: RwLock::new(vec![None]), // index 0 reserved
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
        let mut max_id: u64 = 0;
        for (interned_key, _) in &initial_data {
            let id = interned_key.id();
            if id > max_id {
                max_id = id;
            }
        }
        // +1 because vec is sized to hold index `max_id` (which is
        // the highest id assigned), plus the sentinel at 0.
        let mut reverse: Vec<Option<UserKey>> = vec![None; (max_id as usize) + 1];

        for (interned_key, user_key) in initial_data {
            let id = interned_key.id();
            map_user_to_interned.insert(user_key.clone(), interned_key);
            reverse[id as usize] = Some(user_key);
        }

        Interner {
            map_user_to_interned,
            reverse: RwLock::new(reverse),
            current_id: Mutex::new(max_id),
        }
    }

    /// Gets an ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        // Fast path: existing entry.
        if let Some(existing) = self.map_user_to_interned.get(&key) {
            return Ok(TouchInd::Exists(existing.clone()));
        }

        // Reserve a new ID under the current_id mutex.
        let new_key: InternerKey = {
            let mut current_id = self.current_id.lock().unwrap();
            *current_id += 1;
            InternerKey::new(*current_id)
        };
        let new_id = new_key.id();

        // CAS into forward map — another thread may have raced us.
        use dashmap::mapref::entry::Entry;
        match self.map_user_to_interned.entry(key.clone()) {
            Entry::Occupied(existing) => {
                // Race: another thread inserted between our get() and entry().
                // We allocated `new_id` but won't use it (small wasted slot
                // — interner is monotonic, leaks are harmless).
                Ok(TouchInd::Exists(existing.get().clone()))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(new_key.clone());
                // Append to reverse: grow vec if needed, then place
                // the key at its id index.
                let mut rev = self.reverse.write().unwrap();
                if (new_id as usize) >= rev.len() {
                    rev.resize((new_id as usize) + 1, None);
                }
                rev[new_id as usize] = Some(key);
                Ok(TouchInd::New(new_key))
            }
        }
    }

    /// Gets the user key corresponding to an interned key.
    ///
    /// **Hot path (Opt F):** single read-lock + bounds-check + clone.
    pub fn get_str(&self, id: &InternerKey) -> Option<UserKey> {
        let rev = self.reverse.read().unwrap();
        let idx = id.id() as usize;
        rev.get(idx).and_then(|slot| slot.clone())
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
        let rev = self.reverse.read().unwrap();
        rev.iter()
            .enumerate()
            .filter_map(|(idx, slot)| {
                slot.as_ref()
                    .map(|key| (InternerKey::new(idx as u64), key.clone()))
            })
            .collect()
    }
}
