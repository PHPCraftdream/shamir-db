use crate::types::common::{new_dash_map_wc, TDashMap};
use crate::types::string_int58::StringInt58;
use std::sync::Mutex;

/// Interned base58 key - represents a compressed string ID.
#[derive(PartialEq, Eq, Hash, Clone, Debug, PartialOrd, Ord)]
pub struct InternedKey(pub String);

impl InternedKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for InternedKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InternedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl InternedKey {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str<S: AsRef<str>>(s: S) -> Self {
        InternedKey(s.as_ref().to_string())
    }
}

impl std::str::FromStr for InternedKey {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(InternedKey(s.to_string()))
    }
}

impl serde::Serialize for InternedKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for InternedKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(InternedKey(s))
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

impl AsRef<str> for TouchInd {
    fn as_ref(&self) -> &str {
        match self {
            TouchInd::New(key) => key.as_str(),
            TouchInd::Exists(key) => key.as_str(),
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

/// A thread-safe, two-way map for interning strings into base58 string IDs.
/// This is the core of the key interning mechanism.
#[derive(Debug)]
pub struct Interner {
    map_user_to_interned: TDashMap<UserKey, InternedKey>,
    map_interned_to_user: TDashMap<InternedKey, UserKey>,
    current_id: Mutex<StringInt58>,
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
            current_id: Mutex::new(StringInt58::new()),
        }
    }

    /// Creates a new Interner from a pre-existing state.
    /// This is used to "hydrate" the interner from a persistent store.
    pub fn with_state(initial_data: Vec<(InternedKey, UserKey)>) -> Self {
        let map_user_to_interned = new_dash_map_wc(initial_data.len());
        let map_interned_to_user = new_dash_map_wc(initial_data.len());
        let mut max_id = StringInt58::new();

        for (id, key) in initial_data {
            // Track the maximum ID
            map_user_to_interned.insert(key.clone(), id.clone());
            map_interned_to_user.insert(id.clone(), key);
            // Update max_id to be at least this id
            // We need to increment until we reach or surpass this id
            while max_id.as_str().len() < id.as_str().len()
                || (max_id.as_str().len() == id.as_str().len() && max_id.as_str() < id.as_str())
            {
                max_id.increment();
            }
        }

        Interner {
            map_user_to_interned,
            map_interned_to_user,
            current_id: Mutex::new(max_id),
        }
    }

    /// Gets the ID for a string, creating it if it doesn't exist.
    pub fn touch_ind<S: AsRef<str>>(&self, str: S) -> Result<TouchInd, &'static str> {
        let key = UserKey::from_str(str.as_ref());

        use dashmap::mapref::entry::Entry;

        match self.map_user_to_interned.entry(key.clone()) {
            Entry::Occupied(e) => {
                // Key already exists
                Ok(TouchInd::Exists(e.get().clone()))
            }
            Entry::Vacant(e) => {
                // Key doesn't exist - create new ID and insert atomically
                let new_id: InternedKey = {
                    let mut current_id = self.current_id.lock().unwrap();
                    current_id.increment();
                    InternedKey::from_str(current_id.as_str())
                };

                // Insert into forward map
                e.insert(new_id.clone());

                // Also insert into reverse map
                self.map_interned_to_user.insert(new_id.clone(), key);

                Ok(TouchInd::New(new_id))
            }
        }
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

    /// Get current base58 string ID.
    pub fn current_base58(&self) -> String {
        let current_id = self.current_id.lock().unwrap();
        current_id.as_str().to_string()
    }

    /// Get next base58 ID without interning a string.
    /// This advances the base58 generator and returns the new ID.
    pub fn next_base58(&self) -> String {
        let mut current_id = self.current_id.lock().unwrap();
        current_id.increment();
        current_id.as_str().to_string()
    }
}
